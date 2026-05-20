import React from "react";
import type { CustomEmojiSummary } from "../../types/app";
import { customEmojiSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";

const INLINE_RETRY_DELAYS_MS = [800, 1800, 3600, 7000];
const INLINE_RETRY_MAX_CYCLES = 3;

export function StatusHtmlWithCustomEmojis({
  html,
  emojis,
  className,
}: {
  html: string;
  emojis: CustomEmojiSummary[];
  className?: string;
}) {
  const content = React.useMemo(
    () => renderStatusHtmlWithCustomEmojis(html, emojis),
    [emojis, html],
  );
  const ref = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    if (!ref.current) return;
    return enhanceInlineCustomEmojiImages(ref.current);
  }, [content]);

  return (
    <div
      ref={ref}
      className={className}
      dangerouslySetInnerHTML={{ __html: content }}
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
  if (!html || !emojis.length || typeof document === "undefined") return html;

  const pattern = customEmojiPattern(emojis);
  if (!pattern) return html;

  const template = document.createElement("template");
  template.innerHTML = html;
  replaceCustomEmojiTextNodes(template.content, pattern, emojiByShortcode(emojis));
  return template.innerHTML;
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
