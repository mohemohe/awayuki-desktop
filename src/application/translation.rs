//! Status translation use case.
//!
//! Translation has no dependency on the desktop runtime, Tauri state, or the
//! portable database. Keeping the platform adapters behind this application
//! boundary prevents a settings IPC handler from owning macOS framework and
//! Foundation Model orchestration directly.

#[cfg(target_os = "macos")]
use apple_ai::{AppleAiClient, GenerationOptions, Message};
use serde::Serialize;

use crate::ipc::dto::TranslateStatusRequest;
#[cfg(any(target_os = "macos", test))]
use crate::state::confirmation::TranslationEngine;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslateStatusResponse {
    text: String,
    source_language: Option<String>,
    target_language: String,
}

pub(crate) async fn translate_status_text(
    request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, String> {
    translate_status_text_impl(request).await
}

#[cfg(target_os = "macos")]
async fn translate_status_text_impl(
    request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, String> {
    let request = prepare_translation_request(request)?;
    match request.translation_engine {
        TranslationEngine::FoundationModel => translate_with_foundation_model(request).await,
        TranslationEngine::TranslationFramework => {
            translate_with_translation_framework(request).await
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct PreparedTranslationRequest {
    text: String,
    source_language: Option<String>,
    target_language: String,
    translation_engine: TranslationEngine,
}

#[cfg(any(target_os = "macos", test))]
fn prepare_translation_request(
    request: TranslateStatusRequest,
) -> Result<PreparedTranslationRequest, String> {
    let text = request.text.trim().to_string();
    if text.is_empty() {
        return Err("Text to translate is empty".to_string());
    }

    let target_language = normalized_language_identifier(&request.target_language);
    if target_language.is_empty() {
        return Err("Target language is empty".to_string());
    }

    let source_language = request
        .source_language
        .as_deref()
        .map(str::trim)
        .map(normalized_language_identifier)
        .filter(|value| !value.is_empty());

    Ok(PreparedTranslationRequest {
        text,
        source_language,
        target_language,
        translation_engine: request.translation_engine.unwrap_or_default(),
    })
}

#[cfg(target_os = "macos")]
async fn translate_with_foundation_model(
    request: PreparedTranslationRequest,
) -> Result<TranslateStatusResponse, String> {
    let source_hint = request.source_language.as_deref().unwrap_or("auto-detect");
    let target_label = translation_language_label(&request.target_language);
    let prompt = format!(
        "Source language: {source_hint}\nTarget language: {target_label}\n\nText:\n{}",
        request.text
    );

    let client =
        AppleAiClient::new().map_err(|error| format!("Translation unavailable: {error}"))?;
    let response = client
        .generate(
            vec![
                Message::system(
                    "You are a translation engine. Translate the user's social-media post text faithfully. Preserve line breaks, mentions, hashtags, URLs, emoji, and punctuation. Return only the translated text without explanations, quotes, language labels, or markdown.",
                ),
                Message::user(prompt),
            ],
            GenerationOptions::default().temperature(0.0),
        )
        .await
        .map_err(|error| format!("Translation failed: {error}"))?;
    let translated = response.text.trim().to_string();
    if translated.is_empty() {
        return Err("Translation returned empty text".to_string());
    }

    Ok(TranslateStatusResponse {
        text: translated,
        source_language: request.source_language,
        target_language: request.target_language,
    })
}

#[cfg(target_os = "macos")]
async fn translate_with_translation_framework(
    request: PreparedTranslationRequest,
) -> Result<TranslateStatusResponse, String> {
    tokio::task::spawn_blocking(move || translate_with_translation_framework_blocking(request))
        .await
        .map_err(|error| format!("Translation failed: {error}"))?
}

#[cfg(target_os = "macos")]
fn translate_with_translation_framework_blocking(
    request: PreparedTranslationRequest,
) -> Result<TranslateStatusResponse, String> {
    let source_language = request.source_language.clone().map(Ok).unwrap_or_else(|| {
        translation::detect_language(&request.text)
            .map_err(|error| format!("Language detection failed: {error}"))?
            .ok_or_else(|| "Source language could not be detected".to_string())
    })?;
    let source = translation::Language::new(source_language)
        .canonicalized()
        .map_err(|error| format!("Invalid source language: {error}"))?;
    let target = translation::Language::new(request.target_language.clone())
        .canonicalized()
        .map_err(|error| format!("Invalid target language: {error}"))?;
    let config =
        translation::TranslationSessionConfiguration::new(source.identifier(), target.identifier());
    let session = translation::TranslationSession::new(config)
        .map_err(|error| format!("Translation unavailable: {error}"))?;

    if !session
        .is_ready()
        .map_err(|error| format!("Translation readiness check failed: {error}"))?
    {
        session
            .prepare_translation()
            .map_err(|error| format!("Translation preparation failed: {error}"))?;
    }

    let response = session
        .translate(&request.text)
        .map_err(|error| format!("Translation failed: {error}"))?;
    let translated = response.target_text().trim().to_string();
    if translated.is_empty() {
        return Err("Translation returned empty text".to_string());
    }

    Ok(TranslateStatusResponse {
        text: translated,
        source_language: Some(response.source_language().to_string()),
        target_language: response.target_language().to_string(),
    })
}

#[cfg(any(target_os = "macos", test))]
fn normalized_language_identifier(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "english" => "en".to_string(),
        "japanese" => "ja".to_string(),
        _ => value.trim().to_string(),
    }
}

#[cfg(target_os = "macos")]
fn translation_language_label(identifier: &str) -> &str {
    match identifier.trim().to_lowercase().as_str() {
        "en" | "en-us" | "en-gb" => "English",
        "ja" | "ja-jp" => "Japanese",
        _ => identifier,
    }
}

#[cfg(not(target_os = "macos"))]
async fn translate_status_text_impl(
    _request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, String> {
    Err("Translation is only supported on macOS.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_request_normalizes_languages_without_touching_content() {
        let prepared = prepare_translation_request(TranslateStatusRequest {
            text: "  hello @awayuki #tag  ".to_string(),
            source_language: Some(" English ".to_string()),
            target_language: " Japanese ".to_string(),
            translation_engine: None,
        })
        .expect("prepare translation request");

        assert_eq!(prepared.text, "hello @awayuki #tag");
        assert_eq!(prepared.source_language.as_deref(), Some("en"));
        assert_eq!(prepared.target_language, "ja");
        assert_eq!(
            prepared.translation_engine,
            TranslationEngine::TranslationFramework
        );
    }

    #[test]
    fn translation_request_rejects_empty_text_and_target() {
        let request = TranslateStatusRequest {
            text: "   ".to_string(),
            source_language: None,
            target_language: "ja".to_string(),
            translation_engine: None,
        };
        assert!(prepare_translation_request(request).is_err());

        let request = TranslateStatusRequest {
            text: "hello".to_string(),
            source_language: None,
            target_language: "   ".to_string(),
            translation_engine: None,
        };
        assert!(prepare_translation_request(request).is_err());
    }
}
