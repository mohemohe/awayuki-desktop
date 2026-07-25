use std::collections::{HashMap, HashSet};

use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use crate::db::models::{DbAccount, DbNotification, DbStatus};

pub type EntityKey = (String, String);

#[derive(Debug)]
pub struct NotificationPageContext {
    pub notifications: Vec<DbNotification>,
    pub statuses: HashMap<EntityKey, DbStatus>,
    pub accounts: HashMap<EntityKey, DbAccount>,
    /// Number of SQL statements actually executed to construct this page.
    pub statement_count: usize,
}

/// Load notification primaries, actors and status authors in a fixed number of
/// statements. Related reblogs/quotes are hydrated by the shared bulk status
/// view context, never by a per-notification query.
pub async fn load_notification_page_context(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<NotificationPageContext, sqlx::Error> {
    let notifications = sqlx::query_as::<_, DbNotification>(
        "SELECT * FROM notifications
         ORDER BY created_at DESC, server_domain DESC, id DESC, account_acct DESC
         LIMIT ? OFFSET ?",
    )
    .bind(limit.max(0))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?;
    let mut statement_count = 1;

    let mut status_keys = Vec::new();
    let mut seen_statuses = HashSet::new();
    for notification in &notifications {
        let Some(status_id) = notification.status_id.as_deref() else {
            continue;
        };
        let key = (status_id.to_string(), notification.server_domain.clone());
        if seen_statuses.insert(key.clone()) {
            status_keys.push(key);
        }
    }
    let statuses = if status_keys.is_empty() {
        HashMap::new()
    } else {
        statement_count += 1;
        load_statuses_by_keys(pool, &status_keys).await?
    };

    let mut account_keys = Vec::new();
    let mut seen_accounts = HashSet::new();
    for notification in &notifications {
        let key = (
            notification.account_id.clone(),
            notification.server_domain.clone(),
        );
        if seen_accounts.insert(key.clone()) {
            account_keys.push(key);
        }
    }
    for status in statuses.values() {
        let key = (status.account_id.clone(), status.server_domain.clone());
        if seen_accounts.insert(key.clone()) {
            account_keys.push(key);
        }
    }
    let accounts = if account_keys.is_empty() {
        HashMap::new()
    } else {
        statement_count += 1;
        load_accounts_by_keys(pool, &account_keys).await?
    };

    Ok(NotificationPageContext {
        notifications,
        statuses,
        accounts,
        statement_count,
    })
}

async fn load_statuses_by_keys(
    pool: &SqlitePool,
    keys: &[EntityKey],
) -> Result<HashMap<EntityKey, DbStatus>, sqlx::Error> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM statuses WHERE ");
    push_entity_predicates(&mut builder, keys, "id", "server_domain");
    let rows = builder.build_query_as::<DbStatus>().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|row| ((row.id.clone(), row.server_domain.clone()), row))
        .collect())
}

async fn load_accounts_by_keys(
    pool: &SqlitePool,
    keys: &[EntityKey],
) -> Result<HashMap<EntityKey, DbAccount>, sqlx::Error> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM accounts WHERE ");
    push_entity_predicates(&mut builder, keys, "id", "server_domain");
    let rows = builder
        .build_query_as::<DbAccount>()
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| ((row.id.clone(), row.server_domain.clone()), row))
        .collect())
}

fn push_entity_predicates(
    builder: &mut QueryBuilder<Sqlite>,
    keys: &[EntityKey],
    id_column: &str,
    server_column: &str,
) {
    for (index, (id, server_domain)) in keys.iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder
            .push("(")
            .push(id_column)
            .push(" = ")
            .push_bind(id)
            .push(" AND ")
            .push(server_column)
            .push(" = ")
            .push_bind(server_domain)
            .push(")");
    }
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct AggregateStatusRef {
    pub server_domain: String,
    pub status_id: String,
    pub source_acct: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AggregateFilter {
    pub exclude_boosts: bool,
    pub exclude_media: bool,
    pub include_media: bool,
}

pub async fn query_aggregate_status_refs(
    pool: &SqlitePool,
    timeline_type: &str,
    preferred_account_acct: Option<&str>,
    limit: i64,
    offset: i64,
    filter: AggregateFilter,
) -> Result<Vec<AggregateStatusRef>, sqlx::Error> {
    let page_limit = limit.clamp(0, 500);
    if page_limit == 0 {
        return Ok(Vec::new());
    }
    let page_offset = offset.max(0);
    let candidate_limit = (page_limit + page_offset)
        .saturating_mul(8)
        .clamp(128, 8_192);
    let sql = aggregate_query_sql(filter, false);
    // Every interpolated fragment is selected from AggregateFilter; all
    // timeline/account values remain bind parameters.
    sqlx::query_as::<_, AggregateStatusRef>(sqlx::AssertSqlSafe(sql))
        .bind(preferred_account_acct)
        .bind(timeline_type)
        .bind(candidate_limit)
        .bind(timeline_type)
        .bind(preferred_account_acct)
        .bind(page_limit)
        .bind(page_offset)
        .fetch_all(pool)
        .await
}

fn aggregate_query_sql(filter: AggregateFilter, explain: bool) -> String {
    let mut predicate = String::new();
    if filter.exclude_boosts {
        predicate.push_str(" AND s.reblog_of_id IS NULL");
    }
    let has_media = "((s.media_attachments_json IS NOT NULL AND s.media_attachments_json != '[]')
        OR EXISTS (
          SELECT 1 FROM statuses original
           WHERE original.id = s.reblog_of_id
             AND original.server_domain = s.server_domain
             AND original.media_attachments_json IS NOT NULL
             AND original.media_attachments_json != '[]'
        ))";
    if filter.exclude_media {
        predicate.push_str(" AND NOT ");
        predicate.push_str(has_media);
    }
    if filter.include_media {
        predicate.push_str(" AND ");
        predicate.push_str(has_media);
    }
    let explain = if explain { "EXPLAIN QUERY PLAN " } else { "" };
    format!(
        "{explain}WITH recent_candidates AS MATERIALIZED (
           SELECT te.server_domain,
                  te.status_id,
                  te.account_acct AS source_acct,
                  CASE WHEN te.account_acct = ? THEN 1 ELSE 0 END AS account_rank,
                  te.position_at AS latest_position,
                  COALESCE(
                    NULLIF(identity.canonical_uri, ''),
                    NULLIF(s.uri, ''),
                    te.server_domain || ':' || te.status_id
                  ) AS canonical_uri
             FROM timeline_entries te
             JOIN statuses s
               ON s.id = te.status_id AND s.server_domain = te.server_domain
             LEFT JOIN status_identities identity
               ON identity.status_id = te.status_id
              AND identity.server_domain = te.server_domain
            WHERE te.timeline_type = ? {predicate}
            ORDER BY te.position_at DESC,
                     te.server_domain DESC,
                     te.status_id DESC,
                     te.account_acct DESC
            LIMIT ?
         ), preferred_candidates AS (
           SELECT te.server_domain,
                  te.status_id,
                  te.account_acct AS source_acct,
                  1 AS account_rank,
                  te.position_at AS latest_position,
                  wanted.canonical_uri
             -- The CROSS JOIN order is intentional. On large databases SQLite
             -- otherwise drives this branch from every timeline row, then scans
             -- the bounded URI set for each row (minutes instead of milliseconds).
             FROM (SELECT DISTINCT canonical_uri FROM recent_candidates) wanted
             CROSS JOIN status_identities identity
             CROSS JOIN timeline_entries te
             JOIN statuses s
               ON s.id = te.status_id AND s.server_domain = te.server_domain
            WHERE identity.canonical_uri = wanted.canonical_uri
              AND te.server_domain = identity.server_domain
              AND te.status_id = identity.status_id
              AND te.timeline_type = ?
              AND te.account_acct = ? {predicate}
         ), candidate_entries AS (
           SELECT * FROM recent_candidates
           UNION ALL
           SELECT * FROM preferred_candidates
         ), ranked AS (
           SELECT *,
                  ROW_NUMBER() OVER (
                    PARTITION BY canonical_uri
                    ORDER BY account_rank DESC,
                             latest_position DESC,
                             server_domain DESC,
                             status_id DESC,
                             source_acct DESC
                  ) AS canonical_rank,
                  MAX(latest_position) OVER (
                    PARTITION BY canonical_uri
                  ) AS canonical_latest_position
             FROM candidate_entries
         )
         SELECT server_domain, status_id, source_acct
           FROM ranked
          WHERE canonical_rank = 1
          ORDER BY canonical_latest_position DESC, server_domain DESC, status_id DESC
          LIMIT ? OFFSET ?"
    )
}

#[cfg(test)]
pub async fn explain_aggregate_query_plan(
    pool: &SqlitePool,
    timeline_type: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let sql = aggregate_query_sql(AggregateFilter::default(), true);
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(Option::<&str>::None)
        .bind(timeline_type)
        .bind(512_i64)
        .bind(timeline_type)
        .bind(Option::<&str>::None)
        .bind(50_i64)
        .bind(0_i64)
        .fetch_all(pool)
        .await?;
    use sqlx::Row;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("detail").ok())
        .collect())
}

#[derive(Debug)]
pub struct ThreadStatusPage {
    pub statuses: Vec<DbStatus>,
    pub root_id: String,
    pub statement_count: usize,
}

const THREAD_STATUS_PAGE_SQL: &str = "WITH RECURSIVE
     ancestors(id, server_domain, in_reply_to_id, depth, path) AS (
       SELECT id, server_domain, in_reply_to_id, 0,
              char(31) || id || char(31)
         FROM statuses
        WHERE id = ? AND server_domain = ?
       UNION ALL
       SELECT parent.id, parent.server_domain, parent.in_reply_to_id,
              ancestors.depth + 1,
              ancestors.path || parent.id || char(31)
         FROM ancestors
         JOIN statuses parent
           ON parent.id = ancestors.in_reply_to_id
          AND parent.server_domain = ancestors.server_domain
        WHERE ancestors.depth < ?
          AND instr(ancestors.path, char(31) || parent.id || char(31)) = 0
     ),
     root(id, server_domain) AS (
       SELECT id, server_domain FROM ancestors ORDER BY depth DESC LIMIT 1
     ),
     descendants(id, server_domain, depth, path) AS (
       SELECT id, server_domain, 0, char(31) || id || char(31) FROM root
       UNION ALL
       SELECT child.id, child.server_domain, descendants.depth + 1,
              descendants.path || child.id || char(31)
         FROM descendants
         JOIN statuses child
           ON child.in_reply_to_id = descendants.id
          AND child.server_domain = descendants.server_domain
        WHERE descendants.depth < ?
          AND instr(descendants.path, char(31) || child.id || char(31)) = 0
     ),
     selected(id, server_domain) AS (
       SELECT id, server_domain FROM ancestors
       UNION
       SELECT id, server_domain FROM descendants
     )
     SELECT statuses.*
       FROM selected
       -- Keep the bounded CTE outermost; a reorder scans the entire status cache.
       CROSS JOIN statuses
         ON statuses.id = selected.id
        AND statuses.server_domain = selected.server_domain
      LIMIT ?";

pub async fn query_thread_status_page(
    pool: &SqlitePool,
    status_id: &str,
    server_domain: &str,
    limit: usize,
) -> Result<Option<ThreadStatusPage>, sqlx::Error> {
    let limit = limit.clamp(1, 500) as i64;
    let statuses = sqlx::query_as::<_, DbStatus>(THREAD_STATUS_PAGE_SQL)
        .bind(status_id)
        .bind(server_domain)
        .bind(limit)
        .bind(limit)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    if statuses.is_empty() {
        return Ok(None);
    }

    let by_id = statuses
        .iter()
        .map(|status| (status.id.as_str(), status))
        .collect::<HashMap<_, _>>();
    let mut root_id = status_id.to_string();
    let mut seen = HashSet::new();
    while seen.insert(root_id.clone()) {
        let Some(parent_id) = by_id
            .get(root_id.as_str())
            .and_then(|status| status.in_reply_to_id.as_deref())
        else {
            break;
        };
        if !by_id.contains_key(parent_id) {
            break;
        }
        root_id = parent_id.to_string();
    }

    Ok(Some(ThreadStatusPage {
        statuses,
        root_id,
        statement_count: 1,
    }))
}

pub async fn cache_counter(pool: &SqlitePool, name: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT value FROM cache_counters WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::Database;
    use sqlx::Row;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    async fn database(label: &str) -> (Database, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-read-model-{label}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = Database::new(directory.join("awayuki.db")).await.unwrap();
        let _ = database.run_migrations().await.unwrap();
        sqlx::query(
            "INSERT INTO servers(domain, streaming_url, server_kind)
             VALUES ('example.test', 'wss://example.test', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        (database, directory)
    }

    async fn seed_account(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO accounts
             (id, server_domain, username, acct, display_name, note, avatar,
              avatar_static, header, locked, bot, followers_count,
              following_count, statuses_count, created_at, fetched_at)
             VALUES (?, 'example.test', ?, ?, ?, '', '', '', '', 0, 0, 0, 0, 0,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_status(
        pool: &SqlitePool,
        id: &str,
        account_id: &str,
        parent_id: Option<&str>,
        created_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO statuses
             (id, server_domain, uri, created_at, account_id, content,
              in_reply_to_id, fetched_at)
             VALUES (?, 'example.test', ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("https://example.test/@{account_id}/{id}"))
        .bind(created_at)
        .bind(account_id)
        .bind(id)
        .bind(parent_id)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn thread_uses_one_recursive_statement_regardless_of_depth() {
        let (database, directory) = database("thread").await;
        seed_account(database.writer(), "author").await;
        for index in 0..40 {
            let id = format!("s{index:03}");
            let parent = (index > 0).then(|| format!("s{:03}", index - 1));
            seed_status(
                database.writer(),
                &id,
                "author",
                parent.as_deref(),
                &format!("2026-01-01T00:{index:02}:00Z"),
            )
            .await;
        }
        let page = query_thread_status_page(database.reader(), "s039", "example.test", 100)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.statement_count, 1);
        assert_eq!(page.statuses.len(), 40);
        assert_eq!(page.root_id, "s000");

        let explain_sql = format!("EXPLAIN QUERY PLAN {THREAD_STATUS_PAGE_SQL}");
        let plan = sqlx::query(sqlx::AssertSqlSafe(explain_sql))
            .bind("s039")
            .bind("example.test")
            .bind(100_i64)
            .bind(100_i64)
            .bind(100_i64)
            .fetch_all(database.reader())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>();
        assert!(
            plan.iter().any(|detail| {
                detail.contains("SEARCH statuses USING INDEX sqlite_autoindex_statuses_1")
            }),
            "thread result hydration must use a primary-key lookup: {plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|detail| detail.starts_with("SCAN statuses")),
            "thread result hydration must not scan the complete status cache: {plan:?}"
        );
        database.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn counters_follow_writes_without_full_count_queries() {
        let (database, directory) = database("counter").await;
        seed_account(database.writer(), "author").await;
        seed_status(
            database.writer(),
            "one",
            "author",
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        assert_eq!(
            cache_counter(database.reader(), "accounts").await.unwrap(),
            1
        );
        assert_eq!(
            cache_counter(database.reader(), "statuses").await.unwrap(),
            1
        );
        sqlx::query("DELETE FROM statuses WHERE id = 'one'")
            .execute(database.writer())
            .await
            .unwrap();
        assert_eq!(
            cache_counter(database.reader(), "statuses").await.unwrap(),
            0
        );
        database.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn notification_statement_count_does_not_grow_with_page_size() {
        let (database, directory) = database("notifications").await;
        seed_account(database.writer(), "viewer").await;
        sqlx::query(
            "INSERT INTO login_accounts
             (acct, server_domain, account_id, display_name, avatar, is_active,
              access_token, server_kind)
             VALUES ('viewer@example.test', 'example.test', 'viewer', 'viewer', '', 1,
                     'token', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        seed_status(
            database.writer(),
            "target",
            "viewer",
            None,
            "2026-01-01T00:00:00Z",
        )
        .await;
        for index in 0..80 {
            sqlx::query(
                "INSERT INTO notifications
                 (id, server_domain, account_acct, notification_type, created_at,
                  account_id, status_id, fetched_at)
                 VALUES (?, 'example.test', 'viewer@example.test', 'favourite', ?,
                         'viewer', 'target', ?)",
            )
            .bind(format!("notification-{index:03}"))
            .bind(format!("2026-01-01T00:{:02}:00Z", index % 60))
            .bind("2026-01-01T01:00:00Z")
            .execute(database.writer())
            .await
            .unwrap();
        }
        let small = load_notification_page_context(database.reader(), 1, 0)
            .await
            .unwrap();
        let large = load_notification_page_context(database.reader(), 80, 0)
            .await
            .unwrap();
        assert_eq!(small.statement_count, 3);
        assert_eq!(large.statement_count, 3);
        assert_eq!(large.notifications.len(), 80);

        database.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn aggregate_is_limit_first_and_uses_covering_index() {
        let (database, directory) = database("aggregate").await;
        seed_account(database.writer(), "author").await;
        sqlx::query(
            "INSERT INTO login_accounts
             (acct, server_domain, account_id, display_name, avatar, is_active,
              access_token, server_kind)
             VALUES ('viewer@example.test', 'example.test', 'author', 'viewer', '', 1,
                     'token', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        for index in 0..200 {
            let id = format!("s{index:04}");
            let timestamp = format!("2026-01-01T{:02}:{:02}:00Z", index / 60, index % 60);
            seed_status(database.writer(), &id, "author", None, &timestamp).await;
            sqlx::query(
                "INSERT INTO timeline_entries
                 (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', 'example.test', ?, 'viewer@example.test', ?)",
            )
            .bind(&id)
            .bind(&timestamp)
            .execute(database.writer())
            .await
            .unwrap();
        }
        let page = query_aggregate_status_refs(
            database.reader(),
            "home",
            Some("viewer@example.test"),
            20,
            0,
            AggregateFilter::default(),
        )
        .await
        .unwrap();
        assert_eq!(page.len(), 20);
        let plan = explain_aggregate_query_plan(database.reader(), "home")
            .await
            .unwrap()
            .join("\n");
        assert!(
            plan.contains("idx_timeline_entries_aggregate_page"),
            "{plan}"
        );
        assert_eq!(
            plan.matches("idx_timeline_entries_aggregate_page").count(),
            1,
            "the aggregate-page index must only drive the bounded recent branch:\n{plan}"
        );
        assert!(
            plan.contains("idx_status_identities_canonical_cover (canonical_uri=?)"),
            "{plan}"
        );
        let wanted_scan = plan
            .find("SCAN wanted")
            .expect("wanted URI scan in query plan");
        let identity_lookup = plan
            .find("idx_status_identities_canonical_cover (canonical_uri=?)")
            .expect("canonical identity lookup in query plan");
        assert!(
            wanted_scan < identity_lookup,
            "the bounded URI set must drive the preferred-account lookup:\n{plan}"
        );
        let sql = aggregate_query_sql(AggregateFilter::default(), false);
        assert!(
            sql.contains(
                "FROM (SELECT DISTINCT canonical_uri FROM recent_candidates) wanted\n             CROSS JOIN status_identities identity\n             CROSS JOIN timeline_entries te"
            ),
            "preferred-account lookup must keep the bounded URI set as the outer loop"
        );
        database.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }
}
