//! Index-backed candidate plans for KQ sources.
//!
//! A source decides which cached rows reach the in-memory evaluator before any
//! `where` predicate runs, so it must not degrade into a walk over the complete
//! status cache. The evaluator stays authoritative: every plan here selects a
//! superset of the wrapper rows its sources can match and adds no semantics of
//! its own. Sources that cannot be answered by an index (`local`, `search`,
//! `track`) keep the ordered full scan.

use std::collections::HashSet;

use serde::Serialize;
use sqlx::SqlitePool;

use super::kq_evaluator::{
    fold, hashtag_parameter_matches, login_provider_acct, selected_login_accounts,
    split_scoped_list_id,
};
use super::kq_filter::{
    LoginAccountIdentity, SourceKind, SourceSpec, SqlPrefilterValue, StatusKey,
};

/// Up to this many source rows, sorting the collected candidates for every
/// page is cheaper than walking the ordered status index and probing each row.
const MAX_SORTED_CANDIDATES: i64 = 20_000;

const FULL_SCAN_SELECT: &str = "SELECT s.* FROM statuses s";
const EFFECTIVE_KEYS_CTE: &str = "kq_effective_keys";

type EntityKey = (String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceScanMode {
    /// At least one source has no index path.
    FullScan,
    /// Sparse sources: collect keys through source indexes, then sort them.
    Candidates,
    /// Dense sources: walk the ordered status index and probe each row.
    Probe,
    /// No login account, list, tag, author, or conversation matched.
    Empty,
}

impl SourceScanMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FullScan => "full_scan",
            Self::Candidates => "candidates",
            Self::Probe => "probe",
            Self::Empty => "empty",
        }
    }
}

/// SQL that yields the source rows of one KQ query as `s`.
///
/// `select_sql` ends at the `FROM` clause; the caller appends `predicate` and
/// its cursor conditions to one `WHERE` clause, then the stable status order.
/// Bindings follow the textual order: select first, then predicate.
#[derive(Debug, Clone)]
pub(crate) struct SourceScanPlan {
    mode: SourceScanMode,
    select_sql: String,
    select_bindings: Vec<SqlPrefilterValue>,
    predicate: Option<String>,
    predicate_bindings: Vec<SqlPrefilterValue>,
}

impl SourceScanPlan {
    fn full_scan() -> Self {
        Self {
            mode: SourceScanMode::FullScan,
            select_sql: FULL_SCAN_SELECT.to_string(),
            select_bindings: Vec::new(),
            predicate: None,
            predicate_bindings: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self {
            mode: SourceScanMode::Empty,
            select_sql: FULL_SCAN_SELECT.to_string(),
            select_bindings: Vec::new(),
            predicate: Some("0".to_string()),
            predicate_bindings: Vec::new(),
        }
    }

    pub(crate) fn mode(&self) -> SourceScanMode {
        self.mode
    }

    pub(crate) fn select_sql(&self) -> &str {
        &self.select_sql
    }

    pub(crate) fn select_bindings(&self) -> &[SqlPrefilterValue] {
        &self.select_bindings
    }

    pub(crate) fn predicate(&self) -> Option<&str> {
        self.predicate.as_deref()
    }

    pub(crate) fn predicate_bindings(&self) -> &[SqlPrefilterValue] {
        &self.predicate_bindings
    }
}

/// Sources that match through the login accounts saved in `login_accounts`.
pub(crate) fn sources_need_login_accounts(sources: &[SourceSpec]) -> bool {
    sources.iter().any(|source| {
        matches!(
            source.kind,
            SourceKind::Home
                | SourceKind::List
                | SourceKind::Mentions
                | SourceKind::Direct
                | SourceKind::Public
                | SourceKind::LocalPublic
                | SourceKind::Bookmarks
                | SourceKind::Favourites
        )
    })
}

/// A source matched on the top/wrapper row itself.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowSource {
    /// Stored `timeline_entries` membership, optionally for some accounts.
    Memberships {
        timeline_types: Vec<String>,
        account_accts: Option<Vec<String>>,
    },
    /// Wrapper author `(account_id, server_domain)`.
    Authors(Vec<EntityKey>),
}

/// A source matched on the effective status. A boost wrapper matches through
/// its original, so these keys are expanded to reblog wrappers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EffectiveSource {
    ViewerState {
        column: &'static str,
        account_accts: Vec<String>,
    },
    DirectVisibility,
    RepliesTo(Vec<String>),
    Mentions {
        /// JSON string literals of the login account IDs, matched verbatim.
        id_literals: Vec<String>,
        /// Lowercase ASCII acct fragments, or None when an acct may match only
        /// after NFKC/case folding and no byte search can exclude a row.
        acct_needles: Option<Vec<String>>,
    },
    Keys(Vec<EntityKey>),
}

pub(crate) async fn plan_source_scan(
    pool: &SqlitePool,
    sources: &[SourceSpec],
    login_accounts: &[LoginAccountIdentity],
    conversation_keys: &[StatusKey],
) -> Result<SourceScanPlan, sqlx::Error> {
    plan_source_scan_with_limit(
        pool,
        sources,
        login_accounts,
        conversation_keys,
        MAX_SORTED_CANDIDATES,
    )
    .await
}

async fn plan_source_scan_with_limit(
    pool: &SqlitePool,
    sources: &[SourceSpec],
    login_accounts: &[LoginAccountIdentity],
    conversation_keys: &[StatusKey],
    max_sorted_candidates: i64,
) -> Result<SourceScanPlan, sqlx::Error> {
    let mut rows = Vec::new();
    let mut effective = Vec::new();
    let mut stored_hashtag_types = None;
    let mut author_keys = Vec::new();
    let mut seen_author_keys = HashSet::new();
    let mut has_conversation = false;
    for source in sources {
        match source.kind {
            SourceKind::Local | SourceKind::Search | SourceKind::Track => {
                return Ok(SourceScanPlan::full_scan());
            }
            SourceKind::Home => {
                push_membership(&mut rows, "home", &source.arguments, login_accounts)
            }
            SourceKind::Public => {
                push_membership(&mut rows, "public", &source.arguments, login_accounts)
            }
            SourceKind::LocalPublic => {
                push_membership(&mut rows, "local", &source.arguments, login_accounts)
            }
            SourceKind::List => {
                for argument in &source.arguments {
                    let (account_selector, list_id) = split_scoped_list_id(argument);
                    let selectors = account_selector
                        .map(|selector| vec![selector.to_string()])
                        .unwrap_or_default();
                    push_membership(
                        &mut rows,
                        &format!("list:{list_id}"),
                        &selectors,
                        login_accounts,
                    );
                }
            }
            SourceKind::Hashtag => {
                if stored_hashtag_types.is_none() {
                    stored_hashtag_types = Some(load_stored_hashtag_types(pool).await?);
                }
                let stored = stored_hashtag_types.as_deref().unwrap_or_default();
                for tag in &source.arguments {
                    let timeline_types = stored
                        .iter()
                        .filter(|timeline_type| {
                            timeline_type
                                .strip_prefix("tag:")
                                .is_some_and(|parameter| hashtag_parameter_matches(parameter, tag))
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if !timeline_types.is_empty() {
                        push_unique(
                            &mut rows,
                            RowSource::Memberships {
                                timeline_types,
                                account_accts: None,
                            },
                        );
                    }
                }
            }
            SourceKind::User => {
                for selector in &source.arguments {
                    for key in resolve_author_keys(pool, selector).await? {
                        if seen_author_keys.insert(key.clone()) {
                            author_keys.push(key);
                        }
                    }
                }
            }
            SourceKind::Bookmarks | SourceKind::Favourites => {
                let account_accts = selected_accts(&source.arguments, login_accounts);
                if !account_accts.is_empty() {
                    let column = if source.kind == SourceKind::Bookmarks {
                        "bookmarked"
                    } else {
                        "favourited"
                    };
                    push_unique(
                        &mut effective,
                        EffectiveSource::ViewerState {
                            column,
                            account_accts,
                        },
                    );
                }
            }
            SourceKind::Mentions => {
                let accounts = selected_login_accounts(&source.arguments, login_accounts);
                if !accounts.is_empty() {
                    let account_ids = accounts
                        .iter()
                        .map(|account| account.account_id.clone())
                        .collect();
                    push_unique(&mut effective, EffectiveSource::RepliesTo(account_ids));
                    push_unique(
                        &mut effective,
                        EffectiveSource::Mentions {
                            id_literals: accounts
                                .iter()
                                .map(|account| json_string(&account.account_id))
                                .collect(),
                            acct_needles: mention_acct_needles(&accounts),
                        },
                    );
                }
            }
            SourceKind::Direct => {
                if !selected_login_accounts(&source.arguments, login_accounts).is_empty() {
                    push_unique(&mut effective, EffectiveSource::DirectVisibility);
                }
            }
            SourceKind::Conversation => has_conversation = true,
        }
    }
    if !author_keys.is_empty() {
        rows.push(RowSource::Authors(author_keys));
    }
    if has_conversation && !conversation_keys.is_empty() {
        effective.push(EffectiveSource::Keys(
            conversation_keys
                .iter()
                .map(|key| (key.id.clone(), key.server_domain.clone()))
                .collect(),
        ));
    }
    if rows.is_empty() && effective.is_empty() {
        return Ok(SourceScanPlan::empty());
    }

    let estimate = estimate_source_rows(pool, &rows, &effective, max_sorted_candidates).await?;
    Ok(if estimate <= max_sorted_candidates {
        candidate_plan(&rows, &effective)
    } else {
        probe_plan(&rows, &effective)
    })
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn selected_accts(arguments: &[String], login_accounts: &[LoginAccountIdentity]) -> Vec<String> {
    selected_login_accounts(arguments, login_accounts)
        .into_iter()
        .map(|account| account.acct.clone())
        .collect()
}

fn push_membership(
    rows: &mut Vec<RowSource>,
    timeline_type: &str,
    account_selectors: &[String],
    login_accounts: &[LoginAccountIdentity],
) {
    // `timeline_entries.account_acct` references `login_accounts.acct`, so the
    // evaluator's case-folded account comparison is exact here.
    let account_accts = selected_accts(account_selectors, login_accounts);
    if !account_accts.is_empty() {
        push_unique(
            rows,
            RowSource::Memberships {
                timeline_types: vec![timeline_type.to_string()],
                account_accts: Some(account_accts),
            },
        );
    }
}

async fn load_stored_hashtag_types(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    // Hashtag arguments are case-folded, so resolve the few stored tag
    // memberships here and query them by their exact indexed value.
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT timeline_type FROM timeline_entries
         WHERE timeline_type >= 'tag:' AND timeline_type < 'tag;'",
    )
    .fetch_all(pool)
    .await
}

/// Accounts a `user:"selector"` source can match: provider ID, acct (also as
/// `local@server` for a server-local acct), or username.
async fn resolve_author_keys(
    pool: &SqlitePool,
    selector: &str,
) -> Result<Vec<EntityKey>, sqlx::Error> {
    let selector = selector.trim();
    let identity = selector
        .strip_prefix('@')
        .or_else(|| selector.strip_prefix('#'))
        .unwrap_or(selector);
    let name = identity.trim().trim_start_matches('@');
    let folded = fold(name);
    let local_name = name.split_once('@').map_or(name, |(local, _)| local);
    let folded_local_name = folded
        .split_once('@')
        .map_or(folded.as_str(), |(local, _)| local);
    sqlx::query_as::<_, EntityKey>(
        "SELECT id, server_domain FROM accounts
         WHERE id = ?
            OR acct = ? COLLATE NOCASE OR acct = ? COLLATE NOCASE
            OR acct = ? COLLATE NOCASE OR acct = ? COLLATE NOCASE
            OR username = ? COLLATE NOCASE OR username = ? COLLATE NOCASE",
    )
    .bind(identity)
    .bind(name)
    .bind(&folded)
    .bind(local_name)
    .bind(folded_local_name)
    .bind(name)
    .bind(&folded)
    .fetch_all(pool)
    .await
}

/// Lowercase ASCII fragments one of which occurs in every ASCII mention JSON
/// that names a selected login account by acct.
fn mention_acct_needles(accounts: &[&LoginAccountIdentity]) -> Option<Vec<String>> {
    let mut needles = Vec::new();
    for account in accounts {
        let acct = fold(login_provider_acct(account).trim().trim_start_matches('@'));
        // A mention stores either the full acct or its server-local part.
        let local = acct.split('@').next().unwrap_or_default();
        // JSON escapes these characters, so the raw text would not contain them.
        if !local.is_ascii()
            || local
                .chars()
                .any(|ch| ch == '"' || ch == '\\' || ch.is_control())
        {
            return None;
        }
        needles.push(local.to_string());
    }
    Some(needles)
}

/// The JSON text serde writes for a string, as stored in `mentions_json`.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

async fn estimate_source_rows(
    pool: &SqlitePool,
    rows: &[RowSource],
    effective: &[EffectiveSource],
    max_sorted_candidates: i64,
) -> Result<i64, sqlx::Error> {
    // UNION ALL under LIMIT stops reading a dense source early; duplicates
    // only overestimate, which at worst selects the probe plan.
    let mut sql = SqlBuilder::default();
    push_effective_cte(&mut sql, effective);
    sql.push(" SELECT COUNT(*) FROM (");
    let mut first = true;
    for row in rows {
        if !first {
            sql.push(" UNION ALL ");
        }
        first = false;
        sql.push("SELECT 1 FROM (");
        row.push_keys(&mut sql);
        sql.push(")");
    }
    if !effective.is_empty() {
        if !first {
            sql.push(" UNION ALL ");
        }
        sql.push("SELECT 1 FROM ");
        sql.push(EFFECTIVE_KEYS_CTE);
    }
    sql.push(" LIMIT ?)");
    sql.bindings
        .push(SqlPrefilterValue::Integer(max_sorted_candidates + 1));

    let mut query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.sql));
    for binding in &sql.bindings {
        query = match binding {
            SqlPrefilterValue::Text(value) => query.bind(value),
            SqlPrefilterValue::Integer(value) => query.bind(*value),
        };
    }
    query.fetch_one(pool).await
}

fn candidate_plan(rows: &[RowSource], effective: &[EffectiveSource]) -> SourceScanPlan {
    let mut sql = SqlBuilder::default();
    if effective.is_empty() {
        sql.push("WITH ");
    } else {
        push_effective_cte(&mut sql, effective);
        sql.push(", ");
    }
    sql.push("kq_source_keys(id, server_domain) AS (");
    let mut first = true;
    for row in rows {
        if !first {
            sql.push(" UNION ");
        }
        first = false;
        row.push_keys(&mut sql);
    }
    if !effective.is_empty() {
        if !first {
            sql.push(" UNION ");
        }
        sql.push("SELECT id, server_domain FROM ");
        sql.push(EFFECTIVE_KEYS_CTE);
        sql.push(" UNION SELECT rb.id, rb.server_domain FROM ");
        sql.push(EFFECTIVE_KEYS_CTE);
        sql.push(
            " e CROSS JOIN statuses rb
               ON rb.reblog_of_id = e.id AND rb.server_domain = e.server_domain",
        );
    }
    // CROSS JOIN keeps the small candidate set as the outer loop; the page is
    // then sorted into the stable status order by the caller's ORDER BY.
    sql.push(
        ") SELECT s.* FROM kq_source_keys c CROSS JOIN statuses s
           ON s.id = c.id AND s.server_domain = c.server_domain",
    );
    SourceScanPlan {
        mode: SourceScanMode::Candidates,
        select_sql: sql.sql,
        select_bindings: sql.bindings,
        predicate: None,
        predicate_bindings: Vec::new(),
    }
}

fn probe_plan(rows: &[RowSource], effective: &[EffectiveSource]) -> SourceScanPlan {
    let mut select = SqlBuilder::default();
    if !effective.is_empty() {
        push_effective_cte(&mut select, effective);
        select.push(" ");
    }
    select.push(FULL_SCAN_SELECT);

    let mut predicate = SqlBuilder::default();
    predicate.push("(");
    let mut first = true;
    for row in rows {
        if !first {
            predicate.push(" OR ");
        }
        first = false;
        row.push_probe(&mut predicate);
    }
    if !effective.is_empty() {
        if !first {
            predicate.push(" OR ");
        }
        predicate.push("(s.id, s.server_domain) IN (SELECT id, server_domain FROM ");
        predicate.push(EFFECTIVE_KEYS_CTE);
        predicate.push(
            ") OR (s.reblog_of_id IS NOT NULL
                  AND (s.reblog_of_id, s.server_domain) IN (SELECT id, server_domain FROM ",
        );
        predicate.push(EFFECTIVE_KEYS_CTE);
        predicate.push("))");
    }
    predicate.push(")");
    SourceScanPlan {
        mode: SourceScanMode::Probe,
        select_sql: select.sql,
        select_bindings: select.bindings,
        predicate: Some(predicate.sql),
        predicate_bindings: predicate.bindings,
    }
}

fn push_effective_cte(sql: &mut SqlBuilder, effective: &[EffectiveSource]) {
    if effective.is_empty() {
        return;
    }
    sql.push("WITH ");
    sql.push(EFFECTIVE_KEYS_CTE);
    sql.push("(id, server_domain) AS MATERIALIZED (");
    for (index, source) in effective.iter().enumerate() {
        if index > 0 {
            sql.push(" UNION ALL ");
        }
        source.push_keys(sql);
    }
    sql.push(")");
}

impl RowSource {
    fn push_keys(&self, sql: &mut SqlBuilder) {
        match self {
            Self::Memberships {
                timeline_types,
                account_accts,
            } => {
                sql.push(
                    "SELECT te.status_id, te.server_domain FROM timeline_entries te
                     WHERE te.timeline_type IN (SELECT value FROM json_each(?))",
                );
                sql.bind_json(timeline_types);
                if let Some(account_accts) = account_accts {
                    sql.push(" AND te.account_acct IN (SELECT value FROM json_each(?))");
                    sql.bind_json(account_accts);
                }
            }
            Self::Authors(keys) => {
                sql.push(
                    "SELECT a.id, a.server_domain FROM statuses a
                     WHERE (a.account_id, a.server_domain) IN (",
                );
                push_json_keys(sql, keys);
                sql.push(")");
            }
        }
    }

    fn push_probe(&self, sql: &mut SqlBuilder) {
        match self {
            Self::Memberships {
                timeline_types,
                account_accts,
            } => {
                sql.push(
                    "EXISTS (SELECT 1 FROM timeline_entries te
                     WHERE te.timeline_type IN (SELECT value FROM json_each(?))
                       AND te.server_domain = s.server_domain AND te.status_id = s.id",
                );
                sql.bind_json(timeline_types);
                if let Some(account_accts) = account_accts {
                    sql.push(" AND te.account_acct IN (SELECT value FROM json_each(?))");
                    sql.bind_json(account_accts);
                }
                sql.push(")");
            }
            Self::Authors(keys) => {
                sql.push("(s.account_id, s.server_domain) IN (");
                push_json_keys(sql, keys);
                sql.push(")");
            }
        }
    }
}

impl EffectiveSource {
    fn push_keys(&self, sql: &mut SqlBuilder) {
        match self {
            Self::ViewerState {
                column,
                account_accts,
            } => {
                sql.push(
                    "SELECT v.status_id, v.server_domain FROM status_viewer_state v
                     WHERE v.login_account_acct IN (SELECT value FROM json_each(?)) AND v.",
                );
                sql.bind_json(account_accts);
                // A fixed column name; the partial indexes match `= 1`.
                sql.push(column);
                sql.push(" = 1");
            }
            Self::DirectVisibility => sql
                .push("SELECT d.id, d.server_domain FROM statuses d WHERE d.visibility = 'direct'"),
            Self::RepliesTo(account_ids) => {
                sql.push(
                    "SELECT r.id, r.server_domain FROM statuses r
                     WHERE r.in_reply_to_account_id IN (SELECT value FROM json_each(?))",
                );
                sql.bind_json(account_ids);
            }
            Self::Mentions {
                id_literals,
                acct_needles,
            } => {
                sql.push(
                    "SELECT m.id, m.server_domain FROM statuses m WHERE m.mentions_json IS NOT NULL",
                );
                if let Some(acct_needles) = acct_needles {
                    // Non-ASCII JSON may match only after NFKC/case folding,
                    // so a byte search can exclude ASCII JSON alone.
                    sql.push(
                        " AND (m.mentions_json GLOB '*[^ -~]*'
                          OR EXISTS (SELECT 1 FROM json_each(?) n
                                     WHERE instr(m.mentions_json, n.value) > 0)
                          OR EXISTS (SELECT 1 FROM json_each(?) n
                                     WHERE instr(lower(m.mentions_json), n.value) > 0))",
                    );
                    sql.bind_json(id_literals);
                    sql.bind_json(acct_needles);
                }
            }
            Self::Keys(keys) => push_json_keys(sql, keys),
        }
    }
}

fn push_json_keys(sql: &mut SqlBuilder, keys: &[EntityKey]) {
    sql.push("SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]') FROM json_each(?)");
    sql.bind_json(keys);
}

/// Compiler-owned SQL text with bind-only values, in textual order.
#[derive(Debug, Default)]
struct SqlBuilder {
    sql: String,
    bindings: Vec<SqlPrefilterValue>,
}

impl SqlBuilder {
    fn push(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    fn bind_json<T: Serialize + ?Sized>(&mut self, value: &T) {
        // Serializing strings and string tuples cannot fail.
        let json = serde_json::to_string(value).unwrap_or_else(|_| "[]".to_string());
        self.bindings.push(SqlPrefilterValue::Text(json));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::Database;
    use crate::services::kq_filter::compile_query;
    use crate::services::kq_timeline::query_statuses;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio_util::sync::CancellationToken;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    async fn source_fixture() -> (Database, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-kq-source-plan-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = Database::new(directory.join("awayuki.db")).await.unwrap();
        let _ = database.run_migrations().await.unwrap();
        for statement in [
            "INSERT INTO servers(domain, streaming_url, server_kind)
             VALUES ('example.test', 'wss://example.test', 'mastodon')",
            "INSERT INTO login_accounts
               (acct, server_domain, account_id, display_name, server_kind, is_active)
             VALUES
               ('alice@example.test', 'example.test', 'alice-id', 'Alice', 'mastodon', 1),
               ('bob@example.test', 'example.test', 'bob-id', 'Bob', 'mastodon', 0)",
            "INSERT INTO accounts (id, server_domain, username, acct, display_name, created_at)
             VALUES
               ('author', 'example.test', 'author', 'author@remote.test', 'Author',
                '2026-09-01T00:00:00Z'),
               ('carol', 'example.test', 'Carol', 'Carol', 'Carol', '2026-09-01T00:00:00Z'),
               ('alice-id', 'example.test', 'alice', 'alice', 'Alice', '2026-09-01T00:00:00Z')",
            "INSERT INTO statuses
               (id, server_domain, uri, created_at, account_id, content, visibility,
                in_reply_to_account_id, reblog_of_id, mentions_json, fetched_at)
             VALUES
               ('plain', 'example.test', 'https://example.test/1', '2026-09-01T00:01:00Z',
                'author', '<p>plain</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('home-1', 'example.test', 'https://example.test/2', '2026-09-01T00:02:00Z',
                'author', '<p>home</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('bookmarked', 'example.test', 'https://example.test/3', '2026-09-01T00:03:00Z',
                'author', '<p>saved</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('boost-of-bookmarked', 'example.test', 'https://example.test/4',
                '2026-09-01T00:04:00Z', 'carol', '', 'public', NULL, 'bookmarked', NULL,
                '2026-09-01T00:00:00Z'),
               ('fav-bob', 'example.test', 'https://example.test/5', '2026-09-01T00:05:00Z',
                'author', '<p>liked</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('reply-to-alice', 'example.test', 'https://example.test/6',
                '2026-09-01T00:06:00Z', 'author', '<p>reply</p>', 'public', 'alice-id', NULL,
                NULL, '2026-09-01T00:00:00Z'),
               ('mention-alice', 'example.test', 'https://example.test/7',
                '2026-09-01T00:07:00Z', 'author', '<p>hi</p>', 'public', NULL, NULL,
                '[{\"id\":\"stale-id\",\"username\":\"Alice\",\"acct\":\"Alice\",\"url\":\"https://example.test/@alice\"}]',
                '2026-09-01T00:00:00Z'),
               ('mention-other', 'example.test', 'https://example.test/8',
                '2026-09-01T00:08:00Z', 'author', '<p>hi</p>', 'public', NULL, NULL,
                '[{\"id\":\"zed-id\",\"username\":\"zed\",\"acct\":\"zed@far.test\",\"url\":\"https://far.test/@zed\"}]',
                '2026-09-01T00:00:00Z'),
               ('dm', 'example.test', 'https://example.test/9', '2026-09-01T00:09:00Z',
                'alice-id', '<p>secret</p>', 'direct', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('tagged', 'example.test', 'https://example.test/10', '2026-09-01T00:10:00Z',
                'author', '<p>#Rust</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('listed', 'example.test', 'https://example.test/11', '2026-09-01T00:11:00Z',
                'author', '<p>list</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z'),
               ('carol-post', 'example.test', 'https://example.test/12', '2026-09-01T00:12:00Z',
                'carol', '<p>carol</p>', 'public', NULL, NULL, NULL, '2026-09-01T00:00:00Z')",
            "INSERT INTO timeline_entries
               (timeline_type, server_domain, status_id, account_acct, position_at)
             VALUES
               ('home', 'example.test', 'home-1', 'alice@example.test', '2026-09-01T00:02:00Z'),
               ('tag:Rust', 'example.test', 'tagged', 'alice@example.test', '2026-09-01T00:10:00Z'),
               ('list:42', 'example.test', 'listed', 'bob@example.test', '2026-09-01T00:11:00Z')",
            "INSERT INTO status_viewer_state
               (login_account_acct, status_id, server_domain, favourited, bookmarked)
             VALUES
               ('alice@example.test', 'bookmarked', 'example.test', 0, 1),
               ('bob@example.test', 'fav-bob', 'example.test', 1, 0)",
        ] {
            sqlx::query(statement)
                .execute(database.writer())
                .await
                .unwrap();
        }
        (database, directory)
    }

    async fn login_accounts(pool: &SqlitePool) -> Vec<LoginAccountIdentity> {
        sqlx::query_as::<_, (String, String, String, String, String, bool)>(
            "SELECT acct, server_domain, account_id, display_name, server_kind, is_active
             FROM login_accounts ORDER BY acct",
        )
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(
            |(acct, server_domain, account_id, display_name, server_kind, is_active)| {
                LoginAccountIdentity {
                    acct,
                    server_domain,
                    account_id,
                    display_name,
                    server_kind,
                    is_active,
                }
            },
        )
        .collect()
    }

    /// Rows the source plan hands to the evaluator, newest first.
    async fn planned_ids(
        pool: &SqlitePool,
        query: &str,
        conversation_keys: &[StatusKey],
        max_sorted_candidates: i64,
    ) -> (SourceScanMode, Vec<String>) {
        let compiled = compile_query(query).unwrap();
        let logins = login_accounts(pool).await;
        let plan = plan_source_scan_with_limit(
            pool,
            compiled.sources(),
            &logins,
            conversation_keys,
            max_sorted_candidates,
        )
        .await
        .unwrap();
        let sql = format!(
            "{} WHERE {} ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC",
            plan.select_sql(),
            plan.predicate().unwrap_or("1")
        );
        let mut statement = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql));
        for binding in plan
            .select_bindings()
            .iter()
            .chain(plan.predicate_bindings())
        {
            statement = match binding {
                SqlPrefilterValue::Text(value) => statement.bind(value),
                SqlPrefilterValue::Integer(value) => statement.bind(*value),
            };
        }
        let ids = statement
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>();
        (plan.mode(), ids)
    }

    async fn matched_ids(pool: &SqlitePool, query: &str) -> Vec<String> {
        query_statuses(pool, query, 100, 0, None, None, &CancellationToken::new())
            .await
            .unwrap()
            .statuses
            .into_iter()
            .map(|status| status.id)
            .collect()
    }

    #[tokio::test]
    async fn indexed_sources_match_the_evaluator_in_both_scan_modes() {
        let (database, directory) = source_fixture().await;
        let pool = database.analytics_reader();
        let cases: &[(&str, &[&str])] = &[
            ("from bookmarks", &["boost-of-bookmarked", "bookmarked"]),
            (
                "from bookmarked:\"alice@example.test\"",
                &["boost-of-bookmarked", "bookmarked"],
            ),
            ("from favourites:\"bob@example.test\"", &["fav-bob"]),
            ("from favs:\"alice@example.test\"", &[]),
            ("from home", &["home-1"]),
            ("from home:\"bob@example.test\"", &[]),
            ("from mentions", &["mention-alice", "reply-to-alice"]),
            ("from replies:\"bob@example.test\"", &[]),
            ("from dm", &["dm"]),
            ("from hashtag:\"#rust\"", &["tagged"]),
            ("from list:\"42\"", &["listed"]),
            ("from list:\"alice@example.test/42\"", &[]),
            (
                "from user:\"carol@EXAMPLE.test\"",
                &["carol-post", "boost-of-bookmarked"],
            ),
            (
                "from user:\"CAROL\"",
                &["carol-post", "boost-of-bookmarked"],
            ),
            (
                "from bookmarks, mentions, home",
                &[
                    "mention-alice",
                    "reply-to-alice",
                    "boost-of-bookmarked",
                    "bookmarked",
                    "home-1",
                ],
            ),
        ];
        for (query, expected) in cases {
            assert_eq!(&matched_ids(pool, query).await, expected, "{query}");

            let (sorted_mode, sorted) = planned_ids(pool, query, &[], MAX_SORTED_CANDIDATES).await;
            let (probe_mode, probed) = planned_ids(pool, query, &[], 0).await;
            assert_eq!(sorted, probed, "{query}: scan modes disagree");
            assert!(
                expected.iter().all(|id| sorted.iter().any(|row| row == id)),
                "{query}: {sorted:?} misses {expected:?}"
            );
            assert!(
                sorted.len() < 12,
                "{query}: source plan must not select every cached status"
            );
            // Nothing to estimate: an empty candidate set stays sorted.
            if sorted.is_empty() {
                continue;
            }
            assert_eq!(sorted_mode, SourceScanMode::Candidates, "{query}");
            assert_eq!(probe_mode, SourceScanMode::Probe, "{query}");
        }

        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn conversation_keys_expand_to_boost_wrappers() {
        let (database, directory) = source_fixture().await;
        let pool = database.analytics_reader();
        let keys = [StatusKey::new("example.test", "bookmarked")];

        let (mode, ids) = planned_ids(pool, "from conversation:\"x\"", &keys, 100).await;
        assert_eq!(mode, SourceScanMode::Candidates);
        assert_eq!(ids, vec!["boost-of-bookmarked", "bookmarked"]);

        let (mode, ids) = planned_ids(pool, "from conversation:\"x\"", &[], 100).await;
        assert_eq!(mode, SourceScanMode::Empty);
        assert!(ids.is_empty());

        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn unindexed_or_unmatched_sources_choose_full_scan_or_empty() {
        let (database, directory) = source_fixture().await;
        let pool = database.analytics_reader();

        for query in [
            "from local",
            "where ()",
            "from home, search:\"carol\"",
            "from track:\"plain\"",
        ] {
            let (mode, ids) = planned_ids(pool, query, &[], MAX_SORTED_CANDIDATES).await;
            assert_eq!(mode, SourceScanMode::FullScan, "{query}");
            assert_eq!(ids.len(), 12, "{query}");
        }
        for query in [
            "from home:\"nobody@example.test\"",
            "from hashtag:\"missing\"",
            "from user:\"nobody@nowhere.test\"",
        ] {
            let (mode, ids) = planned_ids(pool, query, &[], MAX_SORTED_CANDIDATES).await;
            assert_eq!(mode, SourceScanMode::Empty, "{query}");
            assert!(ids.is_empty(), "{query}");
            assert!(matched_ids(pool, query).await.is_empty(), "{query}");
        }

        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn mention_needles_cover_local_and_qualified_acct_mentions() {
        let account = LoginAccountIdentity {
            acct: "Alice@Example.test".to_string(),
            server_domain: "example.test".to_string(),
            account_id: "Alice-ID".to_string(),
            display_name: "Alice".to_string(),
            server_kind: "mastodon".to_string(),
            is_active: true,
        };
        assert_eq!(
            mention_acct_needles(&[&account]),
            Some(vec!["alice".to_string()])
        );
        assert_eq!(json_string(&account.account_id), "\"Alice-ID\"");

        let folded_only = LoginAccountIdentity {
            acct: "Ｅｌｉ@example.test".to_string(),
            ..account.clone()
        };
        // NFKC folds the full-width acct to ASCII, so its mentions stay
        // searchable; a non-ASCII local part disables the byte filter.
        assert_eq!(
            mention_acct_needles(&[&folded_only]),
            Some(vec!["eli".to_string()])
        );
        let non_ascii = LoginAccountIdentity {
            acct: "Élodie@example.test".to_string(),
            ..account
        };
        assert_eq!(mention_acct_needles(&[&non_ascii]), None);
    }
}
