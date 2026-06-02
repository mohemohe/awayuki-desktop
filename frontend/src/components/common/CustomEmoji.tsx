import React from "react";
import type { CustomEmojiSummary } from "../../types/app";
import { customEmojiSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";

const INLINE_RETRY_DELAYS_MS = [800, 1800, 3600, 7000];
const INLINE_RETRY_MAX_CYCLES = 3;
const JUMBOMOJI_MAX_GRAPHEMES = 23;

type GraphemeSegmenter = {
  segment(input: string): Iterable<{ segment: string }>;
};

const graphemeSegmenter = (() => {
  const Segmenter = (
    Intl as unknown as {
      Segmenter?: new (
        locales?: string | string[],
        options?: { granularity: "grapheme" },
      ) => GraphemeSegmenter;
    }
  ).Segmenter;
  return Segmenter
    ? new Segmenter(undefined, { granularity: "grapheme" })
    : null;
})();

const emojiPresentationPattern = /\p{Emoji_Presentation}/u;
const extendedPictographicPattern = /\p{Extended_Pictographic}/u;
const regionalIndicatorPattern = /\p{Regional_Indicator}/u;
const keycapEmojiPattern = /^[0-9#*]\uFE0F?\u20E3$/u;

export function StatusHtmlWithCustomEmojis({
  html,
  emojis,
  className,
  jumbomojiEnabled = false,
}: {
  html: string;
  emojis: CustomEmojiSummary[];
  className?: string;
  jumbomojiEnabled?: boolean;
}) {
  const content = React.useMemo(
    () => renderStatusHtmlWithCustomEmojisResult(html, emojis, jumbomojiEnabled),
    [emojis, html, jumbomojiEnabled],
  );
  const ref = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    if (!ref.current) return;
    return enhanceInlineCustomEmojiImages(ref.current);
  }, [content]);

  return (
    <div
      ref={ref}
      className={[
        className,
        content.jumbomoji ? "status-content-jumbomoji" : undefined,
      ]
        .filter(Boolean)
        .join(" ")}
      dangerouslySetInnerHTML={{ __html: content.html }}
    />
  );
}

export function CustomEmojiText({
  text,
  emojis,
}: {
  text: string;
  emojis: CustomEmojiSummary[];
}) {
  return <>{renderCustomEmojiText(text, emojis)}</>;
}

export function renderStatusHtmlWithCustomEmojis(
  html: string,
  emojis: CustomEmojiSummary[],
) {
  return renderStatusHtmlWithCustomEmojisResult(html, emojis, false).html;
}

function renderStatusHtmlWithCustomEmojisResult(
  html: string,
  emojis: CustomEmojiSummary[],
  jumbomojiEnabled: boolean,
) {
  if (!html || typeof document === "undefined") {
    return { html, jumbomoji: false };
  }

  const template = document.createElement("template");
  template.innerHTML = html;

  if (emojis.length) {
    const pattern = customEmojiPattern(emojis);
    if (pattern) {
      replaceCustomEmojiTextNodes(
        template.content,
        pattern,
        emojiByShortcode(emojis),
      );
    }
  }

  return {
    html: template.innerHTML,
    jumbomoji: jumbomojiEnabled && isJumbomojiContent(template.content),
  };
}

function renderCustomEmojiText(text: string, emojis: CustomEmojiSummary[]) {
  if (!text || !emojis.length) return text;

  const pattern = customEmojiPattern(emojis);
  if (!pattern) return text;

  const lookup = emojiByShortcode(emojis);
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text))) {
    const shortcode = match[1];
    const emoji = lookup.get(shortcode);
    if (!emoji) continue;
    if (match.index > cursor) nodes.push(text.slice(cursor, match.index));
    nodes.push(
      <RetriedCustomEmojiImage
        key={`${shortcode}-${match.index}`}
        emoji={emoji}
        className="status-custom-emoji"
        alt={`:${shortcode}:`}
        title={`:${shortcode}:`}
      />,
    );
    cursor = match.index + match[0].length;
  }

  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes.length ? nodes : text;
}

export function RetriedCustomEmojiImage({
  emoji,
  className,
  alt,
  title,
}: {
  emoji: CustomEmojiSummary;
  className?: string;
  alt?: string;
  title?: string;
}) {
  const image = useRetriedMediaSource(customEmojiSources(emoji));
  const label = alt ?? `:${emoji.shortcode}:`;

  if (!image.src || image.failed) {
    return (
      <span className="status-custom-emoji-fallback" title={title ?? label}>
        {label}
      </span>
    );
  }

  return (
    <img
      key={image.key}
      className={`${className ?? ""} ${image.loaded ? "" : "opacity-0"}`}
      src={image.src}
      alt={label}
      title={title ?? label}
      loading="lazy"
      onLoad={image.onLoad}
      onError={image.onError}
    />
  );
}

function replaceCustomEmojiTextNodes(
  root: ParentNode,
  pattern: RegExp,
  lookup: Map<string, CustomEmojiSummary>,
) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  let node = walker.nextNode();
  while (node) {
    if (node.parentElement && shouldSkipNode(node.parentElement)) {
      node = walker.nextNode();
      continue;
    }
    nodes.push(node as Text);
    node = walker.nextNode();
  }

  for (const textNode of nodes) {
    const text = textNode.nodeValue ?? "";
    pattern.lastIndex = 0;
    if (!pattern.test(text)) continue;
    pattern.lastIndex = 0;

    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(text))) {
      const shortcode = match[1];
      const emoji = lookup.get(shortcode);
      if (!emoji) continue;
      if (match.index > cursor) {
        fragment.append(document.createTextNode(text.slice(cursor, match.index)));
      }
      fragment.append(createCustomEmojiImage(emoji));
      cursor = match.index + match[0].length;
    }
    if (cursor < text.length) {
      fragment.append(document.createTextNode(text.slice(cursor)));
    }
    textNode.replaceWith(fragment);
  }
}

function createCustomEmojiImage(emoji: CustomEmojiSummary) {
  const img = document.createElement("img");
  img.className = "status-custom-emoji";
  const sources = customEmojiSources(emoji);
  img.src = sources[0] ?? emoji.url;
  img.dataset.sources = sources.join("\n");
  img.alt = `:${emoji.shortcode}:`;
  img.title = `:${emoji.shortcode}:`;
  img.loading = "lazy";
  return img;
}

function enhanceInlineCustomEmojiImages(root: ParentNode) {
  const cleanups = Array.from(
    root.querySelectorAll<HTMLImageElement>("img.status-custom-emoji"),
  ).map(enhanceInlineCustomEmojiImage);

  return () => {
    for (const cleanup of cleanups) cleanup();
  };
}

function enhanceInlineCustomEmojiImage(img: HTMLImageElement) {
  const sources = (img.dataset.sources ?? img.src)
    .split("\n")
    .map((source) => source.trim())
    .filter(Boolean);
  if (!sources.length) return () => {};

  let sourceIndex = 0;
  let attempt = 0;
  let cycle = 0;
  let retryTimer: number | null = null;
  let cancelled = false;
  let fallback: Text | null = null;

  const clearRetryTimer = () => {
    if (retryTimer === null) return;
    window.clearTimeout(retryTimer);
    retryTimer = null;
  };

  const markLoaded = () => {
    clearRetryTimer();
    img.style.opacity = "";
    if (fallback) {
      fallback.remove();
      fallback = null;
    }
  };

  const markFailed = () => {
    img.style.display = "none";
    if (!fallback) {
      fallback = document.createTextNode(img.alt || img.title || "");
      img.after(fallback);
    }
  };

  const nextRetryDelay = () => {
    const baseDelay = INLINE_RETRY_DELAYS_MS[attempt];
    if (baseDelay !== undefined) {
      attempt += 1;
      return Math.min(baseDelay * 2 ** cycle, 30_000);
    }
    if (sourceIndex + 1 < sources.length) {
      sourceIndex += 1;
      attempt = 0;
      return 0;
    }
    if (cycle + 1 < INLINE_RETRY_MAX_CYCLES) {
      cycle += 1;
      sourceIndex = 0;
      attempt = 0;
      return 0;
    }
    return null;
  };

  const probeCurrentSource = () => {
    if (cancelled) return;
    const probe = new Image();
    probe.onload = () => {
      if (cancelled) return;
      img.src = sources[sourceIndex];
      markLoaded();
    };
    probe.onerror = () => {
      if (cancelled) return;
      queueRetry();
    };
    probe.src = sources[sourceIndex];
  };

  const queueRetry = () => {
    clearRetryTimer();
    img.style.opacity = "0";
    const delay = nextRetryDelay();
    if (delay === null) {
      markFailed();
      return;
    }
    retryTimer = window.setTimeout(probeCurrentSource, delay);
  };

  img.addEventListener("load", markLoaded);
  img.addEventListener("error", queueRetry);

  if (img.complete && img.naturalWidth === 0) {
    queueRetry();
  }

  return () => {
    cancelled = true;
    clearRetryTimer();
    img.removeEventListener("load", markLoaded);
    img.removeEventListener("error", queueRetry);
    if (fallback) fallback.remove();
  };
}

function isJumbomojiContent(root: ParentNode) {
  let graphemes = 0;
  let valid = true;

  const visit = (node: Node) => {
    if (!valid) return;
    if (node.nodeType === Node.TEXT_NODE) {
      for (const segment of segmentGraphemes(node.textContent ?? "")) {
        if (!segment.trim()) continue;
        if (!isEmojiGrapheme(segment)) {
          valid = false;
          return;
        }
        graphemes += 1;
        if (graphemes > JUMBOMOJI_MAX_GRAPHEMES) {
          valid = false;
          return;
        }
      }
      return;
    }

    if (node.nodeType !== Node.ELEMENT_NODE) {
      return;
    }

    const element = node as Element;
    if (isEmojiImage(element)) {
      graphemes += 1;
      if (graphemes > JUMBOMOJI_MAX_GRAPHEMES) valid = false;
      return;
    }

    if (element.tagName === "BR") {
      return;
    }

    for (const child of element.childNodes) visit(child);
  };

  for (const child of root.childNodes) visit(child);
  return valid && graphemes > 0;
}

function segmentGraphemes(value: string) {
  if (!graphemeSegmenter) return Array.from(value);
  return Array.from(graphemeSegmenter.segment(value), (item) => item.segment);
}

function isEmojiGrapheme(value: string) {
  return (
    keycapEmojiPattern.test(value) ||
    emojiPresentationPattern.test(value) ||
    extendedPictographicPattern.test(value) ||
    regionalIndicatorPattern.test(value)
  );
}

function isEmojiImage(element: Element) {
  if (element.tagName !== "IMG") return false;
  return (
    element.classList.contains("status-custom-emoji") ||
    element.classList.contains("emoji") ||
    element.classList.contains("emojione") ||
    element.classList.contains("custom-emoji")
  );
}

function customEmojiPattern(emojis: CustomEmojiSummary[]) {
  const shortcodes = [...new Set(emojis.map((emoji) => emoji.shortcode).filter(Boolean))];
  if (!shortcodes.length) return null;
  shortcodes.sort((a, b) => b.length - a.length);
  return new RegExp(`:(${shortcodes.map(escapeRegExp).join("|")}):`, "g");
}

function emojiByShortcode(emojis: CustomEmojiSummary[]) {
  return new Map(emojis.map((emoji) => [emoji.shortcode, emoji]));
}

function shouldSkipNode(element: Element) {
  return Boolean(
    element.closest("script, style, textarea, code, pre, img, video, audio"),
  );
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
