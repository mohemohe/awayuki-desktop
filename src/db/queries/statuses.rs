use std::collections::HashMap;

use sqlx::{QueryBuilder, Sqlite, SqliteConnection, SqlitePool};

use crate::db::models::{DbStatus, DbStatusViewerState};
use crate::domain::identity::StatusIdentity;

pub type ViewerStateKey = (String, String, String);
pub type ViewerIdentityKey = (String, String);

/// Transaction-friendly variant used by status page/event batches.
pub async fn upsert_status_on(
    connection: &mut SqliteConnection,
    status: &DbStatus,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO statuses (id, server_domain, uri, url, created_at, edited_at, account_id, content, visibility, sensitive, spoiler_text, reblogs_count, favourites_count, replies_count, in_reply_to_id, in_reply_to_account_id, reblog_of_id, language, pinned, favourited, reblogged, muted, bookmarked, poll_json, card_json, application_json, mentions_json, tags_json, emojis_json, media_attachments_json, fetched_at, quote_id, quote_original_url)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id, server_domain) DO UPDATE SET
           uri = excluded.uri,
           url = excluded.url,
           edited_at = excluded.edited_at,
           content = excluded.content,
           visibility = excluded.visibility,
           sensitive = excluded.sensitive,
           spoiler_text = excluded.spoiler_text,
           reblogs_count = excluded.reblogs_count,
           favourites_count = excluded.favourites_count,
           replies_count = excluded.replies_count,
           in_reply_to_id = excluded.in_reply_to_id,
           in_reply_to_account_id = excluded.in_reply_to_account_id,
           reblog_of_id = excluded.reblog_of_id,
           language = excluded.language,
           pinned = excluded.pinned,
           poll_json = excluded.poll_json,
           card_json = excluded.card_json,
           application_json = excluded.application_json,
           mentions_json = excluded.mentions_json,
           tags_json = excluded.tags_json,
           emojis_json = excluded.emojis_json,
           media_attachments_json = excluded.media_attachments_json,
           fetched_at = excluded.fetched_at,
           quote_id = excluded.quote_id,
           quote_original_url = excluded.quote_original_url"
    )
    .bind(&status.id)
    .bind(&status.server_domain)
    .bind(&status.uri)
    .bind(&status.url)
    .bind(&status.created_at)
    .bind(&status.edited_at)
    .bind(&status.account_id)
    .bind(&status.content)
    .bind(&status.visibility)
    .bind(status.sensitive)
    .bind(&status.spoiler_text)
    .bind(status.reblogs_count)
    .bind(status.favourites_count)
    .bind(status.replies_count)
    .bind(&status.in_reply_to_id)
    .bind(&status.in_reply_to_account_id)
    .bind(&status.reblog_of_id)
    .bind(&status.language)
    .bind(status.pinned)
    // Viewer-dependent values are stored in status_viewer_state. Keep the
    // legacy columns NULL so one account can never overwrite another.
    .bind(Option::<bool>::None)
    .bind(Option::<bool>::None)
    .bind(Option::<bool>::None)
    .bind(Option::<bool>::None)
    .bind(&status.poll_json)
    .bind(&status.card_json)
    .bind(&status.application_json)
    .bind(&status.mentions_json)
    .bind(&status.tags_json)
    .bind(&status.emojis_json)
    .bind(&status.media_attachments_json)
    .bind(&status.fetched_at)
    .bind(&status.quote_id)
    .bind(&status.quote_original_url)
    .execute(&mut *connection)
    .await?;

    let identity = StatusIdentity::inferred(&status.server_domain, &status.uri, &status.id);
    sqlx::query(
        "INSERT INTO status_identities
           (status_id, server_domain, protocol, canonical_uri, remote_id)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(status_id, server_domain) DO UPDATE SET
           protocol = excluded.protocol,
           canonical_uri = excluded.canonical_uri,
           remote_id = excluded.remote_id",
    )
    .bind(&status.id)
    .bind(&status.server_domain)
    .bind(identity.protocol.as_db_str())
    .bind(&identity.canonical_uri)
    .bind(&identity.remote_id)
    .execute(connection)
    .await?;

    Ok(())
}

pub async fn upsert_status_viewer_state_on(
    connection: &mut SqliteConnection,
    status: &DbStatus,
    login_account_acct: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO status_viewer_state
           (login_account_acct, status_id, server_domain,
            favourited, reblogged, muted, bookmarked, pinned, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(login_account_acct, status_id, server_domain) DO UPDATE SET
           favourited = excluded.favourited,
           reblogged = excluded.reblogged,
           muted = excluded.muted,
           bookmarked = excluded.bookmarked,
           pinned = excluded.pinned,
           updated_at = excluded.updated_at",
    )
    .bind(login_account_acct)
    .bind(&status.id)
    .bind(&status.server_domain)
    .bind(status.favourited)
    .bind(status.reblogged)
    .bind(status.muted)
    .bind(status.bookmarked)
    .bind(status.pinned)
    .bind(&status.fetched_at)
    .execute(connection)
    .await?;
    Ok(())
}

pub async fn replace_status_tags_on(
    connection: &mut SqliteConnection,
    status_id: &str,
    server_domain: &str,
    tag_names: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM status_tags WHERE status_id = ? AND server_domain = ?")
        .bind(status_id)
        .bind(server_domain)
        .execute(&mut *connection)
        .await?;
    for tag_name in tag_names {
        sqlx::query(
            "INSERT INTO tags (name, server_domain) VALUES (?, ?)
             ON CONFLICT(name, server_domain) DO NOTHING",
        )
        .bind(tag_name)
        .bind(server_domain)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO status_tags
               (status_id, server_domain, tag_name) VALUES (?, ?, ?)",
        )
        .bind(status_id)
        .bind(server_domain)
        .bind(tag_name)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

pub async fn get_viewer_states_by_keys(
    pool: &SqlitePool,
    keys: &[ViewerStateKey],
) -> Result<HashMap<ViewerStateKey, DbStatusViewerState>, sqlx::Error> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    // Each key consumes three bind parameters. Stay below SQLite builds that
    // retain the historical 999-variable limit while supporting a full
    // bounded frontend entity set.
    const KEYS_PER_QUERY: usize = 250;
    let mut states = HashMap::new();
    for chunk in keys.chunks(KEYS_PER_QUERY) {
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM status_viewer_state WHERE ");
        for (index, (acct, status_id, server_domain)) in chunk.iter().enumerate() {
            if index > 0 {
                builder.push(" OR ");
            }
            builder
                .push("(login_account_acct = ")
                .push_bind(acct)
                .push(" AND status_id = ")
                .push_bind(status_id)
                .push(" AND server_domain = ")
                .push_bind(server_domain)
                .push(")");
        }
        for state in builder
            .build_query_as::<DbStatusViewerState>()
            .fetch_all(pool)
            .await?
        {
            states.insert(
                (
                    state.login_account_acct.clone(),
                    state.status_id.clone(),
                    state.server_domain.clone(),
                ),
                state,
            );
        }
    }
    Ok(states)
}

pub async fn get_viewer_states_for_identities(
    pool: &SqlitePool,
    acting_account_acct: &str,
    identities: &[ViewerIdentityKey],
) -> Result<HashMap<ViewerIdentityKey, DbStatusViewerState>, sqlx::Error> {
    if identities.is_empty() {
        return Ok(HashMap::new());
    }
    // Two identity parameters per item plus the actor. This remains portable
    // to SQLite builds with the historical 999-variable limit.
    const IDENTITIES_PER_QUERY: usize = 400;
    let mut states = HashMap::new();
    for chunk in identities.chunks(IDENTITIES_PER_QUERY) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT v.*, i.protocol AS identity_protocol, i.canonical_uri AS identity_uri
             FROM status_viewer_state v
             JOIN status_identities i
               ON i.status_id = v.status_id AND i.server_domain = v.server_domain
             WHERE v.login_account_acct = ",
        );
        builder.push_bind(acting_account_acct).push(" AND (");
        for (index, (protocol, canonical_uri)) in chunk.iter().enumerate() {
            if index > 0 {
                builder.push(" OR ");
            }
            builder
                .push("(i.protocol = ")
                .push_bind(protocol)
                .push(" AND i.canonical_uri = ")
                .push_bind(canonical_uri)
                .push(")");
        }
        builder.push(") ORDER BY v.updated_at ASC");
        let rows = builder.build().fetch_all(pool).await?;
        use sqlx::Row;
        for row in rows {
            let state = DbStatusViewerState {
                login_account_acct: row.try_get("login_account_acct")?,
                status_id: row.try_get("status_id")?,
                server_domain: row.try_get("server_domain")?,
                favourited: row.try_get("favourited")?,
                reblogged: row.try_get("reblogged")?,
                muted: row.try_get("muted")?,
                bookmarked: row.try_get("bookmarked")?,
                pinned: row.try_get("pinned")?,
                updated_at: row.try_get("updated_at")?,
            };
            states.insert(
                (
                    row.try_get::<String, _>("identity_protocol")?,
                    row.try_get::<String, _>("identity_uri")?,
                ),
                state,
            );
        }
    }
    Ok(states)
}

pub async fn delete_status_and_references(
    pool: &SqlitePool,
    id: &str,
    server_domain: &str,
) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    // Keep both predicates independently indexable. Combining the primary-key
    // and reblog lookup with OR made SQLite choose the broad domain/created
    // index on large databases and scan every status from the server while
    // holding Awayuki's only writer connection.
    let reblogs = sqlx::query(
        "DELETE FROM statuses
         WHERE reblog_of_id = ? AND server_domain = ?",
    )
    .bind(id)
    .bind(server_domain)
    .execute(&mut *transaction)
    .await?;
    let original = sqlx::query("DELETE FROM statuses WHERE id = ? AND server_domain = ?")
        .bind(id)
        .bind(server_domain)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(reblogs
        .rows_affected()
        .saturating_add(original.rows_affected()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::Database;

    fn fixture_status() -> DbStatus {
        DbStatus {
            id: "same-id".to_string(),
            server_domain: "example.test".to_string(),
            uri: "https://example.test/@author/same-id".to_string(),
            url: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            edited_at: None,
            account_id: "author-id".to_string(),
            content: "<p>fixture</p>".to_string(),
            visibility: "public".to_string(),
            sensitive: false,
            spoiler_text: String::new(),
            reblogs_count: 0,
            favourites_count: 0,
            replies_count: 0,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            reblog_of_id: None,
            language: None,
            pinned: None,
            favourited: Some(true),
            reblogged: Some(false),
            muted: Some(false),
            bookmarked: Some(false),
            poll_json: None,
            card_json: None,
            application_json: None,
            mentions_json: None,
            tags_json: Some(r#"[{"name":"rust"}]"#.to_string()),
            emojis_json: None,
            media_attachments_json: None,
            fetched_at: "2026-01-01T00:00:00Z".to_string(),
            quote_id: None,
            quote_original_url: None,
        }
    }

    async fn migrated_database() -> (Database, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "awayuki-data-identity-{}.sqlite3",
            uuid::Uuid::new_v4().simple()
        ));
        let database = Database::new(&path).await.expect("open fixture database");
        let _ = database.run_migrations().await.expect("run migrations");
        sqlx::query(
            "INSERT INTO servers (domain, streaming_url, server_kind)
             VALUES ('example.test', 'wss://example.test', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .expect("insert server");
        sqlx::query(
            "INSERT INTO accounts
               (id, server_domain, username, acct, display_name, note, avatar,
                avatar_static, header, locked, bot, followers_count,
                following_count, statuses_count, created_at)
             VALUES
               ('author-id', 'example.test', 'author', 'author@example.test',
                'Author', '', '', '', '', 0, 0, 0, 0, 0,
                '2026-01-01T00:00:00Z')",
        )
        .execute(database.writer())
        .await
        .expect("insert author");
        for (acct, active) in [("alice@example.test", true), ("bob@example.test", false)] {
            sqlx::query(
                "INSERT INTO login_accounts
                   (acct, server_domain, account_id, display_name, is_active)
                 VALUES (?, 'example.test', ?, ?, ?)",
            )
            .bind(acct)
            .bind(format!("{acct}-id"))
            .bind(acct)
            .bind(active)
            .execute(database.writer())
            .await
            .expect("insert login account");
        }
        (database, path)
    }

    #[tokio::test]
    async fn viewer_state_notification_identity_and_cascades_are_account_scoped() {
        let (database, path) = migrated_database().await;
        let mut status = fixture_status();
        let mut transaction = database.writer().begin().await.unwrap();
        upsert_status_on(&mut transaction, &status).await.unwrap();
        upsert_status_viewer_state_on(&mut transaction, &status, "alice@example.test")
            .await
            .unwrap();
        replace_status_tags_on(
            &mut transaction,
            &status.id,
            &status.server_domain,
            &["rust".to_string()],
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();

        let missing_tag_parent = sqlx::query(
            "INSERT INTO status_tags (status_id, server_domain, tag_name)
             VALUES ('same-id', 'example.test', 'missing-parent')",
        )
        .execute(database.writer())
        .await;
        assert!(missing_tag_parent.is_err());

        sqlx::query("INSERT INTO tags (name, server_domain) VALUES ('temporary', 'example.test')")
            .execute(database.writer())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO status_tags (status_id, server_domain, tag_name)
             VALUES ('same-id', 'example.test', 'temporary')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query("DELETE FROM tags WHERE name = 'temporary' AND server_domain = 'example.test'")
            .execute(database.writer())
            .await
            .unwrap();
        let temporary_mapping_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM status_tags WHERE tag_name = 'temporary'")
                .fetch_one(database.reader())
                .await
                .unwrap();
        assert_eq!(temporary_mapping_count, 0);

        status.favourited = Some(false);
        status.bookmarked = Some(true);
        status.fetched_at = "2026-01-01T00:01:00Z".to_string();
        let mut transaction = database.writer().begin().await.unwrap();
        upsert_status_on(&mut transaction, &status).await.unwrap();
        upsert_status_viewer_state_on(&mut transaction, &status, "bob@example.test")
            .await
            .unwrap();
        transaction.commit().await.unwrap();

        let keys = vec![
            (
                "alice@example.test".to_string(),
                "same-id".to_string(),
                "example.test".to_string(),
            ),
            (
                "bob@example.test".to_string(),
                "same-id".to_string(),
                "example.test".to_string(),
            ),
        ];
        let states = get_viewer_states_by_keys(database.reader(), &keys)
            .await
            .unwrap();
        assert_eq!(states[&keys[0]].favourited, Some(true));
        assert_eq!(states[&keys[0]].bookmarked, Some(false));
        assert_eq!(states[&keys[1]].favourited, Some(false));
        assert_eq!(states[&keys[1]].bookmarked, Some(true));

        let mut large_key_set = (0..300)
            .map(|index| {
                (
                    "alice@example.test".to_string(),
                    format!("missing-{index}"),
                    "example.test".to_string(),
                )
            })
            .collect::<Vec<_>>();
        large_key_set[0] = keys[0].clone();
        let chunked_states = get_viewer_states_by_keys(database.reader(), &large_key_set)
            .await
            .unwrap();
        assert_eq!(chunked_states.len(), 1);
        assert_eq!(chunked_states[&keys[0]].favourited, Some(true));

        sqlx::query(
            "INSERT INTO servers (domain, streaming_url, server_kind)
             VALUES ('actor.example', 'wss://actor.example', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO accounts
               (id, server_domain, username, acct, display_name, note, avatar,
                avatar_static, header, locked, bot, followers_count,
                following_count, statuses_count, created_at)
             VALUES
               ('resolved-author', 'actor.example', 'author', 'author@example.test',
                'Author', '', '', '', '', 0, 0, 0, 0, 0,
                '2026-01-01T00:00:00Z')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        let mut resolved = status.clone();
        resolved.id = "resolved-id".to_string();
        resolved.server_domain = "actor.example".to_string();
        resolved.account_id = "resolved-author".to_string();
        resolved.favourited = Some(false);
        resolved.fetched_at = "2026-01-01T00:02:00Z".to_string();
        let mut transaction = database.writer().begin().await.unwrap();
        upsert_status_on(&mut transaction, &resolved).await.unwrap();
        upsert_status_viewer_state_on(&mut transaction, &resolved, "alice@example.test")
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let canonical_key = ("activitypub".to_string(), status.uri.clone());
        let canonical_states = get_viewer_states_for_identities(
            database.reader(),
            "alice@example.test",
            std::slice::from_ref(&canonical_key),
        )
        .await
        .unwrap();
        assert_eq!(canonical_states[&canonical_key].status_id, "resolved-id");
        assert_eq!(canonical_states[&canonical_key].favourited, Some(false));

        for acct in ["alice@example.test", "bob@example.test"] {
            sqlx::query(
                "INSERT INTO notifications
                   (id, server_domain, account_acct, notification_type,
                    created_at, account_id, status_id)
                 VALUES ('same-notification', 'example.test', ?, 'favourite',
                         '2026-01-01T00:00:00Z', 'author-id', 'same-id')",
            )
            .bind(acct)
            .execute(database.writer())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', 'example.test', 'same-id', ?,
                         '2026-01-01T00:00:00Z')",
            )
            .bind(acct)
            .execute(database.writer())
            .await
            .unwrap();
        }
        let notification_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE id = 'same-notification'")
                .fetch_one(database.reader())
                .await
                .unwrap();
        assert_eq!(notification_count, 2);

        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(database.writer())
            .await
            .unwrap();
        assert!(violations.is_empty());

        delete_status_and_references(database.writer(), "same-id", "example.test")
            .await
            .unwrap();
        delete_status_and_references(database.writer(), "resolved-id", "actor.example")
            .await
            .unwrap();
        for table in [
            "notifications",
            "timeline_entries",
            "status_viewer_state",
            "status_tags",
            "status_identities",
        ] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                    .fetch_one(database.reader())
                    .await
                    .unwrap();
            assert_eq!(count, 0, "{table} retained an orphan");
        }

        database.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn status_delete_uses_targeted_parent_and_cascade_indexes() {
        let (database, path) = migrated_database().await;
        let original_plan = sqlx::query_as::<_, (i64, i64, i64, String)>(
            "EXPLAIN QUERY PLAN DELETE FROM statuses WHERE id = ? AND server_domain = ?",
        )
        .bind("missing")
        .bind("example.test")
        .fetch_all(database.writer())
        .await
        .unwrap();
        assert!(
            original_plan
                .iter()
                .any(|(_, _, _, detail)| { detail.contains("sqlite_autoindex_statuses_1") }),
            "status delete must use the composite primary key: {original_plan:?}"
        );

        let reblog_plan = sqlx::query_as::<_, (i64, i64, i64, String)>(
            "EXPLAIN QUERY PLAN DELETE FROM statuses
             WHERE reblog_of_id = ? AND server_domain = ?",
        )
        .bind("missing")
        .bind("example.test")
        .fetch_all(database.writer())
        .await
        .unwrap();
        assert!(
            reblog_plan
                .iter()
                .any(|(_, _, _, detail)| detail.contains("idx_statuses_reblog")),
            "reblog delete must use the reblog lookup index: {reblog_plan:?}"
        );

        for (table, index) in [
            ("status_viewer_state", "idx_status_viewer_state_status_fk"),
            ("notifications", "idx_notifications_status_fk"),
            (
                "startup_sync_reconciliation_members",
                "idx_startup_sync_reconciliation_status_fk",
            ),
        ] {
            let plan = sqlx::query_as::<_, (i64, i64, i64, String)>(sqlx::AssertSqlSafe(format!(
                "EXPLAIN QUERY PLAN SELECT 1 FROM {table} \
                     WHERE status_id = ? AND server_domain = ?"
            )))
            .bind("missing")
            .bind("example.test")
            .fetch_all(database.writer())
            .await
            .unwrap();
            assert!(
                plan.iter().any(|(_, _, _, detail)| detail.contains(index)),
                "{table} foreign-key lookup did not use {index}: {plan:?}"
            );
        }

        database.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn status_delete_removes_reblogs_only_on_the_target_server() {
        let (database, path) = migrated_database().await;
        let original = fixture_status();
        let mut reblog = original.clone();
        reblog.id = "reblog-id".to_string();
        reblog.uri = "https://example.test/@author/reblog-id".to_string();
        reblog.reblog_of_id = Some(original.id.clone());

        sqlx::query(
            "INSERT INTO servers (domain, streaming_url, server_kind)
             VALUES ('other.test', 'wss://other.test', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO accounts
               (id, server_domain, username, acct, display_name, note, avatar,
                avatar_static, header, locked, bot, followers_count,
                following_count, statuses_count, created_at)
             VALUES
               ('author-id', 'other.test', 'author', 'author@other.test',
                'Author', '', '', '', '', 0, 0, 0, 0, 0,
                '2026-01-01T00:00:00Z')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        let mut other_server = original.clone();
        other_server.server_domain = "other.test".to_string();
        other_server.uri = "https://other.test/@author/same-id".to_string();

        let mut transaction = database.writer().begin().await.unwrap();
        upsert_status_on(&mut transaction, &original).await.unwrap();
        upsert_status_on(&mut transaction, &reblog).await.unwrap();
        upsert_status_on(&mut transaction, &other_server)
            .await
            .unwrap();
        transaction.commit().await.unwrap();

        assert_eq!(
            delete_status_and_references(database.writer(), "same-id", "example.test")
                .await
                .unwrap(),
            2
        );
        let remaining = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM statuses WHERE id = 'same-id' AND server_domain = 'other.test'",
        )
        .fetch_one(database.reader())
        .await
        .unwrap();
        assert_eq!(remaining, 1);

        database.close().await;
        let _ = std::fs::remove_file(path);
    }
}
