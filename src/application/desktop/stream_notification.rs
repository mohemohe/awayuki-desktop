//! Persistence and native notification policy for streamed notifications.
//!
//! These side effects consume the source account carried by the stream event.
//! They never consult the Active account, which is reserved for user-initiated
//! mutations.

use chrono::Utc;

use super::{
    accounts, notification_mutes, settings_application, timeline_service, Database, DbAccount,
    Notification, NotificationSuppressionList, NotificationType,
};

pub(super) async fn save_notification_to_db<F>(
    database: &Database,
    notification: &Notification,
    server_domain: &str,
    source_acct: &str,
    mut on_commit: F,
) -> Result<(), String>
where
    F: FnMut(),
{
    let account = DbAccount::from_api(&notification.account, server_domain);
    accounts::upsert_account(database.writer(), &account)
        .await
        .map_err(|error| error.to_string())?;
    on_commit();

    if let Some(status) = notification.status.as_ref() {
        timeline_service::save_status_for_viewer_to_db_with_retry(
            database.writer(),
            status,
            server_domain,
            source_acct,
        )
        .await
        .map_err(|error| error.to_string())?;
        on_commit();
    }

    sqlx::query(
        "INSERT INTO notifications (id, server_domain, account_acct, notification_type, created_at, account_id, status_id, read_at, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)
         ON CONFLICT(id, server_domain, account_acct) DO UPDATE SET
           notification_type = excluded.notification_type,
           created_at = excluded.created_at,
           account_id = excluded.account_id,
           status_id = excluded.status_id,
           fetched_at = excluded.fetched_at",
    )
    .bind(&notification.id)
    .bind(server_domain)
    .bind(source_acct)
    .bind(notification.notification_type.as_str())
    .bind(notification.created_at.to_rfc3339())
    .bind(&notification.account.id)
    .bind(notification.status.as_ref().map(|status| status.id.as_str()))
    .bind(Utc::now().to_rfc3339())
    .execute(database.writer())
    .await
    .map_err(|error| error.to_string())?;
    on_commit();

    Ok(())
}

pub(super) async fn should_send_desktop_notification(
    database: &Database,
    notification: &Notification,
    server_domain: &str,
) -> bool {
    if !matches!(
        &notification.notification_type,
        NotificationType::Reblog | NotificationType::Favourite | NotificationType::Follow
    ) {
        return false;
    }

    match notification_mutes::is_account_muted(
        database.reader(),
        &notification.account.id,
        server_domain,
    )
    .await
    {
        Ok(true) => return false,
        Ok(false) => {}
        Err(error) => tracing::warn!("Failed to read notification mute state: {}", error),
    }

    match settings_application::load_setting::<NotificationSuppressionList>(
        database,
        "notification_suppression",
    )
    .await
    {
        Ok(suppression) => {
            let acct = notification.account.acct.trim_start_matches('@');
            let display_acct = format!("@{}", acct);
            let qualified_acct = if acct.contains('@') {
                acct.to_string()
            } else {
                format!("{}@{}", acct, server_domain)
            };
            !suppression.is_suppressed(acct)
                && !suppression.is_suppressed(&display_acct)
                && !suppression.is_suppressed(&qualified_acct)
        }
        Err(error) => {
            tracing::warn!("Failed to read notification suppression: {}", error);
            true
        }
    }
}
