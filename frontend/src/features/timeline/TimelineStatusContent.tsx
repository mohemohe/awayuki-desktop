import React from "react";
import { Languages } from "lucide-react";
import { invokeTypedCommand } from "../../api/tauri";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { AppearanceSettings, TimelineStatus } from "../../types/app";
import { getClientPlatform } from "../../utils/browser";
import { formatTime, htmlToPlainText } from "../../utils/format";
import { thumbnailMediaSources } from "../../utils/media";
import { Avatar } from "../../components/common/Avatar";
import {
  CustomEmojiText,
  StatusHtmlWithCustomEmojis,
} from "../../components/common/CustomEmoji";
import {
  languageDisplayName,
  shouldOfferTranslation,
  targetTranslationLanguage,
  translatedTextToHtml,
  translationCache,
  translationCacheKey,
} from "./translation";
import { MediaThumbnail, statusDisplayCreatedAt } from "./TimelineMedia";
import {
  translationScheduler,
  type TranslationLease,
} from "./translationScheduler";

type TranslationState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "translated"; text: string; sourceLanguage?: string | null }
  | { kind: "error"; message: string };

export function QuotePreview({
  status,
  onOpenUser,
  onOpenStatus,
  onOpenMedia,
}: {
  status: TimelineStatus;
  onOpenUser: (status: TimelineStatus) => void;
  onOpenStatus: (status: TimelineStatus) => void;
  onOpenMedia: (
    status: TimelineStatus,
    media: TimelineStatus["media"][number],
  ) => void;
}) {
  const cwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.cw_behavior ?? "Hide",
  );
  const nsfwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.nsfw_behavior ?? "Hide",
  );
  const mediaSourcePreference = useAppStore(
    (state) => state.snapshot?.settings.confirmation.media_source ?? "Local",
  );
  const [mediaVisibility, setMediaVisibility] = React.useState<
    Record<string, boolean>
  >({});

  return (
    <div className="mt-2 max-w-full overflow-hidden rounded border border-surface1 bg-base-300/50 p-2">
      <div className="flex min-w-0 items-center gap-2">
        <button
          className="shrink-0"
          onClick={() => onOpenUser(status)}
          title={t("Open profile")}
        >
          <Avatar src={status.avatar} label={status.displayName} size="md" />
        </button>
        <button
          className="min-w-0 flex-1 truncate text-left text-xs font-semibold hover:text-blue"
          onClick={() => onOpenUser(status)}
          title={t("Open profile")}
        >
          <CustomEmojiText
            text={status.displayName || status.acct}
            emojis={status.accountEmojis}
          />
        </button>
        <button
          className="shrink-0 text-xs text-overlay0 hover:text-blue"
          onClick={() => onOpenStatus(status)}
          title={t("Open quoted post")}
        >
          {formatTime(statusDisplayCreatedAt(status))}
        </button>
      </div>
      <div className="mt-1 truncate text-xs text-subtext0">{status.acct}</div>
      <StatusContentBlock
        status={status}
        cwBehavior={cwBehavior}
        className="mt-1 max-w-full font-extralight"
      />
      {status.media.length ? (
        <div className="mt-2 grid grid-cols-2 gap-1">
          {status.media.slice(0, 4).map((media) => {
            const sources = thumbnailMediaSources(
              media,
              mediaSourcePreference,
            );
            return sources.length ? (
              <MediaThumbnail
                key={media.id}
                media={media}
                sources={sources}
                sensitive={status.sensitive}
                visible={
                  mediaVisibility[media.id] ?? nsfwBehavior === "AlwaysShow"
                }
                onToggle={() =>
                  setMediaVisibility((current) => ({
                    ...current,
                    [media.id]: !(
                      current[media.id] ?? nsfwBehavior === "AlwaysShow"
                    ),
                  }))
                }
                onOpen={() => onOpenMedia(status, media)}
              />
            ) : null;
          })}
        </div>
      ) : null}
    </div>
  );
}

export function StatusContentBlock({
  status,
  cwBehavior,
  className,
}: {
  status: TimelineStatus;
  cwBehavior: AppearanceSettings["cw_behavior"];
  className?: string;
}) {
  const behavior = useAppStore((state) => state.snapshot?.settings.confirmation);
  const translationEnabled = behavior?.translate_enabled ?? false;
  const autoTranslationEnabled = behavior?.auto_translate_enabled ?? false;
  const translationEngine = behavior?.translation_engine ?? "TranslationFramework";
  const jumbomojiEnabled = behavior?.jumbomoji_enabled ?? false;
  const translationSupported = getClientPlatform() === "macos";
  const targetLanguage = targetTranslationLanguage();
  const plainText = React.useMemo(
    () => htmlToPlainText(status.content),
    [status.content],
  );
  const cacheKey = translationCacheKey(status, targetLanguage, translationEngine);
  const [translation, setTranslation] = React.useState<TranslationState>(() => {
    const cached = translationCache.get(cacheKey);
    return cached
      ? {
          kind: "translated",
          text: cached.text,
          sourceLanguage: cached.sourceLanguage,
        }
      : { kind: "idle" };
  });
  const [showTranslated, setShowTranslated] = React.useState(() =>
    translationCache.has(cacheKey),
  );
  const [translationRoot, setTranslationRoot] = React.useState<HTMLElement | null>(
    null,
  );
  const [isVisible, setIsVisible] = React.useState(
    () => typeof IntersectionObserver === "undefined",
  );
  const translationLeaseRef = React.useRef<
    TranslationLease<{
      text: string;
      sourceLanguage?: string | null;
      targetLanguage: string;
    }> | null
  >(null);
  const translationGenerationRef = React.useRef(0);
  const contentRootRef = React.useCallback((node: HTMLElement | null) => {
    setTranslationRoot(node);
  }, []);
  const spoilerText = status.spoilerText.trim();
  const canTranslate =
    translationEnabled && shouldOfferTranslation(status, plainText);
  const translated =
    canTranslate && translation.kind === "translated" && showTranslated
      ? translation
      : undefined;

  React.useEffect(() => {
    translationGenerationRef.current += 1;
    translationLeaseRef.current?.cancel();
    translationLeaseRef.current = null;
    const cached = translationCache.get(cacheKey);
    if (cached) {
      setTranslation({
        kind: "translated",
        text: cached.text,
        sourceLanguage: cached.sourceLanguage,
      });
    } else {
      setTranslation({ kind: "idle" });
      setShowTranslated(false);
    }
    return () => {
      translationGenerationRef.current += 1;
      translationLeaseRef.current?.cancel();
      translationLeaseRef.current = null;
    };
  }, [cacheKey]);

  React.useEffect(() => {
    if (!translationRoot || typeof IntersectionObserver === "undefined") {
      setIsVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      ([entry]) => setIsVisible(entry?.isIntersecting ?? false),
      { rootMargin: "200px" },
    );
    observer.observe(translationRoot);
    return () => observer.disconnect();
  }, [translationRoot]);

  const translate = React.useCallback(async (priority = 100) => {
    if (!translationSupported || !plainText.trim()) return;
    const cached = translationCache.get(cacheKey);
    if (cached) {
      setTranslation({
        kind: "translated",
        text: cached.text,
        sourceLanguage: cached.sourceLanguage,
      });
      setShowTranslated(true);
      return;
    }

    setTranslation({ kind: "loading" });
    const generation = ++translationGenerationRef.current;
    translationLeaseRef.current?.cancel();
    const lease = translationScheduler.schedule(
      cacheKey,
      () =>
        invokeTypedCommand("translate_status_text", {
          request: {
            text: plainText,
            sourceLanguage: status.language ?? null,
            targetLanguage,
            translationEngine,
          },
        }),
      priority,
    );
    translationLeaseRef.current = lease;
    try {
      const response = await lease.promise;
      if (translationGenerationRef.current !== generation) return;
      const next = {
        text: response.text.trim(),
        sourceLanguage: response.sourceLanguage ?? status.language ?? null,
      };
      translationCache.set(cacheKey, next);
      setTranslation({
        kind: "translated",
        text: next.text,
        sourceLanguage: next.sourceLanguage,
      });
      setShowTranslated(true);
    } catch (error) {
      if (translationGenerationRef.current !== generation) return;
      setTranslation({
        kind: "error",
        message: error instanceof Error ? error.message : String(error),
      });
      setShowTranslated(false);
    } finally {
      if (translationGenerationRef.current === generation) {
        translationLeaseRef.current = null;
      }
    }
  }, [
    cacheKey,
    plainText,
    status.language,
    targetLanguage,
    translationEngine,
    translationSupported,
  ]);

  React.useEffect(() => {
    if (
      !canTranslate ||
      !translationSupported ||
      !autoTranslationEnabled ||
      !isVisible ||
      translation.kind !== "idle"
    ) {
      return;
    }
    void translate(10);
  }, [
    autoTranslationEnabled,
    canTranslate,
    isVisible,
    translate,
    translation.kind,
    translationSupported,
  ]);

  const translationMeta = canTranslate ? (
    <div className="mb-1 flex min-w-0 flex-wrap items-center gap-1.5 text-xs text-subtext0">
      <Languages className="h-3.5 w-3.5 shrink-0" />
      {!translationSupported ? (
        <span>{t("Translation is not supported on this OS.")}</span>
      ) : translated ? (
        <>
          <span>
            {t("Translated from {language}", {
              language: languageDisplayName(
                translated.sourceLanguage ?? status.language,
              ),
            })}
          </span>
          <button
            type="button"
            className="font-semibold text-blue hover:underline"
            onClick={() => setShowTranslated(false)}
          >
            {t("Show original")}
          </button>
        </>
      ) : (
        <>
          <button
            type="button"
            className="inline-flex items-center gap-1 font-semibold text-blue hover:underline disabled:cursor-wait disabled:text-subtext0"
            disabled={translation.kind === "loading"}
            onClick={() => void translate()}
          >
            {translation.kind === "loading"
              ? t("Translating...")
              : t("Show translation")}
          </button>
          {translation.kind === "error" ? (
            <span className="text-red">
              {t("Translation failed")}: {translation.message}
            </span>
          ) : null}
        </>
      )}
    </div>
  ) : null;
  const contentHtml = translated
    ? translatedTextToHtml(translated.text)
    : status.content;
  const contentEmojis = translated ? [] : status.emojis;
  const content = (
    <>
      {translationMeta}
      <StatusHtmlWithCustomEmojis
        className="status-content"
        html={contentHtml}
        emojis={contentEmojis}
        jumbomojiEnabled={jumbomojiEnabled}
      />
    </>
  );

  if (!spoilerText) {
    return (
      <div ref={contentRootRef} className={className}>
        {content}
      </div>
    );
  }

  if (cwBehavior === "AlwaysExpand") {
    return (
      <div
        ref={contentRootRef}
        className={`status-cw-collapse collapse collapse-open border border-surface0 bg-base-300/50 ${className ?? ""}`}
      >
        <div className="collapse-title min-h-0 px-3 py-2 text-sm font-semibold text-warning">
          {spoilerText}
        </div>
        <div className="collapse-content px-3 pb-3">{content}</div>
      </div>
    );
  }

  return (
    <details
      ref={contentRootRef}
      className={`status-cw-collapse collapse collapse-arrow border border-surface0 bg-base-300/50 ${className ?? ""}`}
    >
      <summary className="collapse-title min-h-0 px-3 py-2 text-sm font-semibold text-warning">
        {spoilerText}
      </summary>
      <div className="collapse-content px-3 pb-3">{content}</div>
    </details>
  );
}
