//! ICU4X word segmentation for the asynchronous portable search index.
//!
//! Tokenization is intentionally performed in Rust before the indexer tries to
//! acquire SQLite's single writer. FTS5 receives a whitespace-separated token
//! stream and therefore does no language-dependent work in the status write
//! transaction.

use std::collections::HashSet;
use std::fmt::Write;

use icu_casemap::CaseMapper;
use icu_normalizer::ComposingNormalizerBorrowed;
use icu_segmenter::{options::WordBreakInvariantOptions, WordSegmenter, WordSegmenterBorrowed};

#[cfg(test)]
fn tokenize(text: &str) -> Vec<String> {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    tokenize_with_segmenter(text, &segmenter)
}

fn tokenize_with_segmenter(text: &str, segmenter: &WordSegmenterBorrowed<'_>) -> Vec<String> {
    let folded = normalize_and_fold(text);
    let mut boundaries = segmenter.segment_str(&folded).iter_with_word_type();
    let Some((mut start, _)) = boundaries.next() else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    for (end, _word_type) in boundaries {
        if end > start {
            let token = folded[start..end].trim();
            if !token.is_empty() {
                tokens.push(token.to_string());
            }
        }
        start = end;
    }
    tokens
}

fn normalize_and_fold(text: &str) -> String {
    let normalizer = ComposingNormalizerBorrowed::new_nfkc();
    let normalized = normalizer.normalize(text);
    let folded = CaseMapper::new().fold_string(&normalized);
    normalizer.normalize(&folded).into_owned()
}

/// FTS5's built-in `unicode61` tokenizer would split ICU words containing
/// punctuation (for example `can't` or a domain name) a second time. Encode
/// each normalized ICU token as one ASCII-alphanumeric FTS token instead.
/// UTF-8 byte hex also preserves prefix ordering for every Unicode scalar.
fn encode_fts_token(token: &str) -> String {
    let mut encoded = String::with_capacity(1 + token.len().saturating_mul(2));
    encoded.push('x');
    for byte in token.as_bytes() {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

/// Build the compact token stream persisted in `status_search_icu_content`.
/// Fields are segmented separately so a token can never cross a
/// content/URL/tag boundary. Duplicate boolean postings are omitted because
/// the FTS table uses `detail=none` and ranking is not part of selection.
pub fn index_text<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let mut seen = HashSet::new();
    fields
        .into_iter()
        .flat_map(|field| tokenize_with_segmenter(field, &segmenter))
        .filter(|token| seen.insert(token.clone()))
        .map(|token| encode_fts_token(&token))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build an FTS5 boolean prefix expression for one user-visible search term.
/// ICU word boundaries replace arbitrary n-grams; non-whitespace punctuation
/// and emoji segments are tokens too, so every accepted term remains on the
/// indexed path. Prefix matching preserves natural type-ahead without
/// restoring expensive word-internal substrings.
pub fn match_expression(term: &str) -> Option<String> {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let mut seen = HashSet::new();
    let tokens = tokenize_with_segmenter(term, &segmenter)
        .into_iter()
        .filter(|token| seen.insert(token.clone()))
        .map(|token| format!("\"{}\"*", encode_fts_token(&token)))
        .collect::<Vec<_>>();
    (!tokens.is_empty()).then(|| tokens.join(" AND "))
}

/// Apply exactly the same ICU segment-prefix semantics as the FTS candidate
/// path to the bounded set of rows that has not reached the asynchronous index
/// yet. This function must never be used for an unbounded foreground scan.
pub fn matches_fields<'a>(term: &str, fields: impl IntoIterator<Item = &'a str>) -> bool {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let query_tokens = tokenize_with_segmenter(term, &segmenter);
    if query_tokens.is_empty() {
        return false;
    }

    let field_tokens = fields
        .into_iter()
        .flat_map(|field| tokenize_with_segmenter(field, &segmenter))
        .collect::<Vec<_>>();
    query_tokens
        .iter()
        .all(|query| field_tokens.iter().any(|token| token.starts_with(query)))
}

/// Match pre-encoded query tokens against one or more indexed token streams.
/// This is used only after a recent-status window has already been bounded, so
/// recency can be restored without re-running ICU segmentation over content.
pub fn matches_index_text(query_token_text: &str, indexed_fields: &[&str]) -> bool {
    let mut query_tokens = query_token_text.split_whitespace().peekable();
    if query_tokens.peek().is_none() {
        return false;
    }
    query_tokens.all(|query| {
        indexed_fields.iter().any(|field| {
            field
                .split_whitespace()
                .any(|indexed| indexed.starts_with(query))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_segmentation_handles_japanese_words_without_ngrams() {
        assert_eq!(tokenize("こんにちは世界"), vec!["こんにちは", "世界"]);
        let expected = format!(
            "\"{}\"* AND \"{}\"*",
            encode_fts_token("こんにちは"),
            encode_fts_token("世界")
        );
        assert_eq!(
            match_expression("こんにちは世界").as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn compatibility_normalization_and_case_folding_are_shared() {
        assert_eq!(
            index_text(["Ａｗａｙｕｋｉ café"]),
            index_text(["awayuki café"])
        );
        assert_eq!(match_expression("AWAYUKI"), match_expression("awayuki"));
        assert_eq!(match_expression("cafe\u{301}"), match_expression("café"));
        assert_eq!(match_expression("STRASSE"), match_expression("Straße"));
    }

    #[test]
    fn field_boundaries_and_duplicate_tokens_are_compact() {
        assert_eq!(
            index_text(["hello", "world hello"])
                .split_whitespace()
                .count(),
            2
        );
        assert!(match_expression("% 👩‍💻").is_some());
    }

    #[test]
    fn punctuation_inside_icu_words_stays_one_fts_token() {
        let indexed = index_text(["can't example.com"]);
        assert!(indexed.split_whitespace().all(|token| token
            .chars()
            .all(|character| character.is_ascii_alphanumeric())));
        assert!(match_expression("can't").is_some_and(|expression| !expression.contains('\'')));
    }

    #[test]
    fn unindexed_match_uses_the_same_normalization_and_word_prefixes() {
        assert!(matches_fields("STRASSE", ["Straße"]));
        assert!(matches_fields("Ａｗａｙｕｋｉ", ["awayuki"]));
        assert!(matches_fields("東京", ["東京都"]));
        assert!(!matches_fields("yuki", ["awayuki"]));
        assert!(matches_fields("%", ["100% safe"]));
        assert!(matches_fields("👩‍💻", ["shipping 👩‍💻 now"]));
    }

    #[test]
    fn encoded_index_match_preserves_segment_prefixes_across_fields() {
        let query = index_text(["東京 away"]);
        let status = index_text(["東京都"]);
        let account = index_text(["Awayuki"]);
        assert!(matches_index_text(&query, &[&status, &account]));
        let awayuki = index_text(["Awayuki"]);
        assert!(!matches_index_text(&index_text(["yuki"]), &[&awayuki]));
    }
}
