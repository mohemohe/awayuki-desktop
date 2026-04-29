use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    div, point, px, rgb, size, App, AsyncApp, AvailableSpace, Context, EventEmitter, FocusHandle,
    Focusable, IntoElement, Pixels, ScrollHandle, SharedString, Size, Timer, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::scroll::ScrollableElement;
use gpui_component::WindowExt;
use gpui_component::{v_virtual_list, IconName, VirtualListScrollHandle};
use gpui_tokio_bridge::Tokio;

use sqlx;

use crate::api::client::ApiClient;
use crate::db::models::DbStatus;
use crate::db::pool::Database;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::types::status::Status;
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::{self, TimelineType};
use crate::state::active_account::ActiveAccount;
use crate::state::appearance::{AppearanceSettings, DisplayMode};
use crate::state::confirmation::ConfirmationSettings;
use crate::state::notifications::NotificationSuppressionList;
use crate::state::performance::{PerformanceSettings, TimelineRenderer};
use crate::ui::components::status_item::{
    render_compact_status_item, render_status_item, EditTarget, EmojiMapping, QuoteDisplay,
    QuoteTarget, ReplyTarget, StatusItemData,
};
use crate::ui::workspace::ClosePanelRequest;

const DEFAULT_MAX_STATUSES: usize = 100;

/// Fill in `quote_display` for items that have a `quote_id`, by fetching quoted statuses from DB.
/// `db_statuses` is the original DbStatus slice used to resolve server_domain for each item.
async fn fill_quote_displays(
    items: &mut Vec<StatusItemData>,
    db_statuses: &[DbStatus],
    reader: &sqlx::SqlitePool,
) {
    use crate::mastodon::types::account::CustomEmoji;

    // Build a map from status_id to server_domain
    let domain_map: HashMap<String, String> = db_statuses
        .iter()
        .map(|s| (s.id.clone(), s.server_domain.clone()))
        .collect();

    for item in items.iter_mut() {
        let Some(ref qid) = item.quote_id else {
            continue;
        };
        let Some(server_domain) = domain_map.get(&item.id) else {
            continue;
        };
        let Ok(Some(q_status)) =
            crate::db::queries::statuses::get_status(reader, qid, server_domain).await
        else {
            continue;
        };
        let q_acc =
            crate::db::queries::accounts::get_account(reader, &q_status.account_id, server_domain)
                .await
                .ok()
                .flatten();

        let (display_name, acct, avatar_url) = if let Some(ref acc) = q_acc {
            (
                acc.display_name.clone(),
                format!("@{}", acc.acct),
                acc.avatar.clone(),
            )
        } else {
            (
                q_status.account_id.clone(),
                format!("@{}", q_status.account_id),
                String::new(),
            )
        };

        let account_emojis: Vec<CustomEmoji> = q_acc
            .as_ref()
            .and_then(|a| a.emojis_json.as_ref())
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        let status_emojis: Vec<CustomEmoji> = q_status
            .emojis_json
            .as_ref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        let emojis: Vec<EmojiMapping> = status_emojis
            .iter()
            .chain(account_emojis.iter())
            .map(|e| EmojiMapping {
                shortcode: e.shortcode.clone(),
                url: e.url.clone(),
            })
            .collect();

        item.quote_display = Some(QuoteDisplay {
            status_id: q_status.id.clone(),
            display_name: display_name.into(),
            acct: acct.into(),
            avatar_url: avatar_url.into(),
            content: q_status.content.clone().into(),
            url: q_status.url.clone(),
            emojis,
        });
    }
}

/// Inject OFFSET into a SQL query for pagination. Returns (modified_sql, page_size).
/// If `LIMIT N` exists at the end of the query, rewrite to `LIMIT N OFFSET offset`.
/// If no LIMIT, append `LIMIT default_limit OFFSET offset`.
fn inject_offset(sql: &str, default_limit: usize, offset: usize) -> (String, usize) {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let upper = trimmed.to_uppercase();
    if let Some(limit_pos) = upper.rfind("LIMIT") {
        let after_limit = trimmed[limit_pos + 5..].trim();
        if let Ok(n) = after_limit.parse::<usize>() {
            let base = &trimmed[..limit_pos];
            return (format!("{}LIMIT {} OFFSET {}", base, n, offset), n);
        }
    }
    (
        format!("{} LIMIT {} OFFSET {}", trimmed, default_limit, offset),
        default_limit,
    )
}

pub struct TimelinePanel {
    title: SharedString,
    timeline_type: TimelineType,
    max_statuses: usize,
    statuses: Vec<StatusItemData>,
    client: ApiClient,
    account_acct: String,
    account_id: String,
    /// Additional (non-primary) accounts whose Home/Federated/Notification
    /// timelines should be merged into this panel when unified-timeline is on.
    /// Empty when unified-timeline is off.
    extra_clients: Vec<(ApiClient, String)>,
    pending_poll_votes: HashMap<String, HashSet<usize>>,
    database: Arc<Database>,
    loading: bool,
    oldest_id: Option<String>,
    db_offset: usize,
    db_has_more: bool,
    expanded_cw: HashSet<String>,
    revealed_nsfw: HashSet<String>,
    expanded_statuses: HashSet<String>,
    retry_media: HashMap<String, u64>,
    image_refresh_task: Option<gpui::Task<()>>,
    focus_handle: FocusHandle,
    scroll_handle: VirtualListScrollHandle,
    list_scroll_handle: ScrollHandle,
    height_cache: HashMap<String, Pixels>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    last_measured_width: Option<Pixels>,
    is_closable: bool,
}

impl TimelinePanel {
    pub fn new(
        title: impl Into<SharedString>,
        timeline_type: TimelineType,
        client: ApiClient,
        account_acct: String,
        account_id: String,
        database: Arc<Database>,
        max_statuses: Option<u32>,
        extra_clients: Vec<(ApiClient, String)>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            title: title.into(),
            timeline_type,
            max_statuses: max_statuses
                .map(|v| v as usize)
                .unwrap_or(DEFAULT_MAX_STATUSES),
            statuses: Vec::new(),
            client,
            account_acct,
            account_id,
            extra_clients,
            database,
            loading: false,
            oldest_id: None,
            db_offset: 0,
            db_has_more: false,
            expanded_cw: HashSet::new(),
            revealed_nsfw: HashSet::new(),
            expanded_statuses: HashSet::new(),
            pending_poll_votes: HashMap::new(),
            retry_media: HashMap::new(),
            image_refresh_task: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: VirtualListScrollHandle::new(),
            list_scroll_handle: ScrollHandle::new(),
            height_cache: HashMap::new(),
            item_sizes: Rc::new(Vec::new()),
            last_measured_width: None,
            is_closable: false,
        };
        // Clear height cache when appearance settings change
        cx.observe_global::<AppearanceSettings>(|this: &mut TimelinePanel, cx| {
            this.height_cache.clear();
            this.last_measured_width = None;
            cx.notify();
        })
        .detach();

        // Refresh Bookmarks panel when bookmark state changes
        if matches!(panel.timeline_type, TimelineType::Bookmarks) {
            cx.observe_global::<BookmarkChanged>(|this: &mut TimelinePanel, cx| {
                this.fetch_bookmarks_from_db(false, cx);
            })
            .detach();
        }

        panel.load_initial(cx);
        panel
    }

    pub fn set_closable(&mut self, closable: bool) {
        self.is_closable = closable;
    }

    /// Returns the (client, status_uri) pair to use for an outgoing user
    /// action against `status_id`. Each panel is pinned to its primary
    /// account, but the action must execute on the active (action-source)
    /// account; the URI lets the caller resolve the remote post on that
    /// account's server when it's not the primary.
    fn action_target(&self, status_id: &str, cx: &App) -> (ApiClient, Option<String>) {
        let Some(active) = (if !self.extra_clients.is_empty() {
            cx.try_global::<ActiveAccount>().cloned()
        } else {
            None
        }) else {
            return (self.client.clone(), None);
        };

        // Active account == primary: use the primary client and the local id.
        if active.client.domain() == self.client.domain() {
            return (self.client.clone(), None);
        }

        // Cross-account action: the active account's server doesn't have
        // this status under the primary's id. Pull the URI so the caller
        // can resolve it via lookup_status_by_uri.
        let uri = self
            .statuses
            .iter()
            .find(|s| s.id == status_id)
            .map(|s| s.uri.clone())
            .filter(|u| !u.is_empty());
        (active.client, uri)
    }

    /// Build a `server_domain → acct` lookup from this panel's primary and
    /// extra sessions. Used when materialising DB-loaded statuses so each
    /// status's `source_acct` resolves to the session whose server actually
    /// hosts it — instead of the panel's primary acct, which would route a
    /// Bluesky DID or Misskey id to a Mastodon API and 404.
    ///
    /// In unified-timeline mode the same `server_domain` could in principle
    /// be served by multiple sessions (two accounts on the same Mastodon
    /// server). The primary is inserted first and `or_insert_with` keeps it,
    /// so we deterministically pick the panel's own acct when ambiguous.
    fn build_acct_by_domain(&self) -> HashMap<String, String> {
        let mut m: HashMap<String, String> = HashMap::with_capacity(1 + self.extra_clients.len());
        m.insert(self.client.domain().to_string(), self.account_acct.clone());
        for (c, acct) in &self.extra_clients {
            m.entry(c.domain().to_string())
                .or_insert_with(|| acct.clone());
        }
        m
    }

    fn load_initial(&mut self, cx: &mut Context<Self>) {
        match self.timeline_type {
            TimelineType::CustomSql(ref sql) => self.fetch_custom_sql(sql.clone(), false, cx),
            TimelineType::YukariQuery(ref q) => self.fetch_yq(q.clone(), false, cx),
            TimelineType::Bookmarks => self.fetch_bookmarks_from_db(false, cx),
            TimelineType::Notification => self.fetch_notifications(None, false, cx),
            _ => self.fetch_statuses(None, false, cx),
        }
    }

    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        match &self.timeline_type {
            TimelineType::Bookmarks => {
                if self.db_has_more {
                    self.fetch_bookmarks_from_db(true, cx);
                }
            }
            TimelineType::CustomSql(sql) => {
                if self.db_has_more {
                    self.fetch_custom_sql(sql.clone(), true, cx);
                }
            }
            TimelineType::YukariQuery(ref q) => {
                if self.db_has_more {
                    self.fetch_yq(q.clone(), true, cx);
                }
            }
            TimelineType::Notification => {
                if let Some(oldest_id) = self.oldest_id.clone() {
                    self.fetch_notifications(Some(oldest_id), true, cx);
                }
            }
            _ => {
                if let Some(oldest_id) = self.oldest_id.clone() {
                    self.fetch_statuses(Some(oldest_id), true, cx);
                }
            }
        }
    }

    fn fetch_bookmarks_from_db(&mut self, append: bool, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let database = self.database.clone();
        let client = self.client.clone();
        let primary_acct = self.account_acct.clone();
        let acct_by_domain = self.build_acct_by_domain();
        let offset = if append { self.db_offset } else { 0 };
        let limit = 40i64;

        let task = Tokio::spawn(cx, async move {
            let reader = database.reader();
            let server_domain = client.domain();

            let statuses = crate::db::queries::statuses::get_bookmarked_statuses(
                reader,
                server_domain,
                limit,
                offset as i64,
            )
            .await
            .map_err(|e| format!("DB error: {}", e))?;

            // Fetch accounts for display
            let account_keys: Vec<(String, String)> = statuses
                .iter()
                .map(|s| (s.account_id.clone(), s.server_domain.clone()))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let mut accounts = std::collections::HashMap::new();
            for (account_id, server_domain) in &account_keys {
                if let Ok(Some(acc)) =
                    crate::db::queries::accounts::get_account(reader, account_id, server_domain)
                        .await
                {
                    accounts.insert(acc.id.clone(), acc);
                }
            }

            let mut items: Vec<StatusItemData> = statuses
                .iter()
                .map(|s| {
                    let acc = accounts.get(&s.account_id);
                    let src = acct_by_domain
                        .get(&s.server_domain)
                        .cloned()
                        .unwrap_or_else(|| primary_acct.clone());
                    StatusItemData::from_db(s, acc, &src)
                })
                .collect();

            fill_quote_displays(&mut items, &statuses, reader).await;

            Ok::<Vec<StatusItemData>, String>(items)
        });

        let page_size = limit as usize;
        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(items)) => {
                    tracing::info!("Loaded {} bookmarked statuses from DB", items.len());
                    let fetched_count = items.len();
                    let _ = this.update(cx, |this, cx| {
                        if append {
                            this.statuses.extend(items);
                        } else {
                            this.statuses = items;
                        }
                        this.db_offset = this.statuses.len();
                        this.db_has_more = fetched_count == page_size;
                        this.loading = false;
                        this.schedule_image_refresh(cx);
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Bookmarks DB fetch failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Bookmarks task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn fetch_custom_sql(&mut self, sql: String, append: bool, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let database = self.database.clone();
        let primary_acct = self.account_acct.clone();
        let acct_by_domain = self.build_acct_by_domain();
        let offset = if append { self.db_offset } else { 0 };
        let (paginated_sql, page_size) = inject_offset(&sql, self.max_statuses, offset);

        let task = Tokio::spawn(cx, async move {
            let reader = database.reader();

            // Execute paginated SQL to get statuses
            let statuses: Vec<DbStatus> = sqlx::query_as(&paginated_sql)
                .fetch_all(reader)
                .await
                .map_err(|e| format!("SQL error: {}", e))?;

            // Collect unique (account_id, server_domain) and fetch accounts
            let account_keys: Vec<(String, String)> = statuses
                .iter()
                .map(|s| (s.account_id.clone(), s.server_domain.clone()))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let mut accounts = std::collections::HashMap::new();
            for (account_id, server_domain) in &account_keys {
                if let Ok(Some(acc)) =
                    crate::db::queries::accounts::get_account(reader, account_id, server_domain)
                        .await
                {
                    accounts.insert(acc.id.clone(), acc);
                }
            }

            let mut items: Vec<StatusItemData> = statuses
                .iter()
                .map(|s| {
                    let acc = accounts.get(&s.account_id);
                    let src = acct_by_domain
                        .get(&s.server_domain)
                        .cloned()
                        .unwrap_or_else(|| primary_acct.clone());
                    StatusItemData::from_db(s, acc, &src)
                })
                .collect();

            fill_quote_displays(&mut items, &statuses, reader).await;

            Ok::<Vec<StatusItemData>, String>(items)
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(items)) => {
                    tracing::info!("Custom SQL returned {} statuses", items.len());
                    let fetched_count = items.len();
                    let _ = this.update(cx, |this, cx| {
                        if append {
                            this.statuses.extend(items);
                        } else {
                            this.statuses = items;
                        }
                        this.db_offset = this.statuses.len();
                        this.db_has_more = fetched_count == page_size;
                        this.loading = false;
                        this.schedule_image_refresh(cx);
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Custom SQL failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn fetch_yq(&mut self, query_str: String, append: bool, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let initial_offset = if append { self.db_offset } else { 0 };
        let desired_count = self.max_statuses;

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                let task = this.update(cx, |this, cx| {
                    let database = this.database.clone();
                    let primary_acct = this.account_acct.clone();
                    let acct_by_domain = this.build_acct_by_domain();
                    let query = query_str.clone();
                    Tokio::spawn(cx, async move {
                        use futures::StreamExt as _;
                        let reader = database.reader();

                        // Stream rows one at a time so that non-matching rows are
                        // dropped immediately and never accumulate in memory.
                        let sql = if initial_offset > 0 {
                            format!(
                                "SELECT * FROM statuses ORDER BY created_at DESC \
                                 LIMIT -1 OFFSET {}",
                                initial_offset
                            )
                        } else {
                            "SELECT * FROM statuses ORDER BY created_at DESC".to_string()
                        };

                        let mut stream = sqlx::query_as::<_, DbStatus>(&sql).fetch(reader);
                        let mut account_cache: std::collections::HashMap<
                            String,
                            crate::db::models::DbAccount,
                        > = std::collections::HashMap::new();
                        let mut matched_statuses: Vec<DbStatus> = Vec::new();
                        let mut matched_accounts: Vec<Option<crate::db::models::DbAccount>> =
                            Vec::new();
                        let mut rows_scanned: usize = 0;
                        let mut stream_exhausted = true;

                        while let Some(row_result) = stream.next().await {
                            let status = row_result.map_err(|e| format!("SQL error: {}", e))?;
                            rows_scanned += 1;

                            // Lazily fetch and cache accounts
                            let acc_key = format!("{}:{}", status.account_id, status.server_domain);
                            if !account_cache.contains_key(&acc_key) {
                                if let Ok(Some(acc)) = crate::db::queries::accounts::get_account(
                                    reader,
                                    &status.account_id,
                                    &status.server_domain,
                                )
                                .await
                                {
                                    account_cache.insert(acc_key.clone(), acc);
                                }
                            }
                            let acc = account_cache.get(&acc_key);

                            if crate::services::yq_filter::matches_status(&query, &status, acc) {
                                matched_accounts.push(acc.cloned());
                                matched_statuses.push(status);
                                if matched_statuses.len() >= desired_count {
                                    stream_exhausted = false;
                                    break;
                                }
                            }
                            // `status` dropped here for non-matches — no memory retained
                        }

                        tracing::info!(
                            "YQ fetch: {} matches, {} rows scanned",
                            matched_statuses.len(),
                            rows_scanned,
                        );

                        let mut items: Vec<StatusItemData> = matched_statuses
                            .iter()
                            .zip(matched_accounts.iter())
                            .map(|(s, acc)| {
                                let src = acct_by_domain
                                    .get(&s.server_domain)
                                    .cloned()
                                    .unwrap_or_else(|| primary_acct.clone());
                                StatusItemData::from_db(s, acc.as_ref(), &src)
                            })
                            .collect();

                        fill_quote_displays(&mut items, &matched_statuses, reader).await;

                        let new_offset = initial_offset + rows_scanned;
                        let has_more = !stream_exhausted;
                        Ok::<(Vec<StatusItemData>, usize, bool), String>((
                            items, new_offset, has_more,
                        ))
                    })
                });

                let Ok(task) = task else { return };

                match task.await {
                    Ok(Ok((items, new_offset, has_more))) => {
                        let _ = this.update(cx, |this, cx| {
                            if append {
                                this.statuses.extend(items);
                            } else {
                                this.statuses = items;
                            }
                            this.statuses.truncate(desired_count);
                            this.db_offset = new_offset;
                            this.db_has_more = has_more;
                            this.loading = false;
                            this.prune_interaction_sets();
                            this.schedule_image_refresh(cx);
                            cx.notify();
                        });
                    }
                    Ok(Err(e)) => {
                        tracing::error!("YQ fetch failed: {}", e);
                        let _ = this.update(cx, |this, cx| {
                            this.loading = false;
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!("YQ task error: {}", e);
                        let _ = this.update(cx, |this, cx| {
                            this.loading = false;
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    fn fetch_statuses(&mut self, max_id: Option<String>, append: bool, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let database = self.database.clone();
        let account_acct = self.account_acct.clone();
        let tl_type = self.timeline_type.clone();
        // Pagination uses primary account only — `max_id` is server-specific.
        let extra_clients = if max_id.is_some() {
            Vec::new()
        } else {
            self.extra_clients.clone()
        };
        let params = TimelineParams {
            max_id,
            ..TimelineParams::default()
        };

        let task = Tokio::spawn(cx, async move {
            // Fetch from primary account
            let primary_statuses = timeline_service::fetch_from_api(&client, &tl_type, &params)
                .await
                .map_err(|e| e.to_string())?;

            // Save primary results to DB (server + account + status + timeline_entry)
            let server_domain = client.domain().to_string();
            let tl_key = tl_type.as_str();
            for status in &primary_statuses {
                if let Err(e) =
                    timeline_service::save_status_to_db(database.writer(), status, &server_domain)
                        .await
                {
                    tracing::warn!("Failed to save status {} to DB: {}", status.id, e);
                }
                if let Err(e) = crate::db::queries::timeline::insert_timeline_entry(
                    database.writer(),
                    &tl_key,
                    &server_domain,
                    &status.id,
                    &account_acct,
                    &status.created_at.to_rfc3339(),
                )
                .await
                {
                    tracing::warn!("Failed to insert timeline entry: {}", e);
                }
            }

            // Fetch from extra accounts (unified-timeline mode) and dedup by uri.
            // These additional results bypass timeline_entries (which are per-account)
            // and are merged in-memory only for display.
            let mut extra_results: Vec<(Status, String, String)> = Vec::new();
            for (extra_client, extra_acct) in &extra_clients {
                let extra_params = TimelineParams::default();
                match timeline_service::fetch_from_api(extra_client, &tl_type, &extra_params).await
                {
                    Ok(extras) => {
                        let extra_domain = extra_client.domain().to_string();
                        for status in &extras {
                            if let Err(e) = timeline_service::save_status_to_db(
                                database.writer(),
                                status,
                                &extra_domain,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "Failed to save unified status {} to DB: {}",
                                    status.id,
                                    e
                                );
                            }
                        }
                        extra_results.extend(
                            extras
                                .into_iter()
                                .map(|s| (s, extra_acct.clone(), extra_domain.clone())),
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Unified timeline fetch failed for {}: {}", extra_acct, e);
                    }
                }
            }

            let primary_with_acct: Vec<(Status, String, String)> = primary_statuses
                .into_iter()
                .map(|s| (s, account_acct.clone(), server_domain.clone()))
                .collect();

            Ok::<(Vec<(Status, String, String)>, Vec<(Status, String, String)>), String>((
                primary_with_acct,
                extra_results,
            ))
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok((primary, extras))) => {
                    tracing::info!(
                        "Fetched {} statuses from primary, {} from extras",
                        primary.len(),
                        extras.len()
                    );
                    let _ = this.update(cx, |this, cx| {
                        if let Some(last) = primary.last() {
                            this.oldest_id = Some(last.0.id.clone());
                        }

                        // Merge primary + extras, sort by created_at desc,
                        // dedup by the *event*'s URI. For a primary post that
                        // arrives via multiple home streams the URI matches and
                        // collapses; for boosts the wrapper URI is per-booster
                        // so independent boosts of the same post stay separate.
                        let mut combined: Vec<(Status, String, String)> = primary;
                        combined.extend(extras);
                        combined.sort_by(|a, b| b.0.created_at.cmp(&a.0.created_at));
                        let mut seen_uris: HashSet<String> = HashSet::new();
                        combined.retain(|(s, _, _)| {
                            if s.uri.is_empty() {
                                true
                            } else {
                                seen_uris.insert(s.uri.clone())
                            }
                        });

                        let items: Vec<StatusItemData> = combined
                            .iter()
                            .map(|(s, src, dom)| StatusItemData::from_status(s, src, dom))
                            .collect();
                        if append {
                            this.statuses.extend(items);
                            // Re-dedup after extending — load_more may bring
                            // back items already present from streaming.
                            let mut seen: HashSet<String> = HashSet::new();
                            this.statuses.retain(|s| {
                                if s.uri.is_empty() {
                                    true
                                } else {
                                    seen.insert(s.uri.clone())
                                }
                            });
                        } else {
                            this.statuses = items;
                        }
                        this.statuses.truncate(this.max_statuses);
                        this.prune_interaction_sets();
                        this.loading = false;
                        this.schedule_image_refresh(cx);
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Timeline fetch failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn fetch_notifications(
        &mut self,
        max_id: Option<String>,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let primary_acct = self.account_acct.clone();
        // Notifications pagination uses primary account only — `max_id` is server-specific.
        let extra_clients = if max_id.is_some() {
            Vec::new()
        } else {
            self.extra_clients.clone()
        };
        let params = NotificationParams {
            max_id,
            ..NotificationParams::default()
        };

        let primary_domain = self.client.domain().to_string();
        let task = Tokio::spawn(cx, async move {
            let primary = client
                .get_notifications(&params)
                .await
                .map_err(|e| e.to_string())?;

            let mut extras: Vec<(
                crate::mastodon::types::notification::Notification,
                String,
                String,
            )> = Vec::new();
            for (extra_client, extra_acct) in &extra_clients {
                let extra_params = NotificationParams::default();
                let extra_domain = extra_client.domain().to_string();
                match extra_client.get_notifications(&extra_params).await {
                    Ok(list) => extras.extend(
                        list.into_iter()
                            .map(|n| (n, extra_acct.clone(), extra_domain.clone())),
                    ),
                    Err(e) => {
                        tracing::warn!(
                            "Unified notification fetch failed for {}: {}",
                            extra_acct,
                            e
                        );
                    }
                }
            }

            let primary_with_acct: Vec<(
                crate::mastodon::types::notification::Notification,
                String,
                String,
            )> = primary
                .into_iter()
                .map(|n| (n, primary_acct.clone(), primary_domain.clone()))
                .collect();

            Ok::<
                (
                    Vec<(
                        crate::mastodon::types::notification::Notification,
                        String,
                        String,
                    )>,
                    Vec<(
                        crate::mastodon::types::notification::Notification,
                        String,
                        String,
                    )>,
                ),
                String,
            >((primary_with_acct, extras))
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok((primary, extras))) => {
                    tracing::info!(
                        "Fetched {} primary notifications, {} extra notifications",
                        primary.len(),
                        extras.len()
                    );
                    let _ = this.update(cx, |this, cx| {
                        if let Some(last) = primary.last() {
                            this.oldest_id = Some(last.0.id.clone());
                        }

                        // Merge primary + extras, sort by created_at desc,
                        // de-dup by the notification's status URI when
                        // available. Boost notifications carry the wrapper URI,
                        // which is independent per-booster — so B's and C's
                        // boost notifications of the same post stay separate,
                        // but the same notification reaching us twice collapses.
                        let mut combined = primary;
                        combined.extend(extras);
                        combined.sort_by(|a, b| b.0.created_at.cmp(&a.0.created_at));
                        let mut seen_uris: HashSet<String> = HashSet::new();
                        combined.retain(|(n, _, _)| match n.status.as_ref() {
                            Some(s) if !s.uri.is_empty() => seen_uris.insert(s.uri.clone()),
                            _ => true,
                        });

                        let items: Vec<StatusItemData> = combined
                            .iter()
                            .map(|(n, src, dom)| StatusItemData::from_notification(n, src, dom))
                            .collect();
                        if append {
                            this.statuses.extend(items);
                        } else {
                            this.statuses = items;
                        }
                        this.statuses.truncate(this.max_statuses);
                        this.loading = false;
                        this.schedule_image_refresh(cx);
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Notification fetch failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn refresh_poll(&mut self, poll_id: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let pid = poll_id.clone();

        let task = Tokio::spawn(cx, async move {
            client.get_poll(&pid).await.map_err(|e| e.to_string())
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_poll)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this
                            .statuses
                            .iter_mut()
                            .find(|s| s.poll.as_ref().map(|p| p.id == poll_id).unwrap_or(false))
                        {
                            item.poll = Some(updated_poll);
                        }
                        this.height_cache.clear();
                        cx.notify();
                    });
                }
                Ok(Err(e)) => tracing::error!("Poll refresh failed: {}", e),
                Err(e) => tracing::error!("Poll refresh task error: {}", e),
            },
        )
        .detach();
    }

    fn vote_poll(&mut self, poll_id: String, choices: Vec<usize>, cx: &mut Context<Self>) {
        // Find the status that owns this poll, to resolve the URI when the
        // active (action-source) account differs from the panel's primary.
        let owning_status_id = self
            .statuses
            .iter()
            .find(|s| s.poll.as_ref().map(|p| p.id == poll_id).unwrap_or(false))
            .map(|s| s.id.clone())
            .unwrap_or_default();

        let (client, lookup_uri) = self.action_target(&owning_status_id, cx);
        let pid = poll_id.clone();
        let params = crate::mastodon::endpoints::statuses::VotePollParams {
            choices: choices.iter().map(|&c| c as i64).collect(),
        };

        let task = Tokio::spawn(cx, async move {
            let target_poll_id = match lookup_uri {
                Some(uri) => match client.lookup_status_by_uri(&uri).await {
                    Ok(Some(s)) => match s.poll {
                        Some(p) => p.id,
                        None => return Err(format!("Resolved status has no poll: {}", uri)),
                    },
                    Ok(None) => return Err(format!("Could not resolve {} on active account", uri)),
                    Err(e) => return Err(format!("URI lookup failed: {}", e)),
                },
                None => pid,
            };
            client
                .vote_poll(&target_poll_id, &params)
                .await
                .map_err(|e| e.to_string())
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_poll)) => {
                    let _ = this.update(cx, |this, cx| {
                        // Find the status containing this poll and update it
                        if let Some(item) = this
                            .statuses
                            .iter_mut()
                            .find(|s| s.poll.as_ref().map(|p| p.id == poll_id).unwrap_or(false))
                        {
                            item.poll = Some(updated_poll);
                        }
                        this.pending_poll_votes.remove(&poll_id);
                        this.height_cache.clear();
                        cx.notify();
                    });
                }
                Ok(Err(e)) => tracing::error!("Poll vote failed: {}", e),
                Err(e) => tracing::error!("Poll vote task error: {}", e),
            },
        )
        .detach();
    }

    fn toggle_reblog(&mut self, status_id: String, cx: &mut Context<Self>) {
        let item = self.statuses.iter().find(|s| s.id == status_id);
        let currently_reblogged = item.map(|s| s.reblogged).unwrap_or(false);
        let api_id = item
            .map(|s| s.original_status_id.clone())
            .unwrap_or_else(|| status_id.clone());

        let (client, lookup_uri) = self.action_target(&status_id, cx);

        let task = Tokio::spawn(cx, async move {
            // If acting on a remote post via a different account, resolve
            // the URI to a local id on that account's server first.
            let target_id = match lookup_uri {
                Some(uri) => match client.lookup_status_by_uri(&uri).await {
                    Ok(Some(s)) => s.id,
                    Ok(None) => return Err(format!("Could not resolve {} on active account", uri)),
                    Err(e) => return Err(format!("URI lookup failed: {}", e)),
                },
                None => api_id,
            };
            if currently_reblogged {
                client.unreblog(&target_id).await.map_err(|e| e.to_string())
            } else {
                client.reblog(&target_id).await.map_err(|e| e.to_string())
            }
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.reblogged =
                                updated_status.reblogged.unwrap_or(!currently_reblogged);
                            item.reblogs_count = updated_status.reblogs_count;
                            cx.notify();
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Reblog toggle failed: {}", e),
                Err(e) => tracing::error!("Reblog task error: {}", e),
            },
        )
        .detach();
    }

    fn toggle_favourite(&mut self, status_id: String, cx: &mut Context<Self>) {
        let item = self.statuses.iter().find(|s| s.id == status_id);
        let currently_favourited = item.map(|s| s.favourited).unwrap_or(false);
        let api_id = item
            .map(|s| s.original_status_id.clone())
            .unwrap_or_else(|| status_id.clone());

        let (client, lookup_uri) = self.action_target(&status_id, cx);

        let task = Tokio::spawn(cx, async move {
            let target_id = match lookup_uri {
                Some(uri) => match client.lookup_status_by_uri(&uri).await {
                    Ok(Some(s)) => s.id,
                    Ok(None) => return Err(format!("Could not resolve {} on active account", uri)),
                    Err(e) => return Err(format!("URI lookup failed: {}", e)),
                },
                None => api_id,
            };
            if currently_favourited {
                client
                    .unfavourite(&target_id)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                client
                    .favourite(&target_id)
                    .await
                    .map_err(|e| e.to_string())
            }
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.favourited =
                                updated_status.favourited.unwrap_or(!currently_favourited);
                            item.favourites_count = updated_status.favourites_count;
                            cx.notify();
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Favourite toggle failed: {}", e),
                Err(e) => tracing::error!("Favourite task error: {}", e),
            },
        )
        .detach();
    }

    fn toggle_bookmark(&mut self, status_id: String, cx: &mut Context<Self>) {
        let item = self.statuses.iter().find(|s| s.id == status_id);
        let currently_bookmarked = item.map(|s| s.bookmarked).unwrap_or(false);
        let api_id = item
            .map(|s| s.original_status_id.clone())
            .unwrap_or_else(|| status_id.clone());

        let (client, lookup_uri) = self.action_target(&status_id, cx);
        let database = self.database.clone();

        let task = Tokio::spawn(cx, async move {
            let target_id = match lookup_uri {
                Some(uri) => match client.lookup_status_by_uri(&uri).await {
                    Ok(Some(s)) => s.id,
                    Ok(None) => return Err(format!("Could not resolve {} on active account", uri)),
                    Err(e) => return Err(format!("URI lookup failed: {}", e)),
                },
                None => api_id,
            };
            let updated_status = if currently_bookmarked {
                client
                    .unbookmark(&target_id)
                    .await
                    .map_err(|e| e.to_string())?
            } else {
                client
                    .bookmark(&target_id)
                    .await
                    .map_err(|e| e.to_string())?
            };

            // Save the updated status to DB so Bookmarks timeline can pick it up
            let server_domain = client.domain();
            timeline_service::save_status_to_db(database.writer(), &updated_status, server_domain)
                .await
                .map_err(|e| e.to_string())?;

            Ok::<Status, String>(updated_status)
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.bookmarked =
                                updated_status.bookmarked.unwrap_or(!currently_bookmarked);
                            cx.notify();
                        }
                        // Notify Bookmarks panels to refresh
                        let version = cx
                            .try_global::<BookmarkChanged>()
                            .map(|s| s.version)
                            .unwrap_or(0);
                        cx.set_global(BookmarkChanged {
                            version: version + 1,
                        });
                    });
                }
                Ok(Err(e)) => tracing::error!("Bookmark toggle failed: {}", e),
                Err(e) => tracing::error!("Bookmark task error: {}", e),
            },
        )
        .detach();
    }

    /// Measure heights of statuses that are not yet cached.
    /// Uses `layout_as_root()` for off-screen measurement (same pattern as VirtualList::measure_item).
    fn measure_status_heights(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.last_measured_width.unwrap_or(px(350.0));
        let display_mode = cx.global::<AppearanceSettings>().display_mode;
        for status in &self.statuses {
            let key = self.height_cache_key(&status.id);
            if self.height_cache.contains_key(&key) {
                continue;
            }
            let cw_expanded = self.expanded_cw.contains(&status.id);
            let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
            let empty_retry = HashMap::new();
            let mut element = match display_mode {
                DisplayMode::Mystique => {
                    let mystique_expanded = self.expanded_statuses.contains(&status.id);
                    render_compact_status_item(
                        status,
                        mystique_expanded,
                        None,
                        cw_expanded,
                        nsfw_revealed,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        &empty_retry,
                        window,
                        cx,
                    )
                }
                DisplayMode::StarryEyes => render_status_item(
                    status,
                    cw_expanded,
                    nsfw_revealed,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    &empty_retry,
                    window,
                    cx,
                ),
            };
            let measured = element.layout_as_root(
                size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                window,
                cx,
            );
            self.height_cache.insert(key, measured.height);
        }
    }

    fn height_cache_key(&self, id: &str) -> String {
        let mut key = id.to_string();
        if self.expanded_cw.contains(id) {
            key.push_str("-cw");
        }
        if self.expanded_statuses.contains(id) {
            key.push_str("-exp");
        }
        key
    }

    fn rebuild_item_sizes(&mut self) {
        let sizes: Vec<Size<Pixels>> = self
            .statuses
            .iter()
            .map(|status| {
                let key = self.height_cache_key(&status.id);
                let height = self.height_cache.get(&key).copied().unwrap_or(px(100.0));
                size(px(0.0), height)
            })
            .collect();
        self.item_sizes = Rc::new(sizes);
    }

    fn cleanup_height_cache(&mut self) {
        let valid_ids: HashSet<&str> = self.statuses.iter().map(|s| s.id.as_str()).collect();
        self.height_cache.retain(|key, _| {
            // Extract base ID by stripping known suffixes
            let base = key
                .strip_suffix("-cw-exp")
                .or_else(|| key.strip_suffix("-cw"))
                .or_else(|| key.strip_suffix("-exp"))
                .unwrap_or(key);
            valid_ids.contains(base)
        });
    }

    fn invalidate_height_cache(&mut self, status_id: &str) {
        let id = status_id.to_string();
        self.height_cache.remove(&id);
        self.height_cache.remove(&format!("{}-cw", id));
        self.height_cache.remove(&format!("{}-exp", id));
        self.height_cache.remove(&format!("{}-cw-exp", id));
    }

    /// Remove entries from interaction-tracking sets that no longer correspond
    /// to any status visible in the panel.
    fn prune_interaction_sets(&mut self) {
        let ids: std::collections::HashSet<&str> =
            self.statuses.iter().map(|s| s.id.as_str()).collect();
        self.expanded_cw.retain(|id| ids.contains(id.as_str()));
        self.revealed_nsfw.retain(|id| ids.contains(id.as_str()));
        self.expanded_statuses
            .retain(|id| ids.contains(id.as_str()));
        self.retry_media.retain(|id, _| ids.contains(id.as_str()));
    }

    /// Schedule delayed re-renders to pick up images that finished loading
    /// via other panels' asset requests (works around GPUI's use_asset()
    /// notification limitation in multi-column layouts).
    fn schedule_image_refresh(&mut self, cx: &mut Context<Self>) {
        self.image_refresh_task.take();
        self.image_refresh_task = Some(cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                Timer::after(Duration::from_millis(300)).await;
                let _ = this.update(cx, |_, cx| {
                    cx.notify();
                });
                Timer::after(Duration::from_millis(1200)).await;
                let _ = this.update(cx, |this, cx| {
                    this.image_refresh_task = None;
                    cx.notify();
                });
            },
        ));
    }

    /// Start receiving streaming events and prepend new statuses.
    /// Events are filtered based on whether the stream type matches this panel's timeline type.
    /// For CustomSql panels, the SQL query is re-executed and only re-rendered if results change.
    pub fn start_streaming(
        &mut self,
        receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        cx: &mut Context<Self>,
    ) {
        let timeline_type = self.timeline_type.clone();

        match timeline_type {
            TimelineType::CustomSql(sql) => {
                self.start_streaming_custom_sql(receiver, sql, cx);
            }
            TimelineType::YukariQuery(q) => {
                self.start_streaming_yq(receiver, q, cx);
            }
            _ => {
                self.start_streaming_standard(receiver, timeline_type, cx);
            }
        }
    }

    fn start_streaming_standard(
        &mut self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        timeline_type: TimelineType,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                while let Some(event) = receiver.next().await {
                    if this
                        .update(cx, |this, cx| match event {
                            TimelineEvent::NewStatus(
                                status,
                                ref stream_type,
                                source_acct,
                                server_domain,
                            ) => {
                                if timeline_type.matches_stream_type(stream_type) {
                                    // In unified-timeline mode the same event
                                    // may arrive from multiple account streams.
                                    // Drop duplicates by the event's URI: a
                                    // primary post received twice collapses,
                                    // while independent boosts of the same
                                    // post (each with its own wrapper URI)
                                    // stay separate.
                                    let already_displayed = !status.uri.is_empty()
                                        && this.statuses.iter().any(|s| s.uri == status.uri);
                                    if !already_displayed {
                                        let item = StatusItemData::from_status(
                                            &status,
                                            &source_acct,
                                            &server_domain,
                                        );
                                        this.statuses.insert(0, item);
                                        this.statuses.truncate(this.max_statuses);
                                        this.prune_interaction_sets();
                                        this.schedule_image_refresh(cx);
                                        cx.notify();
                                    }
                                }
                            }
                            TimelineEvent::StatusUpdate(status, source_acct, server_domain) => {
                                let item = StatusItemData::from_status(
                                    &status,
                                    &source_acct,
                                    &server_domain,
                                );
                                if let Some(pos) =
                                    this.statuses.iter().position(|s| s.id == status.id)
                                {
                                    this.invalidate_height_cache(&status.id);
                                    this.statuses[pos] = item;
                                    cx.notify();
                                }
                            }
                            TimelineEvent::DeleteStatus(id, _source_acct, _server_domain) => {
                                this.invalidate_height_cache(&id);
                                this.expanded_cw.remove(&id);
                                this.revealed_nsfw.remove(&id);
                                this.expanded_statuses.remove(&id);
                                this.retry_media.remove(&id);
                                this.statuses.retain(|s| s.id != id);
                                cx.notify();
                            }
                            TimelineEvent::NewNotification(
                                notification,
                                _,
                                source_acct,
                                server_domain,
                            ) => {
                                if matches!(timeline_type, TimelineType::Notification) {
                                    // De-dup by the notification's status URI:
                                    // independent boosts/favourites of the same
                                    // post each have a distinct wrapper URI,
                                    // so they stay separate; only the literal
                                    // same notification reaching us twice
                                    // collapses.
                                    let already_displayed = notification
                                        .status
                                        .as_ref()
                                        .map(|s| {
                                            !s.uri.is_empty()
                                                && this.statuses.iter().any(|x| x.uri == s.uri)
                                        })
                                        .unwrap_or(false);
                                    if !already_displayed {
                                        let item = StatusItemData::from_notification(
                                            &notification,
                                            &source_acct,
                                            &server_domain,
                                        );
                                        this.statuses.insert(0, item);
                                        this.statuses.truncate(this.max_statuses);
                                        this.prune_interaction_sets();
                                        let suppressed = cx
                                            .try_global::<NotificationSuppressionList>()
                                            .map(|list| {
                                                list.is_suppressed(&notification.account.acct)
                                            })
                                            .unwrap_or(false);
                                        if !suppressed {
                                            streaming_service::send_desktop_notification(
                                                &notification,
                                            );
                                        }
                                        this.schedule_image_refresh(cx);
                                        cx.notify();
                                    }
                                }
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            },
        )
        .detach();
    }

    fn start_streaming_custom_sql(
        &mut self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        sql: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                while let Some(_event) = receiver.next().await {
                    // Spawn SQL query on tokio runtime via entity context
                    let query = sql.clone();
                    let task = this.update(cx, |this, cx| {
                        let database = this.database.clone();
                        let primary_acct = this.account_acct.clone();
                        let acct_by_domain = this.build_acct_by_domain();
                        Tokio::spawn(cx, async move {
                            let reader = database.reader();
                            let statuses: Vec<DbStatus> = sqlx::query_as(&query)
                                .fetch_all(reader)
                                .await
                                .map_err(|e| format!("SQL error: {}", e))?;

                            let account_keys: Vec<(String, String)> = statuses
                                .iter()
                                .map(|s| (s.account_id.clone(), s.server_domain.clone()))
                                .collect::<std::collections::HashSet<_>>()
                                .into_iter()
                                .collect();

                            let mut accounts = std::collections::HashMap::new();
                            for (account_id, server_domain) in &account_keys {
                                if let Ok(Some(acc)) = crate::db::queries::accounts::get_account(
                                    reader,
                                    account_id,
                                    server_domain,
                                )
                                .await
                                {
                                    accounts.insert(acc.id.clone(), acc);
                                }
                            }

                            let mut items: Vec<StatusItemData> = statuses
                                .iter()
                                .map(|s| {
                                    let acc = accounts.get(&s.account_id);
                                    let src = acct_by_domain
                                        .get(&s.server_domain)
                                        .cloned()
                                        .unwrap_or_else(|| primary_acct.clone());
                                    StatusItemData::from_db(s, acc, &src)
                                })
                                .collect();

                            fill_quote_displays(&mut items, &statuses, reader).await;

                            Ok::<Vec<StatusItemData>, String>(items)
                        })
                    });

                    let Ok(task) = task else { return };

                    match task.await {
                        Ok(Ok(new_items)) => {
                            if this
                                .update(cx, |this, cx| {
                                    // Only re-render if the ID list has changed
                                    let old_ids: Vec<&str> =
                                        this.statuses.iter().map(|s| s.id.as_str()).collect();
                                    let new_ids: Vec<&str> =
                                        new_items.iter().map(|s| s.id.as_str()).collect();
                                    if old_ids != new_ids {
                                        this.statuses = new_items;
                                        this.schedule_image_refresh(cx);
                                        cx.notify();
                                    }
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("Custom SQL re-query failed: {}", e);
                        }
                        Err(e) => {
                            tracing::warn!("Custom SQL task error: {}", e);
                        }
                    }
                }
            },
        )
        .detach();
    }

    fn start_streaming_yq(
        &mut self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        query_str: String,
        cx: &mut Context<Self>,
    ) {
        let max_statuses = self.max_statuses;
        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                while let Some(event) = receiver.next().await {
                    if this
                        .update(cx, |this, cx| match event {
                            TimelineEvent::NewStatus(
                                status,
                                _stream_type,
                                source_acct,
                                server_domain,
                            ) => {
                                let db_status =
                                    crate::db::models::DbStatus::from_api(&status, &server_domain);
                                let db_account = crate::db::models::DbAccount::from_api(
                                    &status.account,
                                    &server_domain,
                                );

                                if crate::services::yq_filter::matches_status(
                                    &query_str,
                                    &db_status,
                                    Some(&db_account),
                                ) {
                                    let item = StatusItemData::from_status(
                                        &status,
                                        &source_acct,
                                        &server_domain,
                                    );
                                    this.statuses.insert(0, item);
                                    this.statuses.truncate(max_statuses);
                                    this.prune_interaction_sets();
                                    this.schedule_image_refresh(cx);
                                    cx.notify();
                                }
                            }
                            TimelineEvent::StatusUpdate(status, source_acct, server_domain) => {
                                let item = StatusItemData::from_status(
                                    &status,
                                    &source_acct,
                                    &server_domain,
                                );
                                if let Some(pos) =
                                    this.statuses.iter().position(|s| s.id == status.id)
                                {
                                    this.invalidate_height_cache(&status.id);
                                    this.statuses[pos] = item;
                                    cx.notify();
                                }
                            }
                            TimelineEvent::DeleteStatus(id, _source_acct, _server_domain) => {
                                this.invalidate_height_cache(&id);
                                this.expanded_cw.remove(&id);
                                this.revealed_nsfw.remove(&id);
                                this.expanded_statuses.remove(&id);
                                this.retry_media.remove(&id);
                                this.statuses.retain(|s| s.id != id);
                                cx.notify();
                            }
                            TimelineEvent::NewNotification(_, _, _, _) => {}
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            },
        )
        .detach();
    }
}

impl Drop for TimelinePanel {
    fn drop(&mut self) {
        tracing::info!(
            "TimelinePanel dropped: {:?} ({} statuses, {} height_cache entries)",
            self.timeline_type,
            self.statuses.len(),
            self.height_cache.len(),
        );
    }
}

impl EventEmitter<PanelEvent> for TimelinePanel {}

impl Focusable for TimelinePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TimelinePanel {
    fn panel_name(&self) -> &'static str {
        "TimelinePanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }

    fn closable(&self, _cx: &App) -> bool {
        self.is_closable
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let scroll_handle = self.scroll_handle.clone();
        let mut buttons = vec![Button::new("scroll-to-top")
            .icon(IconName::ArrowUp)
            .on_click(move |_event, _window, _cx| {
                scroll_handle.set_offset(point(px(0.), px(0.)));
            })];
        if self.is_closable {
            let entity_id = cx.entity().entity_id();
            buttons.push(Button::new("close-panel").icon(IconName::Close).on_click(
                move |_event, _window, cx| {
                    cx.set_global(ClosePanelRequest {
                        entity_id: Some(entity_id),
                    });
                },
            ));
        }
        Some(buttons)
    }
}

impl Render for TimelinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tracing::trace!("render: '{}' column", self.title);

        let timeline_renderer = cx.global::<PerformanceSettings>().timeline_renderer;

        // --- Width change detection & height measurement (VirtualList only) ---
        if timeline_renderer == TimelineRenderer::VirtualList {
            let viewport_bounds = self.scroll_handle.bounds();
            let current_width = viewport_bounds.size.width;
            if current_width > px(0.0) {
                let should_invalidate = match self.last_measured_width {
                    None => true,
                    Some(prev_width) => {
                        let diff = if prev_width > current_width {
                            prev_width - current_width
                        } else {
                            current_width - prev_width
                        };
                        diff > px(1.0)
                    }
                };
                if should_invalidate {
                    self.height_cache.clear();
                }
                self.last_measured_width = Some(current_width);
            }

            self.measure_status_heights(window, cx);
            self.rebuild_item_sizes();
            self.cleanup_height_cache();
        }

        // --- Build callbacks ---
        let on_media: crate::ui::components::status_item::MediaClickHandler = Arc::new(
            |url: String,
             ctx: Option<LightboxStatusContext>,
             _window: &mut Window,
             cx: &mut App| {
                cx.set_global(LightboxState {
                    url: Some(url),
                    local_path: None,
                    status_ctx: ctx,
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                });
            },
        );

        let entity = cx.entity().downgrade();
        let on_cw_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity.update(cx, |this, cx| {
                    if !this.expanded_cw.remove(&id) {
                        this.expanded_cw.insert(id.clone());
                    }
                    this.invalidate_height_cache(&id);
                    cx.notify();
                });
            });

        let entity_nsfw = cx.entity().downgrade();
        let on_nsfw_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_nsfw.update(cx, |this, cx| {
                    if !this.revealed_nsfw.remove(&id) {
                        this.revealed_nsfw.insert(id.clone());
                    }
                    this.invalidate_height_cache(&id);
                    cx.notify();
                });
            });

        let on_reply: Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)> =
            Arc::new(|target: ReplyTarget, _window: &mut Window, cx: &mut App| {
                cx.set_global(ReplyState {
                    target: Some(target),
                });
            });

        let on_quote: Arc<dyn Fn(QuoteTarget, &mut Window, &mut App)> =
            Arc::new(|target: QuoteTarget, _window: &mut Window, cx: &mut App| {
                cx.set_global(QuoteState {
                    target: Some(target),
                });
            });

        let entity_reblog = cx.entity().downgrade();
        let on_reblog: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, window: &mut Window, cx: &mut App| {
                let confirm = cx
                    .try_global::<ConfirmationSettings>()
                    .map(|s| s.confirm_boost)
                    .unwrap_or(false);
                let currently_reblogged = entity_reblog
                    .upgrade()
                    .map(|e| {
                        e.read(cx)
                            .statuses
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.reblogged)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);

                if confirm && !currently_reblogged {
                    let entity = entity_reblog.clone();
                    let id = id.clone();
                    window.open_dialog(cx, move |dialog, _, _| {
                        let entity = entity.clone();
                        let id = id.clone();
                        dialog
                            .confirm()
                            .child("Boost this post?")
                            .on_ok(move |_, _window, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.toggle_reblog(id.clone(), cx);
                                });
                                true
                            })
                    });
                } else {
                    let _ = entity_reblog.update(cx, |this, cx| {
                        this.toggle_reblog(id, cx);
                    });
                }
            });

        let entity_fav = cx.entity().downgrade();
        let on_favourite: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, window: &mut Window, cx: &mut App| {
                let confirm = cx
                    .try_global::<ConfirmationSettings>()
                    .map(|s| s.confirm_favourite)
                    .unwrap_or(false);
                let currently_favourited = entity_fav
                    .upgrade()
                    .map(|e| {
                        e.read(cx)
                            .statuses
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.favourited)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);

                if confirm && !currently_favourited {
                    let entity = entity_fav.clone();
                    let id = id.clone();
                    window.open_dialog(cx, move |dialog, _, _| {
                        let entity = entity.clone();
                        let id = id.clone();
                        dialog.confirm().child("Favourite this post?").on_ok(
                            move |_, _window, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.toggle_favourite(id.clone(), cx);
                                });
                                true
                            },
                        )
                    });
                } else {
                    let _ = entity_fav.update(cx, |this, cx| {
                        this.toggle_favourite(id, cx);
                    });
                }
            });

        let entity_bookmark = cx.entity().downgrade();
        let on_bookmark: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_bookmark.update(cx, |this, cx| {
                    this.toggle_bookmark(id, cx);
                });
            });

        let on_account_click: Arc<dyn Fn(String, String, String, &mut Window, &mut App)> = Arc::new(
            |account_id: String,
             source_acct: String,
             server_domain: String,
             _window: &mut Window,
             cx: &mut App| {
                use crate::ui::panels::account_panel::AccountDetailRequest;
                cx.set_global(AccountDetailRequest {
                    account_id: Some(account_id),
                    source_acct: if source_acct.is_empty() {
                        None
                    } else {
                        Some(source_acct)
                    },
                    server_domain: if server_domain.is_empty() {
                        None
                    } else {
                        Some(server_domain)
                    },
                });
            },
        );

        let on_timestamp_click: Arc<dyn Fn(String, String, String, &mut Window, &mut App)> =
            Arc::new(
                |status_id: String,
                 source_acct: String,
                 server_domain: String,
                 _window: &mut Window,
                 cx: &mut App| {
                    use crate::ui::panels::status_detail_panel::StatusDetailRequest;
                    cx.set_global(StatusDetailRequest {
                        status_id: Some(status_id),
                        source_acct: if source_acct.is_empty() {
                            None
                        } else {
                            Some(source_acct)
                        },
                        server_domain: if server_domain.is_empty() {
                            None
                        } else {
                            Some(server_domain)
                        },
                    });
                },
            );

        let entity_reload = cx.entity().downgrade();
        let on_media_reload: Arc<dyn Fn(String, &mut Window, &mut App)> = Arc::new(
            move |preview_url: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_reload.update(cx, |this, cx| {
                    let count = this.retry_media.entry(preview_url).or_insert(0);
                    *count += 1;
                    cx.notify();
                });
            },
        );

        let entity_edit = cx.entity().downgrade();
        let on_edit: Arc<dyn Fn(String, &mut Window, &mut App)> = Arc::new(
            move |status_id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_edit.update(cx, |this, cx| {
                    let status_data = this.statuses.iter().find(|s| s.id == status_id).map(|s| {
                        (
                            s.original_status_id.clone(),
                            s.display_name.to_string(),
                            s.acct.to_string(),
                            s.content.to_string(),
                            s.visibility.to_string(),
                            s.media_attachments
                                .iter()
                                .map(|m| m.id.clone())
                                .collect::<Vec<_>>(),
                            s.quote_id.clone(),
                            s.poll.clone(),
                        )
                    });

                    if let Some((
                        api_status_id,
                        display_name,
                        acct,
                        content,
                        visibility,
                        media_ids,
                        quote_id,
                        poll,
                    )) = status_data
                    {
                        let client = this.client.clone();
                        let status_id_clone = api_status_id.clone();
                        let task = Tokio::spawn(cx, async move {
                            client
                                .get_status_source(&status_id_clone)
                                .await
                                .map_err(|e| e.to_string())
                        });

                        cx.spawn(
                            async move |_this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                                match task.await {
                                    Ok(Ok(source)) => {
                                        let _ = cx.update(|cx| {
                                            cx.set_global(EditState {
                                                target: Some(EditTarget {
                                                    status_id: api_status_id,
                                                    display_name,
                                                    acct,
                                                    content,
                                                    source_text: source.text,
                                                    spoiler_text: source.spoiler_text,
                                                    visibility,
                                                    media_ids,
                                                    quote_id,
                                                    poll,
                                                }),
                                            });
                                        });
                                    }
                                    Ok(Err(e)) => {
                                        tracing::error!("Failed to get status source: {}", e)
                                    }
                                    Err(e) => tracing::error!("Task error: {}", e),
                                }
                            },
                        )
                        .detach();
                    }
                });
            },
        );

        let entity_vote = cx.entity().downgrade();
        let on_vote: Arc<dyn Fn(String, Vec<usize>, &mut Window, &mut App)> = Arc::new(
            move |poll_id: String, choices: Vec<usize>, _window: &mut Window, cx: &mut App| {
                let _ = entity_vote.update(cx, |this, cx| {
                    this.vote_poll(poll_id, choices, cx);
                });
            },
        );

        let entity_poll_select = cx.entity().downgrade();
        let on_poll_select: Arc<dyn Fn(String, usize, &mut Window, &mut App)> = Arc::new(
            move |poll_id: String, index: usize, _window: &mut Window, cx: &mut App| {
                let _ = entity_poll_select.update(cx, |this, cx| {
                    let set = this.pending_poll_votes.entry(poll_id).or_default();
                    if !set.remove(&index) {
                        set.insert(index);
                    }
                    this.height_cache.clear();
                    cx.notify();
                });
            },
        );

        let entity_poll_refresh = cx.entity().downgrade();
        let on_poll_refresh: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |poll_id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_poll_refresh.update(cx, |this, cx| {
                    this.refresh_poll(poll_id, cx);
                });
            });

        let entity_expand = cx.entity().downgrade();
        let on_expand_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_expand.update(cx, |this, cx| {
                    if !this.expanded_statuses.remove(&id) {
                        this.expanded_statuses.insert(id.clone());
                    }
                    this.invalidate_height_cache(&id);
                    cx.notify();
                });
            });

        // --- Load more state ---
        let show_load_more = !self.statuses.is_empty()
            && !self.loading
            && (self.oldest_id.is_some() || self.db_has_more);
        let loading_more = self.loading && !self.statuses.is_empty();
        let has_footer = show_load_more || loading_more;

        let entity_load = cx.entity().downgrade();
        let on_load_more: Arc<dyn Fn(&mut Window, &mut App)> =
            Arc::new(move |_window: &mut Window, cx: &mut App| {
                let _ = entity_load.update(cx, |this, cx| {
                    this.load_more(cx);
                });
            });

        // --- Build timeline list ---
        let has_statuses = !self.statuses.is_empty();
        let show_loading = self.loading && !has_statuses;
        let show_empty = !has_statuses && !self.loading;

        let display_mode = cx.global::<AppearanceSettings>().display_mode;

        let mut container = div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .relative();

        match timeline_renderer {
            TimelineRenderer::List => {
                if has_statuses {
                    let status_elements: Vec<_> = self
                        .statuses
                        .iter()
                        .map(|status| {
                            let cw_expanded = self.expanded_cw.contains(&status.id);
                            let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
                            match display_mode {
                                DisplayMode::Mystique => {
                                    let mystique_expanded =
                                        self.expanded_statuses.contains(&status.id);
                                    render_compact_status_item(
                                        status,
                                        mystique_expanded,
                                        Some(&on_expand_toggle),
                                        cw_expanded,
                                        nsfw_revealed,
                                        Some(&on_cw_toggle),
                                        Some(&on_nsfw_toggle),
                                        Some(&on_media),
                                        Some(&on_reply),
                                        Some(&on_reblog),
                                        Some(&on_favourite),
                                        Some(&on_bookmark),
                                        Some(&on_quote),
                                        Some(&on_account_click),
                                        Some(&on_timestamp_click),
                                        Some(&on_media_reload),
                                        Some(&on_edit),
                                        Some(&on_vote),
                                        Some(&on_poll_select),
                                        Some(&on_poll_refresh),
                                        status
                                            .poll
                                            .as_ref()
                                            .and_then(|p| self.pending_poll_votes.get(&p.id)),
                                        Some(&self.account_id),
                                        &self.retry_media,
                                        window,
                                        cx,
                                    )
                                }
                                DisplayMode::StarryEyes => render_status_item(
                                    status,
                                    cw_expanded,
                                    nsfw_revealed,
                                    Some(&on_cw_toggle),
                                    Some(&on_nsfw_toggle),
                                    Some(&on_media),
                                    Some(&on_reply),
                                    Some(&on_reblog),
                                    Some(&on_favourite),
                                    Some(&on_bookmark),
                                    Some(&on_quote),
                                    Some(&on_account_click),
                                    Some(&on_timestamp_click),
                                    Some(&on_media_reload),
                                    Some(&on_edit),
                                    Some(&on_vote),
                                    Some(&on_poll_select),
                                    Some(&on_poll_refresh),
                                    status
                                        .poll
                                        .as_ref()
                                        .and_then(|p| self.pending_poll_votes.get(&p.id)),
                                    Some(&self.account_id),
                                    &self.retry_media,
                                    window,
                                    cx,
                                ),
                            }
                        })
                        .collect();

                    let mut scroll_content = div()
                        .id("timeline-list-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .track_scroll(&self.list_scroll_handle)
                        .children(status_elements);

                    // Footer: Load More / Loading
                    if has_footer {
                        if self.loading {
                            scroll_content = scroll_content.child(
                                div()
                                    .id("load-more-loading")
                                    .w_full()
                                    .py(px(12.0))
                                    .flex()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .child("Loading..."),
                            );
                        } else {
                            let cb = on_load_more.clone();
                            scroll_content = scroll_content.child(
                                div()
                                    .id("load-more-timeline")
                                    .w_full()
                                    .py(px(12.0))
                                    .flex()
                                    .justify_center()
                                    .child(
                                        Button::new("load-more-btn")
                                            .ghost()
                                            .label("Load more")
                                            .on_click(move |_, window, cx| {
                                                cb(window, cx);
                                            }),
                                    ),
                            );
                        }
                    }

                    container = container.child(scroll_content);
                }

                if show_loading {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(16.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("Loading..."),
                    );
                }

                if show_empty {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(32.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("No statuses yet"),
                    );
                }

                container.vertical_scrollbar(&self.list_scroll_handle)
            }
            TimelineRenderer::VirtualList => {
                // Append a footer item for Load More button / loading indicator
                let item_sizes = if has_footer {
                    let mut sizes = (*self.item_sizes).clone();
                    sizes.push(size(px(0.0), px(48.0)));
                    Rc::new(sizes)
                } else {
                    self.item_sizes.clone()
                };
                let status_count = self.statuses.len();
                let entity_handle = cx.entity().clone();

                if has_statuses {
                    let virtual_list = v_virtual_list(
                        entity_handle,
                        "timeline-virtual-list",
                        item_sizes,
                        move |this: &mut TimelinePanel,
                              range: Range<usize>,
                              window: &mut Window,
                              cx: &mut Context<TimelinePanel>| {
                            range
                                .map(|ix| {
                                    // Footer item: Load More button or loading indicator
                                    if ix >= status_count {
                                        if this.loading {
                                            return div()
                                                .id("load-more-loading")
                                                .w_full()
                                                .py(px(12.0))
                                                .flex()
                                                .justify_center()
                                                .text_sm()
                                                .text_color(rgb(0x6c7086))
                                                .child("Loading...")
                                                .into_any_element();
                                        }
                                        let cb = on_load_more.clone();
                                        return div()
                                            .id("load-more-timeline")
                                            .w_full()
                                            .py(px(12.0))
                                            .flex()
                                            .justify_center()
                                            .child(
                                                Button::new("load-more-btn")
                                                    .ghost()
                                                    .label("Load more")
                                                    .on_click(move |_, window, cx| {
                                                        cb(window, cx);
                                                    }),
                                            )
                                            .into_any_element();
                                    }

                                    let status = &this.statuses[ix];
                                    let cw_expanded = this.expanded_cw.contains(&status.id);
                                    let nsfw_revealed = this.revealed_nsfw.contains(&status.id);
                                    match display_mode {
                                        DisplayMode::Mystique => {
                                            let mystique_expanded =
                                                this.expanded_statuses.contains(&status.id);
                                            render_compact_status_item(
                                                status,
                                                mystique_expanded,
                                                Some(&on_expand_toggle),
                                                cw_expanded,
                                                nsfw_revealed,
                                                Some(&on_cw_toggle),
                                                Some(&on_nsfw_toggle),
                                                Some(&on_media),
                                                Some(&on_reply),
                                                Some(&on_reblog),
                                                Some(&on_favourite),
                                                Some(&on_bookmark),
                                                Some(&on_quote),
                                                Some(&on_account_click),
                                                Some(&on_timestamp_click),
                                                Some(&on_media_reload),
                                                Some(&on_edit),
                                                Some(&on_vote),
                                                Some(&on_poll_select),
                                                Some(&on_poll_refresh),
                                                status.poll.as_ref().and_then(|p| {
                                                    this.pending_poll_votes.get(&p.id)
                                                }),
                                                Some(&this.account_id),
                                                &this.retry_media,
                                                window,
                                                cx,
                                            )
                                        }
                                        DisplayMode::StarryEyes => render_status_item(
                                            status,
                                            cw_expanded,
                                            nsfw_revealed,
                                            Some(&on_cw_toggle),
                                            Some(&on_nsfw_toggle),
                                            Some(&on_media),
                                            Some(&on_reply),
                                            Some(&on_reblog),
                                            Some(&on_favourite),
                                            Some(&on_bookmark),
                                            Some(&on_quote),
                                            Some(&on_account_click),
                                            Some(&on_timestamp_click),
                                            Some(&on_media_reload),
                                            Some(&on_edit),
                                            Some(&on_vote),
                                            Some(&on_poll_select),
                                            Some(&on_poll_refresh),
                                            status
                                                .poll
                                                .as_ref()
                                                .and_then(|p| this.pending_poll_votes.get(&p.id)),
                                            Some(&this.account_id),
                                            &this.retry_media,
                                            window,
                                            cx,
                                        ),
                                    }
                                })
                                .collect()
                        },
                    )
                    .track_scroll(&self.scroll_handle)
                    .flex_1();

                    container = container.child(virtual_list);
                }

                if show_loading {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(16.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("Loading..."),
                    );
                }

                if show_empty {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(32.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("No statuses yet"),
                    );
                }

                container.vertical_scrollbar(&self.scroll_handle)
            }
        }
    }
}

/// Status context associated with a lightbox, enabling reply/boost/favourite/show-detail actions.
#[derive(Clone, Debug)]
pub struct LightboxStatusContext {
    /// API status id (used for reply/boost/favourite/show-detail)
    pub api_status_id: String,
    pub display_name: String,
    pub acct: String,
    pub content: String,
    pub visibility: String,
    pub url: Option<String>,
    pub reblogged: bool,
    pub favourited: bool,
}

/// Global state for lightbox image display
#[derive(Clone)]
pub struct LightboxState {
    pub url: Option<String>,
    pub local_path: Option<std::path::PathBuf>,
    /// Status context if this lightbox was opened from a status' media attachment.
    pub status_ctx: Option<LightboxStatusContext>,
    /// Zoom multiplier applied to the image's `max_w`/`max_h` (relative to viewport).
    ///
    /// `1.0` preserves the original fit-or-natural rendering. Values `> 1.0` make the
    /// allowed box larger (large images scale up toward natural size or overflow); values
    /// `< 1.0` shrink the allowed box (images are clipped smaller than the initial fit).
    pub zoom: f32,
    /// Pan offset in logical pixels from the centered position. `(0.0, 0.0)` centers the image.
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for LightboxState {
    fn default() -> Self {
        Self {
            url: None,
            local_path: None,
            status_ctx: None,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

impl gpui::Global for LightboxState {}

/// Global state for reply target
#[derive(Default)]
pub struct ReplyState {
    pub target: Option<ReplyTarget>,
}

impl gpui::Global for ReplyState {}

/// Global state for edit target
#[derive(Default)]
pub struct EditState {
    pub target: Option<EditTarget>,
}

impl gpui::Global for EditState {}

/// Global state for quote target
#[derive(Default)]
pub struct QuoteState {
    pub target: Option<QuoteTarget>,
}

impl gpui::Global for QuoteState {}

/// Global state for bookmark change notification
#[derive(Default, Clone)]
pub struct BookmarkChanged {
    pub version: u64,
}

impl gpui::Global for BookmarkChanged {}
