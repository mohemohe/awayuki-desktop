//! Timeline read operation lifecycle.
//!
//! Home, Public, and Notification requests remain aggregate Unified Timeline
//! reads. This layer never fills a missing `account_acct` from Active account;
//! account-bound timeline kinds must carry an explicit source in the request.

use std::collections::HashSet;
use std::time::Instant;

use chrono::{DateTime, Utc};
use tauri::State;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;
use crate::application::desktop::{
    acting_session, db_status_refs_to_views, db_statuses_to_views, dedupe_statuses_by_uri,
    query_aggregate_timeline_statuses, query_bookmarked_statuses, query_cached_status,
    query_custom_statuses, query_favourited_statuses, query_search_statuses_with_cancellation,
    query_status_thread_statuses, query_timeline_statuses, query_user_bookmarked_statuses,
    query_yq_statuses_with_metrics, refresh_aggregate_notifications, refresh_aggregate_timeline,
    session_for_read_source, session_for_timeline_source, status_to_view,
    timeline_status_matches_display_filter, timeline_type_can_load_more_from_api, with_source_acct,
    RuntimeState, StatusViewerStateSummary, TimelineGap, TimelinePageResponse, TimelineStatus,
};
use crate::application::notification;
use crate::constants::DEFAULT_TIMELINE_LIMIT;
use crate::db::queries::custom_timeline::CustomTimelineError;
use crate::db::queries::statuses as status_queries;
use crate::ipc::dto::{
    AirContextRequest, CancelQuoteConsumerRequest, CancelTimelineQueryRequest, StatusThreadRequest,
    StatusViewerStatesRequest, TimelineRequest,
};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::types::status::Status;
use crate::observability::OperationContext;
use crate::services::kq_timeline::{self, KqTimelineError};
use crate::services::startup_sync;
use crate::services::timeline_service;
use crate::services::timeline_service::TimelineType;

const TIMELINE_QUERY_METRICS_EVENT: &str = "timeline-query-metrics";

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn column_param_for_log(column_type: &str, column_param: Option<&str>) -> Option<String> {
    column_param.map(|value| {
        if column_type.eq_ignore_ascii_case("kq") {
            format!("<kq-query:{} bytes>", value.len())
        } else {
            value.to_string()
        }
    })
}

fn account_acct_for_log(column_type: &str, account_acct: Option<&str>) -> Option<String> {
    if column_type.eq_ignore_ascii_case("kq") {
        None
    } else {
        account_acct.map(str::to_string)
    }
}

struct TimelineCommandLogContext<'a> {
    command: &'a str,
    column_type: &'a str,
    column_param: &'a Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    since_status_id: &'a Option<String>,
    since_server_domain: &'a Option<String>,
    started_at: Instant,
}

#[derive(Debug, thiserror::Error)]
enum LocalTimelineError {
    #[error(transparent)]
    CustomTimeline(#[from] CustomTimelineError),
    #[error(transparent)]
    KrileQuery(#[from] KqTimelineError),
    #[error("{0}")]
    Other(String),
}

impl From<String> for LocalTimelineError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl LocalTimelineError {
    fn safe_log_message(&self) -> String {
        match self {
            Self::KrileQuery(KqTimelineError::Compile { line, column, .. }) => {
                format!("KQ query failed static validation at line {line}, column {column}")
            }
            Self::KrileQuery(KqTimelineError::Timeout {
                scanned_count,
                max_scanned_rows,
                max_duration_ms,
            }) => format!(
                "KQ query exceeded its evaluation budget: scanned={scanned_count}, \
                 max_rows={max_scanned_rows}, max_duration_ms={max_duration_ms}"
            ),
            Self::KrileQuery(KqTimelineError::Cancelled) => "KQ query cancelled".to_string(),
            Self::KrileQuery(KqTimelineError::Database(_)) => {
                "KQ database query failed".to_string()
            }
            Self::CustomTimeline(error) => error.to_string(),
            Self::Other(error) => error.clone(),
        }
    }
}

fn local_timeline_app_error(error: LocalTimelineError, request_id: &str) -> AppError {
    match error {
        LocalTimelineError::CustomTimeline(error) => {
            tracing::warn!(error = ?error, "Custom timeline repository rejected a query");
            if let Some(message_key) = error.user_message_key() {
                // The rejected FTS error previously reached `from_source` as
                // an internal, non-retryable error. Only replace its catalog
                // key so retry and UI behaviour remain unchanged.
                AppError::from_code(AppErrorCode::Internal, error, request_id)
                    .with_message_key(message_key)
            } else {
                AppError::from_source(error, request_id)
            }
        }
        LocalTimelineError::KrileQuery(error) => kq_timeline_app_error(error, request_id),
        LocalTimelineError::Other(error) => AppError::from_source(error, request_id),
    }
}

fn kq_timeline_app_error(error: KqTimelineError, request_id: &str) -> AppError {
    match error {
        KqTimelineError::Compile { line, column, .. } => AppError::from_code(
            AppErrorCode::Validation,
            "KQ query failed static validation",
            request_id,
        )
        .with_message_key("errors.kq_invalid_query")
        .with_safe_detail("line", line)
        .with_safe_detail("column", column),
        KqTimelineError::Timeout {
            scanned_count,
            max_scanned_rows,
            max_duration_ms,
        } => AppError::from_code(
            AppErrorCode::Validation,
            KqTimelineError::Timeout {
                scanned_count,
                max_scanned_rows,
                max_duration_ms,
            },
            request_id,
        )
        .with_message_key("errors.kq_query_budget_exceeded")
        .with_safe_detail("limit", max_scanned_rows),
        KqTimelineError::Cancelled => AppError::from_code(
            AppErrorCode::Cancelled,
            KqTimelineError::Cancelled,
            request_id,
        ),
        KqTimelineError::Database(error) => AppError::from_database(error, request_id),
    }
}

fn log_timeline_page_command_result(
    context: &TimelineCommandLogContext<'_>,
    result: &Result<TimelinePageResponse, LocalTimelineError>,
) {
    match result {
        Ok(page) => tracing::info!(
            command = context.command,
            column_type = context.column_type,
            column_param = ?context.column_param,
            limit = ?context.limit,
            offset = ?context.offset,
            since_status_id = ?context.since_status_id,
            since_server_domain = ?context.since_server_domain,
            count = page.statuses.len(),
            gap_count = page.gaps.len(),
            duration_ms = elapsed_ms(context.started_at),
            "[awayuki][tauri-command] timeline page command success"
        ),
        Err(error) => tracing::info!(
            command = context.command,
            column_type = context.column_type,
            column_param = ?context.column_param,
            limit = ?context.limit,
            offset = ?context.offset,
            since_status_id = ?context.since_status_id,
            since_server_domain = ?context.since_server_domain,
            duration_ms = elapsed_ms(context.started_at),
            "[awayuki][tauri-command] timeline page command error: {}",
            error.safe_log_message()
        ),
    }
}

pub(crate) async fn load_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    let mut operation = OperationContext::start(
        "load_timeline",
        request.operation_id.as_deref(),
        request.account_acct.as_deref(),
    );
    if TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
        .is_none()
    {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "unsupported timeline configuration",
        ));
    }
    let Some(query_operation) = state.timeline_query_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "timeline query operation is already active",
        ));
    };
    let started_at = Instant::now();
    let column_type = request.column_type.clone();
    let column_param = column_param_for_log(&request.column_type, request.column_param.as_deref());
    let limit = request.limit;
    let offset = request.offset;
    let since_status_id = request.since_status_id.clone();
    let since_server_domain = request.since_server_domain.clone();
    tracing::info!(
        column_type = column_type.as_str(),
        column_param = ?column_param,
        limit = ?limit,
        offset = ?offset,
        since_status_id = ?since_status_id,
        since_server_domain = ?since_server_domain,
        "[awayuki][tauri-command] load_timeline start"
    );
    operation.phase("db");
    let result = load_local_timeline(&state, request, query_operation.token()).await;
    match &result {
        Ok(statuses) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit = ?limit,
            offset = ?offset,
            since_status_id = ?since_status_id,
            since_server_domain = ?since_server_domain,
            count = statuses.len(),
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_timeline success"
        ),
        Err(error) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit = ?limit,
            offset = ?offset,
            since_status_id = ?since_status_id,
            since_server_domain = ?since_server_domain,
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_timeline error: {}",
            error.safe_log_message()
        ),
    }
    match result {
        Ok(statuses) => {
            operation.finish_ok();
            Ok(statuses)
        }
        Err(error) => {
            let app_error = local_timeline_app_error(error, operation.id());
            Err(operation.finish_app_error(app_error))
        }
    }
}

pub(crate) async fn load_more_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    let mut operation = OperationContext::start(
        "load_more_timeline",
        request.operation_id.as_deref(),
        request.account_acct.as_deref(),
    );
    let started_at = Instant::now();
    let column_type = request.column_type.clone();
    let column_param = column_param_for_log(&request.column_type, request.column_param.as_deref());
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(120);
    let offset = request.offset.unwrap_or(0);
    let max_status_id = request.max_status_id.clone();
    tracing::info!(
        column_type = column_type.as_str(),
        column_param = ?column_param,
        limit,
        offset,
        max_status_id = ?max_status_id,
        "[awayuki][tauri-command] load_more_timeline start"
    );

    let Some(tl_type) =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
    else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "unsupported timeline configuration",
        ));
    };
    let Some(query_operation) = state.timeline_query_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "timeline query operation is already active",
        ));
    };
    operation.phase(if timeline_type_can_load_more_from_api(&tl_type) {
        "api"
    } else {
        "db"
    });
    let load = async {
        if timeline_type_can_load_more_from_api(&tl_type) {
            load_more_api_timeline(&state, request, &tl_type, limit, query_operation.token())
                .await
                .map_err(LocalTimelineError::from)
        } else {
            load_local_timeline(&state, request, query_operation.token())
                .await
                .map(|statuses| TimelinePageResponse {
                    has_more: statuses.len() >= limit as usize,
                    statuses,
                    gaps: Vec::new(),
                })
        }
    };
    let result = load.await;

    match &result {
        Ok(response) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit,
            offset,
            max_status_id = ?max_status_id,
            count = response.statuses.len(),
            has_more = response.has_more,
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_more_timeline success"
        ),
        Err(error) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit,
            offset,
            max_status_id = ?max_status_id,
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_more_timeline error: {}",
            error.safe_log_message()
        ),
    }
    match result {
        Ok(response) => {
            operation.finish_ok();
            Ok(response)
        }
        Err(error) => {
            let app_error = local_timeline_app_error(error, operation.id());
            Err(operation.finish_app_error(app_error))
        }
    }
}

pub(crate) async fn load_timeline_gap(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    let mut operation = OperationContext::start(
        "load_timeline_gap",
        request.operation_id.as_deref(),
        request.account_acct.as_deref(),
    );
    let Some(timeline_type) =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
    else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "unsupported timeline configuration",
        ));
    };
    if !matches!(
        timeline_type,
        TimelineType::Home
            | TimelineType::Public
            | TimelineType::Local
            | TimelineType::List(_)
            | TimelineType::Hashtag(_)
    ) || request.account_acct.as_deref().is_none_or(str::is_empty)
        || request.max_status_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "timeline gap request requires a remote timeline source and boundary cursor",
        ));
    }
    let Some(query_operation) = state.timeline_query_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "timeline query operation is already active",
        ));
    };
    operation.phase("api");
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(80);
    match load_more_api_timeline(
        &state,
        request,
        &timeline_type,
        limit,
        query_operation.token(),
    )
    .await
    {
        Ok(page) => {
            operation.finish_ok();
            Ok(page)
        }
        Err(error) => {
            let app_error = local_timeline_app_error(error.into(), operation.id());
            Err(operation.finish_app_error(app_error))
        }
    }
}

pub(crate) async fn cancel_timeline_query(
    state: State<'_, RuntimeState>,
    request: CancelTimelineQueryRequest,
) -> Result<bool, AppError> {
    let mut operation = OperationContext::start(
        "cancel_timeline_query",
        request.operation_id.as_deref(),
        None,
    );
    if uuid::Uuid::parse_str(&request.target_operation_id).is_err() {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "target operation ID must be a UUID",
        ));
    }
    let cancelled = state
        .timeline_query_manager()
        .cancel(&request.target_operation_id);
    operation.finish_ok();
    Ok(cancelled)
}

pub(crate) async fn cancel_quote_consumer(
    request: CancelQuoteConsumerRequest,
) -> Result<bool, AppError> {
    let mut operation = OperationContext::start(
        "cancel_quote_consumer",
        request.operation_id.as_deref(),
        None,
    );
    let consumer_id = request.quote_consumer_id.trim();
    if consumer_id.is_empty() || consumer_id.len() > 256 {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "quote consumer ID must contain 1 to 256 bytes",
        ));
    }
    let cancelled = timeline_service::cancel_pending_quote_consumer(consumer_id);
    operation.finish_ok();
    Ok(cancelled)
}

pub(crate) async fn refresh_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    let mut operation = OperationContext::start(
        "refresh_timeline",
        request.operation_id.as_deref(),
        request.account_acct.as_deref(),
    );
    let Some(query_operation) = state.timeline_query_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "timeline query operation is already active",
        ));
    };
    operation.phase("api");
    let result = refresh_timeline_inner(state, request, query_operation.token()).await;
    match result {
        Ok(statuses) => {
            operation.phase("commit");
            operation.finish_ok();
            Ok(statuses)
        }
        Err(error) => {
            let app_error = local_timeline_app_error(error, operation.id());
            Err(operation.finish_app_error(app_error))
        }
    }
}

async fn refresh_timeline_inner(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<TimelinePageResponse, LocalTimelineError> {
    let total_started_at = Instant::now();
    let request_column_type = request.column_type.clone();
    let request_column_param =
        column_param_for_log(&request.column_type, request.column_param.as_deref());
    let request_limit = request.limit;
    let request_offset = request.offset;
    let request_since_status_id = request.since_status_id.clone();
    let request_since_server_domain = request.since_server_domain.clone();
    let request_account_acct =
        account_acct_for_log(&request.column_type, request.account_acct.as_deref());
    let log_context = TimelineCommandLogContext {
        command: "refresh_timeline",
        column_type: &request_column_type,
        column_param: &request_column_param,
        limit: request_limit,
        offset: request_offset,
        since_status_id: &request_since_status_id,
        since_server_domain: &request_since_server_domain,
        started_at: total_started_at,
    };
    tracing::info!(
        column_type = request_column_type.as_str(),
        column_param = ?request_column_param,
        limit = ?request_limit,
        offset = ?request_offset,
        since_status_id = ?request_since_status_id,
        since_server_domain = ?request_since_server_domain,
        account_acct = ?request_account_acct,
        "[awayuki][tauri-command] refresh_timeline start"
    );
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(80);
    let tl_type =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
            .ok_or_else(|| "Unsupported timeline type".to_string())?;

    if matches!(tl_type, TimelineType::Notification) {
        let result = refresh_aggregate_notifications(
            &state,
            limit,
            request.quote_consumer_id.as_deref(),
            cancellation,
        )
        .await
        .map(|statuses| TimelinePageResponse {
            has_more: !statuses.is_empty(),
            statuses,
            gaps: Vec::new(),
        })
        .map_err(LocalTimelineError::from);
        log_timeline_page_command_result(&log_context, &result);
        return result;
    }

    // Home/Public are always Unified, even if a historical column row still
    // contains an account binding. Active account must never narrow this path.
    if tl_type.is_unified() {
        let result = refresh_aggregate_timeline(
            &state,
            &tl_type,
            request.acting_account_acct.as_deref(),
            limit,
            request.display_filter,
            request.quote_consumer_id.as_deref(),
            cancellation,
        )
        .await
        .map(|(statuses, gaps)| TimelinePageResponse {
            has_more: !statuses.is_empty(),
            statuses,
            gaps,
        })
        .map_err(LocalTimelineError::from);
        log_timeline_page_command_result(&log_context, &result);
        return result;
    }

    if matches!(
        tl_type,
        TimelineType::CustomSql(_)
            | TimelineType::YukariQuery(_)
            | TimelineType::KrileQuery(_)
            | TimelineType::Search(_)
            | TimelineType::Bookmarks
            | TimelineType::Favourites
            | TimelineType::UserBookmarks { .. }
    ) {
        let result = load_local_timeline(&state, request, cancellation)
            .await
            .map(|statuses| TimelinePageResponse {
                has_more: !statuses.is_empty(),
                statuses,
                gaps: Vec::new(),
            });
        log_timeline_page_command_result(&log_context, &result);
        return result;
    }

    let session = session_for_timeline_source(&state, request.account_acct.as_deref()).await?;
    let client = session.client;
    let source_acct = session.acct;
    let mut on_commit = || state.emit_timeline_cache_committed(&source_acct, client.domain());
    let page = timeline_service::sync_timeline_with_control(
        &client,
        state.database().writer(),
        &tl_type,
        &source_acct,
        &TimelineParams {
            limit: Some(limit),
            ..Default::default()
        },
        timeline_service::TimelineSyncControl {
            quote_consumer_id: request.quote_consumer_id.as_deref(),
            cancellation: Some(cancellation),
            on_commit: &mut on_commit,
        },
    )
    .await
    .map_err(|error| error.to_string())?;

    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let result: Result<TimelinePageResponse, LocalTimelineError> = Ok(TimelinePageResponse {
        has_more: !page.statuses.is_empty(),
        statuses: page
            .statuses
            .into_iter()
            .map(|status| {
                with_source_acct(
                    status_to_view(&status, client.domain(), None),
                    Some(source_acct.clone()),
                )
            })
            .filter(|status| timeline_status_matches_display_filter(status, display_filter))
            .collect(),
        gaps: page.gap.map(TimelineGap::from).into_iter().collect(),
    });
    log_timeline_page_command_result(&log_context, &result);
    result
}

async fn load_more_api_timeline(
    state: &RuntimeState,
    request: TimelineRequest,
    timeline_type: &TimelineType,
    limit: u32,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<TimelinePageResponse, String> {
    const MAX_API_PAGES_PER_LOAD_MORE: usize = 10;

    let session = session_for_timeline_source(state, request.account_acct.as_deref()).await?;
    let client = session.client;
    let source_acct = session.acct;
    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let page_limit = limit.clamp(1, 80);
    let mut max_id = request.max_status_id;
    let mut statuses = Vec::new();
    let mut has_more = true;
    let mut scanned_pages = 0usize;
    let mut gap = None;
    let mut on_commit = || state.emit_timeline_cache_committed(&source_acct, client.domain());

    while statuses.len() < limit as usize && scanned_pages < MAX_API_PAGES_PER_LOAD_MORE {
        if cancellation.is_cancelled() {
            return Err("timeline query cancelled".to_string());
        }
        let raw_page = timeline_service::sync_timeline_with_control(
            &client,
            state.database().writer(),
            timeline_type,
            &source_acct,
            &TimelineParams {
                max_id: max_id.clone(),
                limit: Some(page_limit),
                ..Default::default()
            },
            timeline_service::TimelineSyncControl {
                quote_consumer_id: request.quote_consumer_id.as_deref(),
                cancellation: Some(cancellation),
                on_commit: &mut on_commit,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        let raw_statuses = raw_page.statuses;
        gap = raw_page.gap;
        scanned_pages += 1;

        if raw_statuses.is_empty() {
            has_more = false;
            break;
        }
        let raw_count = raw_statuses.len();
        max_id = raw_statuses.last().map(|status| status.id.clone());
        let matched_before = statuses.len();
        statuses.extend(raw_statuses.into_iter().filter_map(|status| {
            let view = with_source_acct(
                status_to_view(&status, client.domain(), None),
                Some(source_acct.clone()),
            );
            timeline_status_matches_display_filter(&view, display_filter).then_some(view)
        }));
        tracing::info!(
            timeline = timeline_type.as_str(),
            source_acct = source_acct.as_str(),
            raw_count,
            matched_count = statuses.len().saturating_sub(matched_before),
            scanned_pages,
            next_max_id = ?max_id,
            "[awayuki][tauri-command] load_more_timeline api page"
        );
    }

    statuses.truncate(limit as usize);
    Ok(TimelinePageResponse {
        statuses,
        has_more,
        gaps: gap.map(TimelineGap::from).into_iter().collect(),
    })
}

async fn load_local_timeline(
    state: &RuntimeState,
    request: TimelineRequest,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Vec<TimelineStatus>, LocalTimelineError> {
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(120) as i64;
    let offset = request.offset.unwrap_or(0) as i64;
    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let tl_type =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
            .ok_or_else(|| "Unsupported timeline type".to_string())?;

    let statuses = match tl_type {
        TimelineType::CustomSql(sql) => {
            query_custom_statuses(
                state.database().analytics_reader(),
                &sql,
                limit,
                offset,
                cancellation,
            )
            .await?
        }
        TimelineType::YukariQuery(query) => {
            let result = query_yq_statuses_with_metrics(
                state.database().analytics_reader(),
                &query,
                limit,
                offset,
                request
                    .since_status_id
                    .as_deref()
                    .zip(request.since_server_domain.as_deref()),
                request
                    .max_status_id
                    .as_deref()
                    .zip(request.max_server_domain.as_deref()),
                cancellation,
            )
            .await?;
            if result.metrics.slow {
                state
                    .emit_application_event(
                        TIMELINE_QUERY_METRICS_EVENT,
                        result.metrics.clone(),
                        "slow YQ query metrics",
                    )
                    .await;
            }
            result.statuses
        }
        TimelineType::KrileQuery(query) => {
            let result = kq_timeline::query_statuses(
                state.database().analytics_reader(),
                &query,
                limit,
                offset,
                request
                    .since_status_id
                    .as_deref()
                    .zip(request.since_server_domain.as_deref()),
                request
                    .max_status_id
                    .as_deref()
                    .zip(request.max_server_domain.as_deref()),
                cancellation,
            )
            .await?;
            if result.metrics.slow {
                state
                    .emit_application_event(
                        TIMELINE_QUERY_METRICS_EVENT,
                        result.metrics.clone(),
                        "slow KQ query metrics",
                    )
                    .await;
            }
            result.statuses
        }
        TimelineType::Search(query) => {
            // Search uses ICU4X segment-prefix candidates. Pending rows and
            // the incomplete-backfill gap are materialized into bounded
            // windows before using the connection-local ICU matcher.
            // Keep analytical search off the latency-sensitive reader path.
            query_search_statuses_with_cancellation(
                state.database().analytics_reader(),
                &query,
                limit,
                offset,
                display_filter,
                request
                    .max_status_id
                    .as_deref()
                    .zip(request.max_server_domain.as_deref()),
                cancellation,
            )
            .await?
        }
        TimelineType::Bookmarks => {
            let statuses = query_bookmarked_statuses(
                state.database().reader(),
                request.account_acct.as_deref(),
                limit,
                offset,
            )
            .await?;
            return Ok(db_status_refs_to_views(state.database().reader(), statuses).await?);
        }
        TimelineType::Favourites => {
            let statuses = query_favourited_statuses(
                state.database().reader(),
                request.account_acct.as_deref(),
                limit,
                offset,
            )
            .await?;
            return Ok(db_status_refs_to_views(state.database().reader(), statuses).await?);
        }
        TimelineType::UserBookmarks {
            server_domain,
            account_id,
        } => {
            let statuses = query_user_bookmarked_statuses(
                state.database().reader(),
                &server_domain,
                &account_id,
                request.account_acct.as_deref(),
                limit,
                offset,
            )
            .await?;
            return Ok(db_status_refs_to_views(state.database().reader(), statuses).await?);
        }
        TimelineType::Notification => {
            return Ok(notification::query_cached_statuses(
                state.database().reader(),
                limit,
                offset,
            )
            .await?);
        }
        TimelineType::Home | TimelineType::Public => {
            debug_assert!(tl_type.is_unified());
            let statuses = query_aggregate_timeline_statuses(
                state.database().reader(),
                &tl_type.as_str(),
                request.acting_account_acct.as_deref(),
                limit,
                offset,
                display_filter,
            )
            .await?;
            return Ok(db_status_refs_to_views(state.database().reader(), statuses).await?);
        }
        _ => {
            let source_acct = session_for_timeline_source(state, request.account_acct.as_deref())
                .await?
                .acct;
            let statuses = query_timeline_statuses(
                state.database().reader(),
                &tl_type.as_str(),
                &source_acct,
                limit,
                offset,
                display_filter,
            )
            .await?;
            return Ok(db_status_refs_to_views(state.database().reader(), statuses).await?);
        }
    };

    Ok(db_statuses_to_views(state.database().reader(), statuses).await?)
}

pub(crate) async fn status_viewer_states(
    state: State<'_, RuntimeState>,
    request: StatusViewerStatesRequest,
) -> Result<Vec<StatusViewerStateSummary>, AppError> {
    const MAX_IDENTITIES: usize = 1_000;
    let mut operation = OperationContext::start(
        "status_viewer_states",
        request.operation_id.as_deref(),
        Some(&request.acting_account_acct),
    );
    if let Err(error) = acting_session(&state, &request.acting_account_acct).await {
        return Err(operation.finish_error_code(AppErrorCode::AuthenticationExpired, error));
    }
    if request.identities.len() > MAX_IDENTITIES {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            format!("status viewer state request exceeds {MAX_IDENTITIES} identities"),
        ));
    }
    let mut seen = HashSet::new();
    let mut identities = Vec::with_capacity(request.identities.len());
    for identity in request.identities {
        if let Err(error) = identity.validate() {
            return Err(operation.finish_error_code(AppErrorCode::Validation, error));
        }
        if seen.insert(identity.clone()) {
            identities.push(identity);
        }
    }
    let keys = identities
        .iter()
        .map(|identity| {
            (
                identity.protocol.as_db_str().to_string(),
                identity.canonical_uri.clone(),
            )
        })
        .collect::<Vec<_>>();
    operation.phase("db");
    let states = match status_queries::get_viewer_states_for_identities(
        state.database().reader(),
        &request.acting_account_acct,
        &keys,
    )
    .await
    {
        Ok(states) => states,
        Err(error) => {
            let app_error = AppError::from_database(error, operation.id());
            return Err(operation.finish_app_error(app_error));
        }
    };

    let summaries = identities
        .into_iter()
        .map(|identity| {
            let key = (
                identity.protocol.as_db_str().to_string(),
                identity.canonical_uri.clone(),
            );
            let state = states.get(&key);
            StatusViewerStateSummary {
                identity,
                favourited: state.and_then(|value| value.favourited).unwrap_or(false),
                reblogged: state.and_then(|value| value.reblogged).unwrap_or(false),
                bookmarked: state.and_then(|value| value.bookmarked).unwrap_or(false),
            }
        })
        .collect();
    operation.finish_ok();
    Ok(summaries)
}

pub(crate) async fn air_context(
    state: State<'_, RuntimeState>,
    request: AirContextRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let limit = request.limit.unwrap_or(2).clamp(1, 2) as usize;
    let notification_created_at =
        parse_air_context_notification_created_at(&request.notification_created_at)?;
    let session = session_for_read_source(
        &state,
        &request.server_domain,
        request.source_acct.as_deref(),
    )
    .await?
    .ok_or_else(|| "No signed-in account for this server".to_string())?;

    let target_status = match session.client.get_status(&request.status_id).await {
        Ok(status) => {
            timeline_service::save_status_to_db_with_retry(
                state.database().writer(),
                &status,
                session.client.domain(),
            )
            .await
            .map_err(|error| error.to_string())?;
            state.emit_timeline_cache_committed(&session.acct, session.client.domain());
            Some(status)
        }
        Err(error) => {
            tracing::info!(
                status_id = request.status_id.as_str(),
                server_domain = session.client.domain(),
                "[awayuki][application] air_context target fetch fallback: {}",
                error
            );
            None
        }
    };

    let mut views = match target_status.as_ref() {
        Some(status) => vec![with_source_acct(
            status_to_view(status, session.client.domain(), None),
            Some(session.acct.clone()),
        )],
        None => {
            let cached = query_cached_status(
                state.database().reader(),
                &request.status_id,
                &request.server_domain,
            )
            .await?
            .ok_or_else(|| "AIR context target status is not cached".to_string())?;
            db_statuses_to_views(state.database().reader(), vec![cached]).await?
        }
    };

    let found = find_air_context_post(
        &session.client,
        &request.account_id,
        &request.status_id,
        notification_created_at,
    )
    .await?;
    let found_statuses = found.into_iter().collect::<Vec<_>>();
    for status in &found_statuses {
        timeline_service::save_status_to_db_with_retry(
            state.database().writer(),
            status,
            session.client.domain(),
        )
        .await
        .map_err(|error| error.to_string())?;
    }
    if !found_statuses.is_empty() {
        state.emit_timeline_cache_committed(&session.acct, session.client.domain());
        if let Some(consumer_id) = request.quote_consumer_id.as_deref() {
            timeline_service::schedule_pending_quote_resolution_for_consumer(
                &session.client,
                state.database().writer(),
                &found_statuses,
                session.client.domain(),
                &session.acct,
                consumer_id,
            );
        } else {
            timeline_service::schedule_pending_quote_resolution(
                &session.client,
                state.database().writer(),
                &found_statuses,
                session.client.domain(),
                &session.acct,
            );
        }
    }
    if let Some(status) = found_statuses.first() {
        views.push(with_source_acct(
            status_to_view(status, session.client.domain(), None),
            Some(session.acct.clone()),
        ));
    }
    views.truncate(limit);
    Ok(views)
}

async fn find_air_context_post(
    client: &ApiClient,
    account_id: &str,
    target_status_id: &str,
    notification_created_at: DateTime<Utc>,
) -> Result<Option<Status>, String> {
    const PAGE_LIMIT: u32 = 40;
    const MAX_PAGES: usize = 8;
    let mut max_id = None;
    let mut candidate: Option<Status> = None;

    for _ in 0..MAX_PAGES {
        let statuses = client
            .get_account_statuses(
                account_id,
                &AccountStatusesParams {
                    max_id: max_id.clone(),
                    limit: Some(PAGE_LIMIT),
                    pinned: None,
                    exclude_replies: Some(false),
                    exclude_reblogs: Some(true),
                    only_media: Some(false),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        if statuses.is_empty() {
            break;
        }
        let mut reached_target_time = false;
        for status in &statuses {
            if status.id == target_status_id || status.account.id != account_id {
                continue;
            }
            if is_post_after_notification(&status.created_at, &notification_created_at) {
                let closer = candidate
                    .as_ref()
                    .map(|current| status.created_at < current.created_at)
                    .unwrap_or(true);
                if closer {
                    candidate = Some(status.clone());
                }
            } else {
                reached_target_time = true;
            }
        }
        if reached_target_time || matches!(client.kind(), ServerKind::Bluesky) {
            break;
        }
        let Some(last_id) = statuses.last().map(|status| status.id.clone()) else {
            break;
        };
        if max_id.as_deref() == Some(last_id.as_str()) {
            break;
        }
        max_id = Some(last_id);
    }
    Ok(candidate)
}

fn is_post_after_notification(
    status_created_at: &DateTime<Utc>,
    notification_created_at: &DateTime<Utc>,
) -> bool {
    status_created_at > notification_created_at
}

fn parse_air_context_notification_created_at(created_at: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(created_at)
        .map(|created_at| created_at.with_timezone(&Utc))
        .map_err(|error| format!("AIR context notification timestamp is invalid: {error}"))
}

pub(crate) async fn status_thread(
    state: State<'_, RuntimeState>,
    request: StatusThreadRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let limit = request.limit.unwrap_or(240).clamp(1, 300) as usize;
    let mut remote_error = None;
    let session = match session_for_read_source(
        &state,
        &request.server_domain,
        request.source_acct.as_deref(),
    )
    .await
    {
        Ok(session) => session,
        Err(error) if request.source_acct.is_none() => {
            remote_error = Some(error);
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(session) = session {
        let mut remote_statuses = Vec::new();
        match session.client.get_status_context(&request.status_id).await {
            Ok(context) => {
                remote_statuses.extend(context.ancestors);
                remote_statuses.extend(context.descendants);
            }
            Err(error) => remote_error = Some(error.to_string()),
        }
        match session.client.get_status(&request.status_id).await {
            Ok(status) => remote_statuses.push(status),
            Err(error) if remote_error.is_none() => remote_error = Some(error.to_string()),
            Err(_) => {}
        }
        dedupe_statuses_by_uri(&mut remote_statuses);
        let mut on_commit = || {
            state.emit_timeline_cache_committed(&session.acct, session.client.domain());
        };
        timeline_service::save_status_batch_with_commit_observer(
            state.database().writer(),
            &remote_statuses,
            session.client.domain(),
            None,
            &mut on_commit,
        )
        .await
        .map_err(|error| error.to_string())?;
        if let Some(consumer_id) = request.quote_consumer_id.as_deref() {
            timeline_service::schedule_pending_quote_resolution_for_consumer(
                &session.client,
                state.database().writer(),
                &remote_statuses,
                session.client.domain(),
                &session.acct,
                consumer_id,
            );
        } else {
            timeline_service::schedule_pending_quote_resolution(
                &session.client,
                state.database().writer(),
                &remote_statuses,
                session.client.domain(),
                &session.acct,
            );
        }
    }

    let statuses = query_status_thread_statuses(
        state.database().reader(),
        &request.status_id,
        &request.server_domain,
        limit,
    )
    .await?;
    if statuses.is_empty() {
        return Err(remote_error.unwrap_or_else(|| "Thread status is not cached".to_string()));
    }
    let retention_keys = statuses
        .iter()
        .map(|status| (status.id.clone(), status.server_domain.clone()))
        .collect::<Vec<_>>();
    startup_sync::protect_thread_statuses(state.database().writer(), &retention_keys, Utc::now())
        .await
        .map_err(|error| error.to_string())?;
    db_statuses_to_views(state.database().reader(), statuses).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_match_context_uses_specific_copy_without_changing_retry_semantics() {
        let error = local_timeline_app_error(
            LocalTimelineError::CustomTimeline(CustomTimelineError::FtsMatchOr(
                sqlx::Error::Protocol("classified FTS MATCH context".to_string()),
            )),
            "request-fts-message",
        );

        assert_eq!(error.code, AppErrorCode::Internal);
        assert!(!error.retryable);
        assert_eq!(
            error.message_key,
            crate::db::queries::custom_timeline::FTS_MATCH_OR_MESSAGE_KEY
        );
    }

    #[test]
    fn kq_timeline_logs_only_query_size() {
        let query = "from search:\"private 雪 phrase\" where user == @alice@example.test";
        let log_value = column_param_for_log("kq", Some(query)).expect("masked log value");

        assert_eq!(log_value, format!("<kq-query:{} bytes>", query.len()));
        assert!(!log_value.contains("private 雪 phrase"));
        assert!(!log_value.contains("alice@example.test"));
        assert_eq!(column_param_for_log("KQ", Some(query)), Some(log_value));
        assert_eq!(account_acct_for_log("kq", Some("alice@example.test")), None);
        assert_eq!(
            column_param_for_log("search", Some("visible search")),
            Some("visible search".to_string())
        );
        assert_eq!(
            account_acct_for_log("search", Some("alice@example.test")),
            Some("alice@example.test".to_string())
        );
    }

    #[test]
    fn kq_compile_error_exposes_only_safe_position_details() {
        const SECRET_SHAPED_UNKNOWN_LITERAL: &str = "private_search__bearer_sk_live_7fd93ac1";
        let local_error = LocalTimelineError::KrileQuery(KqTimelineError::Compile {
            message: format!("unknown literal {SECRET_SHAPED_UNKNOWN_LITERAL}"),
            position: 17,
            line: 2,
            column: 6,
        });
        let safe_log = local_error.safe_log_message();
        assert!(!safe_log.contains(SECRET_SHAPED_UNKNOWN_LITERAL));
        assert!(safe_log.contains("line 2, column 6"));
        let error = local_timeline_app_error(local_error, "request-kq-compile");

        assert_eq!(error.code, AppErrorCode::Validation);
        assert!(!error.retryable);
        assert_eq!(error.message_key, "errors.kq_invalid_query");
        assert_eq!(
            error.safe_details.get("line").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            error.safe_details.get("column").map(String::as_str),
            Some("6")
        );
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains(SECRET_SHAPED_UNKNOWN_LITERAL));
    }

    #[test]
    fn kq_budget_error_is_a_non_retryable_query_validation_failure() {
        let error = local_timeline_app_error(
            LocalTimelineError::KrileQuery(KqTimelineError::Timeout {
                scanned_count: 30_000,
                max_scanned_rows: 25_000,
                max_duration_ms: 6_000,
            }),
            "request-kq-budget",
        );

        assert_eq!(error.code, AppErrorCode::Validation);
        assert!(!error.retryable);
        assert_eq!(error.message_key, "errors.kq_query_budget_exceeded");
        assert_eq!(
            error.safe_details.get("limit").map(String::as_str),
            Some("25000")
        );
    }

    #[test]
    fn kq_cancelled_error_keeps_the_cancelled_contract() {
        let error = local_timeline_app_error(
            LocalTimelineError::KrileQuery(KqTimelineError::Cancelled),
            "request-kq-cancelled",
        );

        assert_eq!(error.code, AppErrorCode::Cancelled);
        assert_eq!(error.message_key, "errors.cancelled");
        assert!(!error.retryable);
    }

    #[test]
    fn air_context_parses_the_notification_operation_timestamp() {
        let notification_created_at =
            parse_air_context_notification_created_at("2026-07-18T20:11:49+09:00").unwrap();

        assert_eq!(
            notification_created_at.to_rfc3339(),
            "2026-07-18T11:11:49+00:00"
        );
    }

    #[test]
    fn air_context_only_accepts_posts_strictly_after_the_notification_operation() {
        let target_post =
            parse_air_context_notification_created_at("2026-07-18T20:11:30Z").unwrap();
        let post_before_operation =
            parse_air_context_notification_created_at("2026-07-18T20:11:43Z").unwrap();
        let operation = parse_air_context_notification_created_at("2026-07-18T20:11:47Z").unwrap();
        let post_after_operation =
            parse_air_context_notification_created_at("2026-07-18T20:11:54Z").unwrap();

        assert!(!is_post_after_notification(&target_post, &operation));
        assert!(!is_post_after_notification(
            &post_before_operation,
            &operation
        ));
        assert!(!is_post_after_notification(&operation, &operation));
        assert!(is_post_after_notification(
            &post_after_operation,
            &operation
        ));
    }

    #[test]
    fn unified_timeline_types_do_not_require_an_account_source() {
        for column_type in ["home", "public", "notification"] {
            let request = TimelineRequest {
                operation_id: None,
                column_type: column_type.to_string(),
                column_param: None,
                account_acct: None,
                acting_account_acct: None,
                limit: None,
                offset: None,
                max_status_id: None,
                max_server_domain: None,
                since_status_id: None,
                since_server_domain: None,
                display_filter: None,
                quote_consumer_id: None,
            };
            assert!(TimelineType::from_column_config(
                &request.column_type,
                request.column_param.as_deref()
            )
            .is_some_and(|timeline_type| timeline_type.is_unified()));
            assert!(request.account_acct.is_none());
        }
    }

    #[test]
    fn legacy_account_binding_cannot_change_unified_classification() {
        for timeline_type in [
            TimelineType::Home,
            TimelineType::Public,
            TimelineType::Notification,
        ] {
            assert!(timeline_type.is_unified());
            let legacy_account_acct = Some("actor@example.test");
            assert!(legacy_account_acct.is_some());
            assert!(timeline_type.is_unified());
        }
        assert!(!TimelineType::Local.is_unified());
    }
}
