pub mod compose_outbox;
mod kq_evaluator;
pub mod kq_filter;
mod kq_source_plan;
pub mod kq_timeline;
pub mod reconnect_budget;
pub mod search_indexer;
pub mod startup_sync;
pub mod streaming_service;
pub mod timeline_service;
pub mod websocket_status;
pub mod yq_filter;
pub mod yq_timeline;

/// Shared with the search candidate filter so substring checks see the same
/// plain text as KQ `text` and the frontend display.
pub(crate) use kq_evaluator::html_to_plain_text;
