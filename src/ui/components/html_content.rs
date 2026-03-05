use gpui::prelude::*;
use gpui::{div, img, rgb, AnyElement, Pixels, SharedString};
use kuchikiki::traits::TendrilSink;

use crate::ui::components::status_item::EmojiMapping;

/// Segments produced by walking the DOM / scanning plain text.
enum InlineSegment {
    Text(String),
    Link { text: String, url: String },
    Emoji { url: String },
    LineBreak,
}

/// Render Mastodon status HTML (content / bio) into native GPUI elements.
///
/// Parses the HTML with kuchikiki, walks the DOM tree, and builds flex-wrap
/// containers with inline text / link / emoji segments — bypassing
/// `TextView::html()` which forces block-level layout.
pub fn render_html_content(
    id_prefix: &str,
    html: &str,
    emojis: &[EmojiMapping],
    font_size: Pixels,
) -> Vec<AnyElement> {
    let sink = kuchikiki::parse_html().one(html);
    let doc = sink.document_node;

    // Collect top-level block segments (each <p> becomes a paragraph,
    // bare inline nodes become a single implicit paragraph).
    let mut paragraphs: Vec<Vec<InlineSegment>> = Vec::new();

    // Walk <html><head>...<body>children — we only care about <body> children.
    let body = find_body(&doc).unwrap_or(doc);

    let mut current: Vec<InlineSegment> = Vec::new();
    for child in body.children() {
        if is_block_element(&child) {
            // Flush any accumulated inline content before the block.
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
            // Collect inlines inside this block element.
            let mut block_segments: Vec<InlineSegment> = Vec::new();
            collect_inline_segments(&child, emojis, &mut block_segments);
            if !block_segments.is_empty() {
                paragraphs.push(block_segments);
            }
        } else {
            collect_inline_segments(&child, emojis, &mut current);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }

    render_paragraphs(id_prefix, &paragraphs, font_size)
}

/// Render plain text (display_name, notification labels) with inline custom
/// emojis. No HTML parsing — just scan for `:shortcode:` patterns.
pub fn render_plain_with_emojis(
    id_prefix: &str,
    text: &str,
    emojis: &[EmojiMapping],
    font_size: Pixels,
) -> Vec<AnyElement> {
    let segments = split_emojis(text, emojis);
    let paragraphs = vec![segments];
    render_paragraphs(id_prefix, &paragraphs, font_size)
}

// ---------------------------------------------------------------------------
// DOM helpers
// ---------------------------------------------------------------------------

/// Find the `<body>` node inside the parsed document.
fn find_body(node: &kuchikiki::NodeRef) -> Option<kuchikiki::NodeRef> {
    for child in node.children() {
        if let Some(el) = child.as_element() {
            if &*el.name.local == "body" {
                return Some(child.clone());
            }
        }
        if let Some(found) = find_body(&child) {
            return Some(found);
        }
    }
    None
}

/// Whether a node is a block-level HTML element (p, div, blockquote, …).
fn is_block_element(node: &kuchikiki::NodeRef) -> bool {
    if let Some(el) = node.as_element() {
        matches!(
            &*el.name.local,
            "p" | "div"
                | "blockquote"
                | "pre"
                | "ul"
                | "ol"
                | "li"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "hr"
                | "table"
                | "section"
                | "article"
        )
    } else {
        false
    }
}

/// Recursively collect inline segments from a DOM subtree.
fn collect_inline_segments(
    node: &kuchikiki::NodeRef,
    emojis: &[EmojiMapping],
    out: &mut Vec<InlineSegment>,
) {
    if let Some(text_cell) = node.as_text() {
        let text = text_cell.borrow().clone();
        out.extend(split_emojis(&text, emojis));
        return;
    }

    if let Some(el) = node.as_element() {
        let tag = &*el.name.local;

        match tag {
            "br" => {
                out.push(InlineSegment::LineBreak);
            }
            "a" => {
                let href = el
                    .attributes
                    .borrow()
                    .get("href")
                    .unwrap_or_default()
                    .to_string();
                // Collect all text inside <a>, including nested spans.
                let link_text = node.text_contents();
                if !link_text.is_empty() {
                    // Also split emojis inside link text
                    let segments = split_emojis(&link_text, emojis);
                    for seg in segments {
                        match seg {
                            InlineSegment::Text(t) => {
                                out.push(InlineSegment::Link {
                                    text: t,
                                    url: href.clone(),
                                });
                            }
                            InlineSegment::Emoji { url } => {
                                out.push(InlineSegment::Emoji { url });
                            }
                            other => out.push(other),
                        }
                    }
                }
            }
            _ => {
                // <span>, <em>, <strong>, etc. — recurse into children.
                for child in node.children() {
                    collect_inline_segments(&child, emojis, out);
                }
            }
        }
    } else {
        // Document, comment, etc. — recurse.
        for child in node.children() {
            collect_inline_segments(&child, emojis, out);
        }
    }
}

/// Split a plain-text string at `:shortcode:` boundaries into Text / Emoji
/// segments.
fn split_emojis(text: &str, emojis: &[EmojiMapping]) -> Vec<InlineSegment> {
    if emojis.is_empty() {
        return if text.is_empty() {
            vec![]
        } else {
            vec![InlineSegment::Text(text.to_string())]
        };
    }

    let mut segments: Vec<InlineSegment> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Find the earliest shortcode match.
        let mut best_pos: Option<usize> = None;
        let mut best_emoji: Option<&EmojiMapping> = None;
        let mut best_len = 0usize;

        for emoji in emojis {
            let pattern_len = emoji.shortcode.len() + 2; // :shortcode:
                                                         // Manual search to avoid allocating a format string every iteration.
            if let Some(start) = find_shortcode(remaining, &emoji.shortcode) {
                if best_pos.is_none() || start < best_pos.unwrap() {
                    best_pos = Some(start);
                    best_emoji = Some(emoji);
                    best_len = pattern_len;
                }
            }
        }

        match (best_pos, best_emoji) {
            (Some(pos), Some(emoji)) => {
                if pos > 0 {
                    segments.push(InlineSegment::Text(remaining[..pos].to_string()));
                }
                segments.push(InlineSegment::Emoji {
                    url: emoji.url.clone(),
                });
                remaining = &remaining[pos + best_len..];
            }
            _ => {
                segments.push(InlineSegment::Text(remaining.to_string()));
                break;
            }
        }
    }

    segments
}

/// Find `:shortcode:` in a string, returning the byte offset of the leading
/// colon.
fn find_shortcode(haystack: &str, shortcode: &str) -> Option<usize> {
    let needle_len = shortcode.len() + 2; // :shortcode:
    if haystack.len() < needle_len {
        return None;
    }

    let bytes = haystack.as_bytes();
    let sc_bytes = shortcode.as_bytes();

    for i in 0..=bytes.len() - needle_len {
        if bytes[i] == b':'
            && bytes[i + 1 + sc_bytes.len()] == b':'
            && &bytes[i + 1..i + 1 + sc_bytes.len()] == sc_bytes
        {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Word splitting (for flex-wrap line breaking)
// ---------------------------------------------------------------------------

/// Whether a character should break individually for line-wrapping purposes
/// (CJK ideographs, kana, hangul, fullwidth punctuation, etc.).
fn is_cjk_breakable(ch: char) -> bool {
    let c = ch as u32;
    matches!(c,
        0x2E80..=0x9FFF   // CJK radicals, kangxi radicals, ideographs, etc.
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFE30..=0xFE4F // CJK Compatibility Forms
        | 0xFF01..=0xFFEF // Fullwidth Latin, halfwidth katakana, etc.
        | 0x20000..=0x2FA1F // CJK Unified Ideographs Extension B–F
        | 0x3000..=0x303F // CJK Symbols and Punctuation (incl. ideographic space)
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana
    )
}

/// Split text into word-level chunks for flex-wrap line breaking.
///
/// - Latin text splits at whitespace boundaries (space stays with preceding word).
/// - CJK / fullwidth characters each become a separate chunk so they can wrap
///   independently inside a `flex().flex_wrap()` container.
fn split_into_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if is_cjk_breakable(ch) {
            // Flush any accumulated Latin/ASCII word first.
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            // Each CJK character is its own breakable unit.
            words.push(ch.to_string());
        } else if ch == ' ' || ch == '\t' {
            // Include the space in the current word, then flush.
            current.push(ch);
            words.push(std::mem::take(&mut current));
        } else if matches!(ch, '/' | '?' | '&' | '=' | '#' | '-' | '.' | ',' | ';') {
            // URL / punctuation break-points: flush before the delimiter,
            // then start a new word beginning with the delimiter character.
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            current.push(ch);
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    // Force-split any chunk that is still too long (e.g. percent-encoded URL
    // segments like "C272806%2CC272801%2CC265810%2C…" with no delimiters).
    const MAX_CHUNK: usize = 12;
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    for word in words {
        if word.chars().count() <= MAX_CHUNK {
            out.push(word);
        } else {
            let mut buf = String::new();
            for ch in word.chars() {
                buf.push(ch);
                if buf.chars().count() >= MAX_CHUNK {
                    out.push(std::mem::take(&mut buf));
                }
            }
            if !buf.is_empty() {
                out.push(buf);
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Convert a list of paragraphs (each being a list of InlineSegments) into
/// GPUI elements.
fn render_paragraphs(
    id_prefix: &str,
    paragraphs: &[Vec<InlineSegment>],
    font_size: Pixels,
) -> Vec<AnyElement> {
    let mut result: Vec<AnyElement> = Vec::new();

    for (p_idx, segments) in paragraphs.iter().enumerate() {
        // Split segments at LineBreak into individual lines.
        let lines = split_at_line_breaks(segments);

        for (l_idx, line) in lines.iter().enumerate() {
            if line.is_empty() {
                // Empty line → small vertical gap.
                result.push(div().h(font_size).into_any_element());
                continue;
            }

            // Check if line has any non-text segments (links or emojis).
            let has_inline_elements = line.iter().any(|seg| {
                matches!(
                    seg,
                    InlineSegment::Link { .. } | InlineSegment::Emoji { .. }
                )
            });

            if !has_inline_elements {
                // Pure text — render as a single element; GPUI's text layout
                // handles word/character wrapping natively.
                let full_text: String = line
                    .iter()
                    .filter_map(|seg| match seg {
                        InlineSegment::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect();
                if !full_text.is_empty() {
                    result.push(div().child(full_text).into_any_element());
                }
            } else {
                // Mixed content (text + emoji/link).
                // Split text into words so each word is a flex item that can
                // wrap independently via flex_wrap.
                let mut container = div().flex().flex_wrap().items_center();
                let mut elem_idx = 0u32;

                for seg in line {
                    match seg {
                        InlineSegment::Text(t) => {
                            for word in split_into_words(t) {
                                if !word.is_empty() {
                                    container = container.child(word);
                                    elem_idx += 1;
                                }
                            }
                        }
                        InlineSegment::Link { text, url } => {
                            // Split link text into words too so long URLs can wrap.
                            for word in split_into_words(text) {
                                if !word.is_empty() {
                                    let id = SharedString::from(format!(
                                        "{}-p{}-l{}-e{}",
                                        id_prefix, p_idx, l_idx, elem_idx
                                    ));
                                    let open_url = url.clone();
                                    container = container.child(
                                        div()
                                            .id(id)
                                            .text_color(rgb(0x89b4fa))
                                            .cursor_pointer()
                                            .on_click(move |_, _, _| {
                                                let _ = open::that(&open_url);
                                            })
                                            .child(word),
                                    );
                                    elem_idx += 1;
                                }
                            }
                        }
                        InlineSegment::Emoji { url } => {
                            container = container.child(
                                img(SharedString::from(url.clone()))
                                    .w(font_size)
                                    .h(font_size)
                                    .flex_shrink_0(),
                            );
                            elem_idx += 1;
                        }
                        InlineSegment::LineBreak => {}
                    }
                }

                result.push(container.into_any_element());
            }
        }
    }

    result
}

/// Split a segment list at `LineBreak` markers into separate lines.
fn split_at_line_breaks(segments: &[InlineSegment]) -> Vec<Vec<&InlineSegment>> {
    let mut lines: Vec<Vec<&InlineSegment>> = vec![vec![]];

    for seg in segments {
        match seg {
            InlineSegment::LineBreak => {
                lines.push(vec![]);
            }
            _ => {
                lines.last_mut().unwrap().push(seg);
            }
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Plain-text extraction (for clipboard copy)
// ---------------------------------------------------------------------------

/// Convert HTML content to plain text suitable for clipboard copy.
///
/// Uses kuchikiki to properly parse the HTML and extract text, inserting
/// line-breaks at block boundaries (`<p>`, `<br>`, `<div>`, etc.).
pub fn html_to_plain_text(html: &str) -> String {
    let sink = kuchikiki::parse_html().one(html);
    let doc = sink.document_node;
    let body = find_body(&doc).unwrap_or(doc);

    let mut result = String::new();
    collect_plain_text(&body, &mut result);
    // Collapse runs of 3+ newlines into 2, then trim.
    let mut prev_was_newline = false;
    let mut consecutive_newlines = 0u32;
    let mut cleaned = String::with_capacity(result.len());
    for ch in result.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                cleaned.push(ch);
            }
            prev_was_newline = true;
        } else {
            consecutive_newlines = 0;
            prev_was_newline = false;
            cleaned.push(ch);
        }
    }
    let _ = prev_was_newline;
    cleaned.trim().to_string()
}

/// Recursively collect plain text from a DOM subtree.
fn collect_plain_text(node: &kuchikiki::NodeRef, out: &mut String) {
    if let Some(text_cell) = node.as_text() {
        out.push_str(&text_cell.borrow());
        return;
    }

    if let Some(el) = node.as_element() {
        let tag = &*el.name.local;

        if tag == "br" {
            out.push('\n');
            return;
        }

        let is_block = matches!(
            tag,
            "p" | "div" | "blockquote" | "pre" | "ul" | "ol" | "li"
                | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                | "hr" | "table" | "section" | "article"
        );

        if is_block && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }

        for child in node.children() {
            collect_plain_text(&child, out);
        }

        if is_block && !out.ends_with('\n') {
            out.push('\n');
        }
    } else {
        for child in node.children() {
            collect_plain_text(&child, out);
        }
    }
}
