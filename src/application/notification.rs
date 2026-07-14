//! Unified notification timeline reads.
//!
//! This boundary intentionally has no account selector: notifications are
//! aggregated from the portable cache for every signed-in source.

use sqlx::SqlitePool;

use crate::application::desktop::{
    apply_viewer_states_to_views, notification_db_to_view_with_context, CachedStatusViewContext,
    TimelineStatus,
};
use crate::db::queries::read_models;

pub(crate) async fn query_cached_statuses(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatus>, String> {
    let context = read_models::load_notification_page_context(pool, limit, offset)
        .await
        .map_err(|error| error.to_string())?;
    tracing::debug!(
        statement_count = context.statement_count,
        notification_count = context.notifications.len(),
        "Loaded unified notification page with bounded SQL statements"
    );
    let primary_statuses = context.statuses.values().cloned().collect::<Vec<_>>();
    let status_context = CachedStatusViewContext::load(pool, &primary_statuses).await?;
    let mut views = Vec::with_capacity(context.notifications.len());
    for notification in context.notifications {
        let actor_account = context
            .accounts
            .get(&(
                notification.account_id.clone(),
                notification.server_domain.clone(),
            ))
            .cloned();
        let status = notification.status_id.as_ref().and_then(|status_id| {
            context
                .statuses
                .get(&(status_id.clone(), notification.server_domain.clone()))
                .cloned()
        });
        views.push(notification_db_to_view_with_context(
            notification,
            actor_account,
            status,
            &status_context,
        ));
    }
    apply_viewer_states_to_views(pool, &mut views).await?;
    Ok(views)
}
