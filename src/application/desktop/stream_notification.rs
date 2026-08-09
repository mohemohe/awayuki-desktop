//! Persistence and native notification policy for streamed notifications.
//!
//! These side effects consume the source account carried by the stream event.
//! They never consult the Active account, which is reserved for user-initiated
//! mutations.

use chrono::Utc;

use super::{
    accounts, notification_mutes, settings, settings_application, timeline_service, Database,
    DbAccount, Notification, NotificationSuppressionList, NotificationType,
};
use crate::state::notifications::{NotificationPreferences, NotificationSound};

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

pub(super) async fn desktop_notification_sound(
    database: &Database,
    notification: &Notification,
    server_domain: &str,
) -> Option<NotificationSound> {
    if !matches!(
        &notification.notification_type,
        NotificationType::Reblog | NotificationType::Favourite | NotificationType::Follow
    ) {
        return None;
    }

    match notification_mutes::is_account_muted(
        database.reader(),
        &notification.account.id,
        server_domain,
    )
    .await
    {
        Ok(true) => return None,
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
            if suppression.is_suppressed(acct)
                || suppression.is_suppressed(&display_acct)
                || suppression.is_suppressed(&qualified_acct)
            {
                return None;
            }
        }
        Err(error) => {
            tracing::warn!("Failed to read notification suppression: {}", error);
        }
    };

    let default_sound = match settings_application::load_setting::<NotificationPreferences>(
        database,
        "notification_preferences",
    )
    .await
    {
        Ok(preferences) => preferences.default_sound,
        Err(error) => {
            tracing::warn!("Failed to read notification preferences: {}", error);
            NotificationSound::default()
        }
    };

    let columns = match settings::get_all_column_configs(database.reader()).await {
        Ok(columns) => columns,
        Err(error) => {
            tracing::warn!("Failed to read pane notification preferences: {}", error);
            return Some(default_sound);
        }
    };
    resolve_notification_sound(
        default_sound,
        columns
            .iter()
            .filter(|column| column.column_type == "notification")
            .map(|column| {
                (
                    column.pane_index.unwrap_or(0),
                    column.desktop_notifications,
                    column.notification_sound.as_deref(),
                )
            }),
    )
}

fn resolve_notification_sound<'a>(
    default_sound: NotificationSound,
    panes: impl IntoIterator<Item = (i32, bool, Option<&'a str>)>,
) -> Option<NotificationSound> {
    let mut saw_notification_pane = false;
    let mut seen_panes = std::collections::HashSet::new();
    for (pane_index, enabled, sound) in panes {
        if !seen_panes.insert(pane_index) {
            continue;
        }
        saw_notification_pane = true;
        if enabled {
            return Some(
                sound
                    .and_then(NotificationSound::parse)
                    .unwrap_or(default_sound),
            );
        }
    }
    if saw_notification_pane {
        None
    } else {
        Some(default_sound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_layout_without_notification_pane_uses_global_sound() {
        assert_eq!(
            resolve_notification_sound(NotificationSound::Message, []),
            Some(NotificationSound::Message)
        );
    }

    #[test]
    fn all_notification_panes_disabled_suppresses_the_toast() {
        assert_eq!(
            resolve_notification_sound(
                NotificationSound::Default,
                [(0, false, None), (1, false, Some("Mail"))],
            ),
            None
        );
    }

    #[test]
    fn first_enabled_pane_overrides_the_global_sound() {
        assert_eq!(
            resolve_notification_sound(
                NotificationSound::Reminder,
                [
                    (0, false, Some("Message")),
                    (1, true, Some("Mail")),
                    (1, true, Some("Message")),
                ],
            ),
            Some(NotificationSound::Mail)
        );
    }
}
