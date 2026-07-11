import { statusKey } from "../../domain/timelineEntities";
import { appLocale, t } from "../../i18n";
import type { ConfirmationSettings, TimelineStatus } from "../../types/app";
import { LruCache } from "../../utils/lru";

export type CachedTranslation = {
  text: string;
  sourceLanguage?: string | null;
};

export type TranslationEngine = ConfirmationSettings["translation_engine"];

export const translationCache = new LruCache<string, CachedTranslation>(500, {
  ttlMs: 60 * 60 * 1000,
  maxWeight: 2 * 1024 * 1024,
  weight: (value) => value.text.length * 2 + 64,
});

export function targetTranslationLanguage() {
  return appLocale === "ja" ? "ja" : "en";
}

export function shouldOfferTranslation(
  status: TimelineStatus,
  plainText: string,
) {
  if (!plainText.trim()) return false;
  const language = status.language?.trim().toLowerCase();
  if (!language) return true;
  return appLocale === "ja"
    ? !language.startsWith("ja")
    : !language.startsWith("en");
}

export function translationCacheKey(
  status: TimelineStatus,
  targetLanguage: string,
  translationEngine: TranslationEngine,
) {
  return `${statusKey(status)}:${targetLanguage}:${translationEngine}:${hashString(status.content)}`;
}

export function languageDisplayName(language?: string | null) {
  const value = language?.trim();
  if (!value) return t("Unknown language");
  try {
    return (
      new Intl.DisplayNames([appLocale], { type: "language" }).of(value) ?? value
    );
  } catch {
    return value;
  }
}

export function translatedTextToHtml(text: string) {
  return text
    .trim()
    .split(/\n{2,}/)
    .map(
      (paragraph) =>
        `<p>${escapeHtml(paragraph).replace(/\n/g, "<br>")}</p>`,
    )
    .join("");
}

function hashString(value: string) {
  let hash = 0;
  for (let index = 0; index < value.length; index += 1) {
    hash = (hash * 31 + value.charCodeAt(index)) | 0;
  }
  return hash.toString(36);
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

