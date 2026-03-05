use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, Context, Entity, IntoElement, ObjectFit, SharedString, Timer, WeakEntity,
    Window,
};
use gpui_component::input::{InputEvent, InputState, Position};
use gpui_tokio_bridge::Tokio;

use crate::db::pool::Database;
use crate::mastodon::client::MastodonClient;
use crate::state::performance::{PerformanceSettings, SuggestionSource};

const DEBOUNCE_MS: u64 = 300;
const MAX_SUGGESTIONS: u32 = 4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TriggerKind {
    Mention,
    Hashtag,
}

#[derive(Debug, Clone)]
struct TriggerContext {
    kind: TriggerKind,
    /// Byte offset of the trigger character (@/#) in the full text
    trigger_offset: usize,
    /// The query string after the trigger character
    query: String,
}

#[derive(Debug, Clone)]
pub enum SuggestionItem {
    Account {
        acct: String,
        display_name: String,
        avatar: String,
    },
    Hashtag {
        name: String,
    },
}

// ---------------------------------------------------------------------------
// AutocompletePopup (Entity)
// ---------------------------------------------------------------------------

pub struct AutocompletePopup {
    compose_input: Entity<InputState>,
    client: MastodonClient,
    database: Arc<Database>,
    suggestions: Vec<SuggestionItem>,
    selected_index: usize,
    visible: bool,
    trigger: Option<TriggerContext>,
    debounce_task: Option<gpui::Task<()>>,
}

impl AutocompletePopup {
    pub fn new(
        compose_input: Entity<InputState>,
        client: MastodonClient,
        database: Arc<Database>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Subscribe to compose input changes
        cx.subscribe_in(
            &compose_input,
            window,
            |this, _state, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.on_input_changed(cx);
                }
                if let InputEvent::Blur = event {
                    this.dismiss(cx);
                }
            },
        )
        .detach();

        Self {
            compose_input,
            client,
            database,
            suggestions: Vec::new(),
            selected_index: 0,
            visible: false,
            trigger: None,
            debounce_task: None,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn select_up(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.is_empty() && self.selected_index > 0 {
            self.selected_index -= 1;
            cx.notify();
        }
    }

    pub fn select_down(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.is_empty() && self.selected_index + 1 < self.suggestions.len() {
            self.selected_index += 1;
            cx.notify();
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.suggestions.clear();
        self.trigger = None;
        self.debounce_task = None;
        self.selected_index = 0;
        cx.notify();
    }

    pub fn accept_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(trigger) = &self.trigger else {
            return;
        };
        let Some(item) = self.suggestions.get(self.selected_index) else {
            return;
        };

        let replacement = match item {
            SuggestionItem::Account { acct, .. } => format!("@{} ", acct),
            SuggestionItem::Hashtag { name } => format!("#{} ", name),
        };

        let current_text = self.compose_input.read(cx).value().to_string();
        let cursor = self.compose_input.read(cx).cursor();
        let trigger_offset = trigger.trigger_offset;

        // Safety check
        if trigger_offset > current_text.len() || cursor > current_text.len() {
            self.dismiss(cx);
            return;
        }

        // Build new text: [before trigger] + replacement + [after cursor]
        let before = &current_text[..trigger_offset];
        let after = &current_text[cursor..];
        let new_text = format!("{}{}{}", before, replacement, after);

        // Calculate new cursor position
        let new_cursor_byte = trigger_offset + replacement.len();
        let before_cursor_text = &new_text[..new_cursor_byte];
        let lines: Vec<&str> = before_cursor_text.split('\n').collect();
        let line = (lines.len() - 1) as u32;
        let col = lines.last().unwrap_or(&"").chars().count() as u32;

        self.compose_input.update(cx, |state, cx| {
            state.set_value(&new_text, window, cx);
            state.set_cursor_position(Position::new(line, col), window, cx);
        });

        self.dismiss(cx);
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    fn on_input_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.compose_input.read(cx).value().to_string();
        let cursor = self.compose_input.read(cx).cursor();

        match Self::detect_trigger(&text, cursor) {
            Some(trigger) if !trigger.query.is_empty() => {
                self.schedule_fetch(trigger, cx);
            }
            _ => {
                self.dismiss(cx);
            }
        }
    }

    fn detect_trigger(text: &str, cursor_byte_offset: usize) -> Option<TriggerContext> {
        if cursor_byte_offset == 0 || cursor_byte_offset > text.len() {
            return None;
        }

        let before_cursor = &text[..cursor_byte_offset];

        // Scan backwards from cursor to find the start of the current word
        let mut scan_start = 0;
        for (i, ch) in before_cursor.char_indices().rev() {
            if ch == ' ' || ch == '\n' || ch == '\r' || ch == '\t' {
                scan_start = i + ch.len_utf8();
                break;
            }
        }

        let candidate = &text[scan_start..cursor_byte_offset];
        if candidate.is_empty() {
            return None;
        }

        let first_char = candidate.chars().next()?;
        let kind = match first_char {
            '@' => TriggerKind::Mention,
            '#' => TriggerKind::Hashtag,
            _ => return None,
        };

        let query = &candidate[first_char.len_utf8()..];

        // If query contains whitespace, it's not a valid trigger
        if query.contains(char::is_whitespace) {
            return None;
        }

        Some(TriggerContext {
            kind,
            trigger_offset: scan_start,
            query: query.to_string(),
        })
    }

    fn schedule_fetch(&mut self, trigger: TriggerContext, cx: &mut Context<Self>) {
        // Cancel any existing debounce task by dropping it
        self.debounce_task = None;
        self.trigger = Some(trigger.clone());

        let client = self.client.clone();
        let database = self.database.clone();
        let query = trigger.query.clone();
        let kind = trigger.kind.clone();

        // Read performance settings to determine data source
        let perf = cx
            .try_global::<PerformanceSettings>()
            .cloned()
            .unwrap_or_default();
        let use_sqlite = match &kind {
            TriggerKind::Mention => perf.mention_source == SuggestionSource::SQLite,
            TriggerKind::Hashtag => perf.hashtag_source == SuggestionSource::SQLite,
        };

        let task = cx.spawn(
            async move |this: WeakEntity<AutocompletePopup>, cx: &mut gpui::AsyncApp| {
                // Debounce
                Timer::after(Duration::from_millis(DEBOUNCE_MS)).await;

                // Return to UI thread to spawn Tokio task
                let api_task = this.update(cx, |_this, cx| {
                    if use_sqlite {
                        let db = database.clone();
                        let query = query.clone();
                        let kind = kind.clone();
                        Tokio::spawn(cx, async move {
                            match kind {
                                TriggerKind::Mention => {
                                    crate::db::queries::accounts::search_accounts_prefix(
                                        db.reader(),
                                        &query,
                                        MAX_SUGGESTIONS,
                                    )
                                    .await
                                    .map(|accounts| {
                                        accounts
                                            .into_iter()
                                            .map(|a| SuggestionItem::Account {
                                                acct: a.acct,
                                                display_name: a.display_name,
                                                avatar: a.avatar,
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                    .map_err(|e| e.to_string())
                                }
                                TriggerKind::Hashtag => {
                                    crate::db::queries::tags::search_tags_prefix(
                                        db.reader(),
                                        &query,
                                        MAX_SUGGESTIONS,
                                    )
                                    .await
                                    .map(|names| {
                                        names
                                            .into_iter()
                                            .map(|name| SuggestionItem::Hashtag { name })
                                            .collect::<Vec<_>>()
                                    })
                                    .map_err(|e| e.to_string())
                                }
                            }
                        })
                    } else {
                        let client = client.clone();
                        let query = query.clone();
                        let kind = kind.clone();
                        Tokio::spawn(cx, async move {
                            match kind {
                                TriggerKind::Mention => client
                                    .search_accounts(&query, MAX_SUGGESTIONS)
                                    .await
                                    .map(|accounts| {
                                        accounts
                                            .into_iter()
                                            .map(|a| SuggestionItem::Account {
                                                acct: a.acct,
                                                display_name: a.display_name,
                                                avatar: a.avatar,
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                    .map_err(|e| e.to_string()),
                                TriggerKind::Hashtag => client
                                    .search_hashtags(&query, MAX_SUGGESTIONS)
                                    .await
                                    .map(|result| {
                                        result
                                            .hashtags
                                            .into_iter()
                                            .map(|t| SuggestionItem::Hashtag { name: t.name })
                                            .collect::<Vec<_>>()
                                    })
                                    .map_err(|e| e.to_string()),
                            }
                        })
                    }
                });

                let Ok(api_task) = api_task else { return };

                match api_task.await {
                    Ok(Ok(items)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.suggestions = items;
                            this.selected_index = 0;
                            this.visible = !this.suggestions.is_empty();
                            cx.notify();
                        });
                    }
                    _ => {
                        let _ = this.update(cx, |this, cx| {
                            this.dismiss(cx);
                        });
                    }
                }
            },
        );

        self.debounce_task = Some(task);
    }

    fn render_suggestion_item(
        &self,
        item: &SuggestionItem,
        index: usize,
        is_selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let bg = if is_selected {
            rgb(0x45475a)
        } else {
            rgb(0x313244)
        };

        match item {
            SuggestionItem::Account {
                acct,
                display_name,
                avatar,
            } => div()
                .id(SharedString::from(format!("suggest-{}", index)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(4.0))
                .bg(bg)
                .hover(|el| el.bg(rgb(0x585b70)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.selected_index = index;
                    this.accept_selection(window, cx);
                }))
                .child(
                    div()
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded(px(4.0))
                        .overflow_hidden()
                        .flex_shrink_0()
                        .child(
                            img(SharedString::from(avatar.clone()))
                                .w(px(24.0))
                                .h(px(24.0))
                                .object_fit(ObjectFit::Cover),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xcdd6f4))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(display_name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x6c7086))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(format!("@{}", acct)),
                        ),
                )
                .into_any_element(),
            SuggestionItem::Hashtag { name } => div()
                .id(SharedString::from(format!("suggest-{}", index)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(6.0))
                .bg(bg)
                .hover(|el| el.bg(rgb(0x585b70)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.selected_index = index;
                    this.accept_selection(window, cx);
                }))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xcdd6f4))
                        .child(format!("#{}", name)),
                )
                .into_any_element(),
        }
    }
}

impl Render for AutocompletePopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible || self.suggestions.is_empty() {
            return div().into_any_element();
        }

        // Verify trigger is still valid
        let text = self.compose_input.read(cx).value().to_string();
        let cursor = self.compose_input.read(cx).cursor();
        if let Some(ref trigger) = self.trigger {
            let still_valid = Self::detect_trigger(&text, cursor)
                .map(|t| t.trigger_offset == trigger.trigger_offset && t.kind == trigger.kind)
                .unwrap_or(false);
            if !still_valid {
                return div().into_any_element();
            }
        }

        div()
            .id("autocomplete-popup")
            .absolute()
            .top(px(64.0))
            .left_0()
            .w_full()
            .max_h(px(200.0))
            .overflow_y_scroll()
            .bg(rgb(0x313244))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(0x45475a))
            .py(px(4.0))
            .shadow_lg()
            .children(
                self.suggestions
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let is_selected = i == self.selected_index;
                        self.render_suggestion_item(item, i, is_selected, cx)
                    })
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }
}
