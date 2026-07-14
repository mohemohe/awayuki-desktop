//! SQLite-backed timeline view hydration.
//!
//! This module batches related status/account reads and applies viewer state
//! without making Timeline source selection depend on Active account.

use std::collections::{HashMap, HashSet};

use sqlx::{QueryBuilder, Sqlite};

use crate::application::timeline_view::{
    db_status_to_view, notification_db_to_view, notification_type_label, parse_custom_emoji_views,
    TimelineStatus,
};
use crate::db::models::{DbAccount, DbNotification, DbStatus};
use crate::db::queries::timeline_views::TimelineStatusRef;
use crate::db::queries::{accounts, statuses as status_queries};

#[derive(Debug, Clone, sqlx::FromRow)]
struct StatusSourceAcctRef {
    server_domain: String,
    status_id: String,
    source_acct: Option<String>,
}

type StatusCacheKey = (String, String);

pub(crate) fn notification_db_to_view_with_context(
    notification: DbNotification,
    actor_account: Option<DbAccount>,
    status: Option<DbStatus>,
    status_context: &CachedStatusViewContext,
) -> TimelineStatus {
    let Some(status) = status else {
        return notification_db_to_view(notification, actor_account, None, None);
    };

    let notification_id = notification.id.clone();
    let source_acct = notification.account_acct.clone();
    let actor_account_id = notification.account_id.clone();
    let actor_label = actor_account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| actor_account_id.clone());
    let actor_acct = actor_account
        .as_ref()
        .map(|account| format!("@{}", account.acct))
        .unwrap_or_else(|| format!("@{}", actor_account_id));
    let notification_label = Some(format!(
        "{} {}",
        actor_label,
        notification_type_label(&notification.notification_type)
    ));
    let notification_avatar = actor_account.as_ref().map(|account| account.avatar.clone());
    let notification_account_emojis = actor_account
        .as_ref()
        .and_then(|account| account.emojis_json.as_deref())
        .map(parse_custom_emoji_views)
        .unwrap_or_default();
    let notification_kind = Some(notification.notification_type.clone());
    let mut view = status_context.status_to_view_resolving_reblog(status);
    view.id = notification.id;
    view.created_at = notification.created_at;
    view.source_acct = source_acct;
    view.notification_id = Some(notification_id);
    view.notification_kind = notification_kind;
    view.notification_label = notification_label;
    view.notification_avatar = notification_avatar;
    view.notification_account_id = Some(actor_account_id);
    view.notification_acct = Some(actor_acct);
    view.notification_display_name = Some(actor_label);
    view.notification_account_emojis = notification_account_emojis;
    view
}

pub(crate) async fn db_statuses_to_views(
    pool: &sqlx::SqlitePool,
    statuses: Vec<DbStatus>,
) -> Result<Vec<TimelineStatus>, String> {
    let primary_keys = status_keys_for_statuses(&statuses);
    let source_accts = query_latest_source_accts_by_status_keys(pool, &primary_keys).await?;
    let cache = CachedStatusViewContext::load(pool, &statuses).await?;
    let mut views = Vec::with_capacity(statuses.len());
    for status in statuses {
        let source_acct = source_accts
            .get(&status_key(&status.id, &status.server_domain))
            .cloned()
            .flatten();
        let view = cache.status_to_view_resolving_reblog(status);
        views.push(with_source_acct(view, source_acct));
    }
    apply_viewer_states_to_views(pool, &mut views).await?;
    Ok(views)
}

pub(crate) async fn db_status_refs_to_views(
    pool: &sqlx::SqlitePool,
    statuses: Vec<TimelineStatusRef>,
) -> Result<Vec<TimelineStatus>, String> {
    let primary_keys = status_keys_for_refs(&statuses);
    let status_cache = query_statuses_by_keys(pool, &primary_keys).await?;
    let primary_statuses = statuses
        .iter()
        .map(|status_ref| {
            status_cache
                .get(&status_key(
                    &status_ref.status_id,
                    &status_ref.server_domain,
                ))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Status {} on {} is not cached",
                        status_ref.status_id, status_ref.server_domain
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cache = CachedStatusViewContext::load(pool, &primary_statuses).await?;

    let mut views = Vec::with_capacity(statuses.len());
    for (status_ref, status) in statuses.into_iter().zip(primary_statuses) {
        let view = cache.status_to_view_resolving_reblog(status);
        views.push(with_source_acct(view, status_ref.source_acct));
    }
    apply_viewer_states_to_views(pool, &mut views).await?;
    Ok(views)
}

pub(crate) fn with_source_acct(
    mut status: TimelineStatus,
    source_acct: Option<String>,
) -> TimelineStatus {
    status.source_acct = source_acct;
    status
}

pub(crate) async fn apply_viewer_states_to_views(
    pool: &sqlx::SqlitePool,
    views: &mut [TimelineStatus],
) -> Result<(), String> {
    let mut seen = HashSet::new();
    let keys = views
        .iter()
        .filter_map(|view| {
            let acct = view.source_acct.as_ref()?.clone();
            let key = (
                acct,
                view.status_identity.remote_id.clone(),
                view.status_identity.server_domain.clone(),
            );
            seen.insert(key.clone()).then_some(key)
        })
        .collect::<Vec<_>>();
    let viewer_states = status_queries::get_viewer_states_by_keys(pool, &keys)
        .await
        .map_err(|error| error.to_string())?;
    for view in views {
        let Some(acct) = view.source_acct.as_ref() else {
            continue;
        };
        let key = (
            acct.clone(),
            view.status_identity.remote_id.clone(),
            view.status_identity.server_domain.clone(),
        );
        if let Some(viewer) = viewer_states.get(&key) {
            view.favourited = viewer.favourited.unwrap_or(false);
            view.reblogged = viewer.reblogged.unwrap_or(false);
            view.bookmarked = viewer.bookmarked.unwrap_or(false);
        }
    }
    Ok(())
}

pub(crate) struct CachedStatusViewContext {
    pub(crate) statuses: HashMap<StatusCacheKey, DbStatus>,
    pub(crate) accounts: HashMap<StatusCacheKey, DbAccount>,
}

impl CachedStatusViewContext {
    pub(crate) async fn load(
        pool: &sqlx::SqlitePool,
        statuses: &[DbStatus],
    ) -> Result<Self, String> {
        let mut statuses_by_key = HashMap::new();
        for status in statuses {
            statuses_by_key.insert(status_key_for_status(status), status.clone());
        }

        let mut related_status_keys = Vec::new();
        let mut seen_status_keys = statuses_by_key.keys().cloned().collect::<HashSet<_>>();
        for status in statuses {
            if let Some(reblog_of_id) = status.reblog_of_id.as_deref() {
                push_unique_status_key(
                    &mut related_status_keys,
                    &mut seen_status_keys,
                    reblog_of_id,
                    &status.server_domain,
                );
            }
            if let Some(quote_id) = status.quote_id.as_deref() {
                push_unique_status_key(
                    &mut related_status_keys,
                    &mut seen_status_keys,
                    quote_id,
                    &status.server_domain,
                );
            }
        }

        let related_statuses = query_statuses_by_keys(pool, &related_status_keys).await?;
        for (key, status) in related_statuses {
            statuses_by_key.entry(key).or_insert(status);
        }

        let mut original_quote_keys = Vec::new();
        let mut seen_status_keys = statuses_by_key.keys().cloned().collect::<HashSet<_>>();
        for status in statuses {
            let Some(reblog_of_id) = status.reblog_of_id.as_deref() else {
                continue;
            };
            let Some(original) =
                statuses_by_key.get(&status_key(reblog_of_id, &status.server_domain))
            else {
                continue;
            };
            let Some(quote_id) = original.quote_id.as_deref() else {
                continue;
            };
            push_unique_status_key(
                &mut original_quote_keys,
                &mut seen_status_keys,
                quote_id,
                &original.server_domain,
            );
        }

        let original_quotes = query_statuses_by_keys(pool, &original_quote_keys).await?;
        for (key, status) in original_quotes {
            statuses_by_key.entry(key).or_insert(status);
        }

        let mut account_keys = Vec::new();
        let mut seen_account_keys = HashSet::new();
        for status in statuses_by_key.values() {
            push_unique_status_key(
                &mut account_keys,
                &mut seen_account_keys,
                &status.account_id,
                &status.server_domain,
            );
        }
        let accounts = accounts::get_accounts_by_keys(pool, &account_keys)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|account| ((account.id.clone(), account.server_domain.clone()), account))
            .collect();

        Ok(Self {
            statuses: statuses_by_key,
            accounts,
        })
    }

    pub(crate) fn status_to_view_resolving_reblog(&self, status: DbStatus) -> TimelineStatus {
        let account = self.account_for_status(&status);
        let Some(reblog_of_id) = status.reblog_of_id.clone() else {
            return self.status_to_view_with_cached_quote(status, account);
        };

        let Some(original) = self
            .statuses
            .get(&status_key(&reblog_of_id, &status.server_domain))
            .cloned()
        else {
            return self.status_to_view_with_cached_quote(status, account);
        };
        let original_account = self.account_for_status(&original);
        let booster = account
            .as_ref()
            .map(|account| {
                if account.display_name.is_empty() {
                    format!("@{}", account.acct)
                } else {
                    account.display_name.clone()
                }
            })
            .unwrap_or_else(|| status.account_id.clone());
        let booster_avatar = account.as_ref().map(|account| account.avatar.clone());
        let top_level_uri = status.uri.clone();
        let mut view = self.status_to_view_with_cached_quote(original, original_account);
        view.id = status.id;
        view.uri = top_level_uri;
        view.original_created_at = Some(view.created_at.clone());
        view.created_at = status.created_at;
        view.notification_label = Some(format!("{} boosted", booster));
        view.notification_kind = Some("reblog".to_string());
        view.notification_avatar = booster_avatar;
        view.notification_account_id = Some(status.account_id.clone());
        view.notification_acct = Some(
            account
                .as_ref()
                .map(|account| format!("@{}", account.acct))
                .unwrap_or_else(|| format!("@{}", status.account_id)),
        );
        view.notification_display_name = Some(booster);
        view.notification_account_emojis = account
            .as_ref()
            .and_then(|account| account.emojis_json.as_deref())
            .map(parse_custom_emoji_views)
            .unwrap_or_default();
        view
    }

    fn status_to_view_with_cached_quote(
        &self,
        status: DbStatus,
        account: Option<DbAccount>,
    ) -> TimelineStatus {
        let quote_id = status.quote_id.clone();
        let server_domain = status.server_domain.clone();
        let mut view = db_status_to_view(status, account);
        let Some(quote_id) = quote_id else {
            return view;
        };
        let Some(quote) = self
            .statuses
            .get(&status_key(&quote_id, &server_domain))
            .cloned()
        else {
            return view;
        };
        let quote_account = self.account_for_status(&quote);
        view.quote = Some(Box::new(db_status_to_view(quote, quote_account)));
        view.quote_state = Some("resolved".to_string());
        view
    }

    fn account_for_status(&self, status: &DbStatus) -> Option<DbAccount> {
        self.accounts
            .get(&status_key(&status.account_id, &status.server_domain))
            .cloned()
    }
}

pub(crate) fn status_key(id: &str, server_domain: &str) -> StatusCacheKey {
    (id.to_string(), server_domain.to_string())
}

fn status_key_for_status(status: &DbStatus) -> StatusCacheKey {
    status_key(&status.id, &status.server_domain)
}

fn status_keys_for_statuses(statuses: &[DbStatus]) -> Vec<StatusCacheKey> {
    let mut keys = Vec::with_capacity(statuses.len());
    let mut seen = HashSet::new();
    for status in statuses {
        push_unique_status_key(&mut keys, &mut seen, &status.id, &status.server_domain);
    }
    keys
}

fn status_keys_for_refs(statuses: &[TimelineStatusRef]) -> Vec<StatusCacheKey> {
    let mut keys = Vec::with_capacity(statuses.len());
    let mut seen = HashSet::new();
    for status in statuses {
        push_unique_status_key(
            &mut keys,
            &mut seen,
            &status.status_id,
            &status.server_domain,
        );
    }
    keys
}

fn push_unique_status_key(
    keys: &mut Vec<StatusCacheKey>,
    seen: &mut HashSet<StatusCacheKey>,
    id: &str,
    server_domain: &str,
) {
    let key = status_key(id, server_domain);
    if seen.insert(key.clone()) {
        keys.push(key);
    }
}

async fn query_statuses_by_keys(
    pool: &sqlx::SqlitePool,
    keys: &[StatusCacheKey],
) -> Result<HashMap<StatusCacheKey, DbStatus>, String> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM statuses WHERE ");
    push_status_key_predicates(&mut builder, keys, "id", "server_domain");

    builder
        .build_query_as::<DbStatus>()
        .fetch_all(pool)
        .await
        .map(|statuses| {
            statuses
                .into_iter()
                .map(|status| (status_key_for_status(&status), status))
                .collect()
        })
        .map_err(|error| error.to_string())
}

async fn query_latest_source_accts_by_status_keys(
    pool: &sqlx::SqlitePool,
    keys: &[StatusCacheKey],
) -> Result<HashMap<StatusCacheKey, Option<String>>, String> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT server_domain, status_id, account_acct AS source_acct,
                  ROW_NUMBER() OVER (
                    PARTITION BY server_domain, status_id
                    ORDER BY position_at DESC
                  ) AS source_rank
           FROM timeline_entries
           WHERE ",
    );
    push_status_key_predicates(&mut builder, keys, "status_id", "server_domain");
    builder.push(") ranked WHERE source_rank = 1");

    builder
        .build_query_as::<StatusSourceAcctRef>()
        .fetch_all(pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    (
                        status_key(&row.status_id, &row.server_domain),
                        row.source_acct,
                    )
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

fn push_status_key_predicates<'args>(
    builder: &mut QueryBuilder<'args, Sqlite>,
    keys: &'args [StatusCacheKey],
    id_column: &str,
    server_domain_column: &str,
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
            .push(server_domain_column)
            .push(" = ")
            .push_bind(server_domain)
            .push(")");
    }
}
