use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, App, Context, Entity, FocusHandle, Focusable, IntoElement, ScrollHandle,
    SharedString, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{Icon, IconName, Selectable, Sizable};

use crate::mastodon::types::account::CustomEmoji;

// ---------------------------------------------------------------------------
// EmojiCategory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EmojiCategory {
    SmileysAndEmotion,
    PeopleAndBody,
    AnimalsAndNature,
    FoodAndDrink,
    TravelAndPlaces,
    Activities,
    Objects,
    Symbols,
    Flags,
    Custom(String),
}

impl EmojiCategory {
    pub fn label(&self) -> &str {
        match self {
            Self::SmileysAndEmotion => "Smileys & Emotion",
            Self::PeopleAndBody => "People & Body",
            Self::AnimalsAndNature => "Animals & Nature",
            Self::FoodAndDrink => "Food & Drink",
            Self::TravelAndPlaces => "Travel & Places",
            Self::Activities => "Activities",
            Self::Objects => "Objects",
            Self::Symbols => "Symbols",
            Self::Flags => "Flags",
            Self::Custom(name) => {
                if name.is_empty() {
                    "Custom Emoji"
                } else {
                    name.as_str()
                }
            }
        }
    }

    pub fn icon_emoji(&self) -> &str {
        match self {
            Self::SmileysAndEmotion => "\u{1f600}",
            Self::PeopleAndBody => "\u{1f44b}",
            Self::AnimalsAndNature => "\u{1f43b}",
            Self::FoodAndDrink => "\u{1f354}",
            Self::TravelAndPlaces => "\u{2708}\u{fe0f}",
            Self::Activities => "\u{26bd}",
            Self::Objects => "\u{1f4a1}",
            Self::Symbols => "\u{2764}\u{fe0f}",
            Self::Flags => "\u{1f3f3}\u{fe0f}",
            Self::Custom(_) => "\u{2b50}",
        }
    }

    fn from_emojis_group(group: emojis::Group) -> Self {
        match group {
            emojis::Group::SmileysAndEmotion => Self::SmileysAndEmotion,
            emojis::Group::PeopleAndBody => Self::PeopleAndBody,
            emojis::Group::AnimalsAndNature => Self::AnimalsAndNature,
            emojis::Group::FoodAndDrink => Self::FoodAndDrink,
            emojis::Group::TravelAndPlaces => Self::TravelAndPlaces,
            emojis::Group::Activities => Self::Activities,
            emojis::Group::Objects => Self::Objects,
            emojis::Group::Symbols => Self::Symbols,
            emojis::Group::Flags => Self::Flags,
        }
    }
}

// ---------------------------------------------------------------------------
// CachedEmoji
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CachedEmoji {
    pub shortcode: String,
    pub unicode: Option<String>,
    pub image_url: Option<String>,
    pub category: EmojiCategory,
}

// ---------------------------------------------------------------------------
// EmojiStore (Global)
// ---------------------------------------------------------------------------

pub struct EmojiStore {
    pub emojis: Vec<CachedEmoji>,
    pub categories: Vec<EmojiCategory>,
}

impl gpui::Global for EmojiStore {}

impl EmojiStore {
    pub fn new() -> Self {
        let mut emojis = Vec::new();
        let mut categories = Vec::new();

        for group in emojis::Group::iter() {
            let category = EmojiCategory::from_emojis_group(group);
            categories.push(category.clone());

            for emoji in group.emojis() {
                if let Some(shortcode) = emoji.shortcode() {
                    emojis.push(CachedEmoji {
                        shortcode: shortcode.to_string(),
                        unicode: Some(emoji.as_str().to_string()),
                        image_url: None,
                        category: category.clone(),
                    });
                }
            }
        }

        Self { emojis, categories }
    }

    pub fn set_custom_emojis(&mut self, custom: Vec<CustomEmoji>) {
        // Remove previous custom emojis
        self.emojis.retain(|e| e.image_url.is_none());
        self.categories
            .retain(|c| !matches!(c, EmojiCategory::Custom(_)));

        // Collect categories in insertion order
        let mut seen_categories: Vec<String> = Vec::new();
        for ce in &custom {
            if !ce.visible_in_picker {
                continue;
            }
            let cat_name = ce.category.clone().unwrap_or_default();
            if !seen_categories.contains(&cat_name) {
                seen_categories.push(cat_name);
            }
        }

        // Add custom categories
        for cat_name in &seen_categories {
            self.categories
                .push(EmojiCategory::Custom(cat_name.clone()));
        }

        // Add custom emojis
        for ce in custom {
            if !ce.visible_in_picker {
                continue;
            }
            let cat_name = ce.category.unwrap_or_default();
            self.emojis.push(CachedEmoji {
                shortcode: ce.shortcode,
                unicode: None,
                image_url: Some(ce.url),
                category: EmojiCategory::Custom(cat_name),
            });
        }
    }

    pub fn search(&self, query: &str) -> Vec<&CachedEmoji> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_lowercase();
        self.emojis
            .iter()
            .filter(|e| e.shortcode.to_lowercase().contains(&query_lower))
            .collect()
    }

    pub fn emojis_in_category(&self, category: &EmojiCategory) -> Vec<&CachedEmoji> {
        self.emojis
            .iter()
            .filter(|e| &e.category == category)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// ContentData (helper for render_content)
// ---------------------------------------------------------------------------

enum ContentData {
    SearchResults(Vec<CachedEmoji>),
    Category(String, Vec<CachedEmoji>),
    Empty,
}

// ---------------------------------------------------------------------------
// EmojiPicker (Entity)
// ---------------------------------------------------------------------------

pub struct EmojiPicker {
    search_input: Entity<InputState>,
    search_query: String,
    selected_category: Option<EmojiCategory>,
    compose_input: Entity<InputState>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    tab_scroll_handle: ScrollHandle,
}

impl EmojiPicker {
    pub fn new(
        compose_input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search emoji..."));

        cx.subscribe_in(
            &search_input,
            window,
            |this, _state, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.on_search_changed(cx);
                }
            },
        )
        .detach();

        Self {
            search_input,
            search_query: String::new(),
            selected_category: Some(EmojiCategory::SmileysAndEmotion),
            compose_input,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            tab_scroll_handle: ScrollHandle::new(),
        }
    }

    fn on_search_changed(&mut self, cx: &mut Context<Self>) {
        self.search_query = self.search_input.read(cx).value().to_string();
        if !self.search_query.is_empty() {
            self.selected_category = None;
        } else if self.selected_category.is_none() {
            self.selected_category = Some(EmojiCategory::SmileysAndEmotion);
        }
        cx.notify();
    }

    fn select_category(
        &mut self,
        category: EmojiCategory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_category = Some(category.clone());
        self.search_query.clear();
        self.search_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });

        // Scroll the tab bar to show the selected tab
        if let Some(store) = cx.try_global::<EmojiStore>() {
            if let Some(idx) = store.categories.iter().position(|c| c == &category) {
                self.tab_scroll_handle.scroll_to_item(idx);
            }
        }

        cx.notify();
    }

    fn insert_emoji(&self, emoji: &CachedEmoji, window: &mut Window, cx: &mut Context<Self>) {
        let text = if let Some(ref unicode) = emoji.unicode {
            unicode.clone()
        } else {
            format!(":{}:", emoji.shortcode)
        };

        self.compose_input.update(cx, |state, cx| {
            state.insert(text, window, cx);
        });
    }

    fn render_category_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let categories = cx
            .try_global::<EmojiStore>()
            .map(|s| s.categories.clone())
            .unwrap_or_default();

        let selected_index = categories
            .iter()
            .position(|c| self.selected_category.as_ref() == Some(c))
            .unwrap_or(0);

        let category_count = categories.len();
        let prev_cat = if selected_index > 0 {
            Some(categories[selected_index - 1].clone())
        } else {
            None
        };
        let next_cat = if selected_index + 1 < category_count {
            Some(categories[selected_index + 1].clone())
        } else {
            None
        };

        let has_prev = prev_cat.is_some();
        let has_next = next_cat.is_some();

        TabBar::new("emoji-cat-tabs")
            .selected_index(selected_index)
            .track_scroll(&self.tab_scroll_handle)
            .pill()
            .prefix(
                div()
                    .id("tab-chevron-left")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(24.0))
                    .cursor(if has_prev {
                        gpui::CursorStyle::PointingHand
                    } else {
                        gpui::CursorStyle::default()
                    })
                    .child(
                        Icon::new(IconName::ChevronLeft)
                            .xsmall()
                            .text_color(if has_prev {
                                rgb(0xa6adc8)
                            } else {
                                rgb(0x45475a)
                            }),
                    )
                    .when_some(prev_cat, |el, cat| {
                        el.on_click(cx.listener(move |this, _, window, cx| {
                            this.select_category(cat.clone(), window, cx);
                        }))
                    }),
            )
            .suffix(
                div()
                    .id("tab-chevron-right")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(24.0))
                    .cursor(if has_next {
                        gpui::CursorStyle::PointingHand
                    } else {
                        gpui::CursorStyle::default()
                    })
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(if has_next {
                                rgb(0xa6adc8)
                            } else {
                                rgb(0x45475a)
                            }),
                    )
                    .when_some(next_cat, |el, cat| {
                        el.on_click(cx.listener(move |this, _, window, cx| {
                            this.select_category(cat.clone(), window, cx);
                        }))
                    }),
            )
            .children(categories.into_iter().enumerate().map(|(i, cat)| {
                let is_selected = i == selected_index;
                let label = cat.icon_emoji().to_string();
                let cat_for_click = cat.clone();

                Tab::new()
                    .label(label)
                    .selected(is_selected)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_category(cat_for_click.clone(), window, cx);
                    }))
            }))
    }

    fn render_emoji_cell(
        &self,
        emoji: &CachedEmoji,
        id: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let emoji_clone = emoji.clone();

        let cell = div()
            .id(id)
            .w(px(34.0))
            .h(px(34.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|el| el.bg(rgb(0x45475a)));

        let cell = if let Some(ref unicode) = emoji.unicode {
            cell.text_size(px(22.0)).child(unicode.clone())
        } else if let Some(ref url) = emoji.image_url {
            cell.child(img(SharedString::from(url.clone())).w(px(24.0)).h(px(24.0)))
        } else {
            cell
        };

        cell.on_click(cx.listener(move |this, _, window, cx| {
            this.insert_emoji(&emoji_clone, window, cx);
        }))
    }

    fn render_emoji_grid(
        &self,
        emojis: &[&CachedEmoji],
        id_prefix: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .gap(px(2.0))
            .p(px(4.0))
            .children(emojis.iter().enumerate().map(|(i, emoji)| {
                let id = SharedString::from(format!("{}-{}", id_prefix, i));
                self.render_emoji_cell(emoji, id, cx)
            }))
    }

    fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Clone data out of the store first to avoid borrow conflicts with cx
        let content_data = cx.try_global::<EmojiStore>().map(|store| {
            if !self.search_query.is_empty() {
                let results: Vec<CachedEmoji> = store
                    .search(&self.search_query)
                    .into_iter()
                    .cloned()
                    .collect();
                ContentData::SearchResults(results)
            } else if let Some(ref category) = self.selected_category {
                let emojis: Vec<CachedEmoji> = store
                    .emojis_in_category(category)
                    .into_iter()
                    .cloned()
                    .collect();
                let label = category.label().to_string();
                ContentData::Category(label, emojis)
            } else {
                ContentData::Empty
            }
        });

        match content_data {
            None => div()
                .id("emoji-grid-scroll")
                .p(px(16.0))
                .child("Loading...")
                .into_any_element(),
            Some(ContentData::SearchResults(results)) => {
                let refs: Vec<&CachedEmoji> = results.iter().collect();
                div()
                    .id("emoji-grid-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .when(refs.is_empty(), |el| {
                        el.child(
                            div()
                                .p(px(16.0))
                                .text_sm()
                                .text_color(rgb(0x6c7086))
                                .child("No emoji found"),
                        )
                    })
                    .when(!refs.is_empty(), |el| {
                        el.child(self.render_emoji_grid(&refs, "search", cx))
                    })
                    .into_any_element()
            }
            Some(ContentData::Category(label, emojis)) => {
                let refs: Vec<&CachedEmoji> = emojis.iter().collect();
                div()
                    .id("emoji-grid-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .text_xs()
                            .text_color(rgb(0xa6adc8))
                            .child(label),
                    )
                    .child(self.render_emoji_grid(&refs, "cat", cx))
                    .into_any_element()
            }
            Some(ContentData::Empty) => div().id("emoji-grid-scroll").into_any_element(),
        }
    }
}

impl Focusable for EmojiPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EmojiPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("emoji-picker-root")
            .w(px(340.0))
            .max_h(px(380.0))
            .flex()
            .flex_col()
            .relative()
            .vertical_scrollbar(&self.scroll_handle)
            .child(self.render_category_tabs(cx))
            .child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .child(Input::new(&self.search_input).appearance(true).h(px(28.0))),
            )
            .child(self.render_content(cx))
    }
}
