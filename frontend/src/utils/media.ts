import type { CustomEmojiSummary, MediaAttachment } from "../types/app";

export type MediaSourcePreference = "Local" | "Remote";

export function isVideoMedia(media: MediaAttachment) {
  return [media.media_type, media.type].some((mediaType) => {
    const normalized = mediaType?.trim().toLowerCase();
    return normalized === "gifv" || normalized?.startsWith("video");
  });
}

export function isAudioMedia(media: MediaAttachment) {
  return [media.media_type, media.type].some((mediaType) =>
    mediaType?.trim().toLowerCase().startsWith("audio"),
  );
}

export type MediaKind = "image" | "video" | "audio" | "unknown";

export function mediaKind(media: MediaAttachment): MediaKind {
  if (isVideoMedia(media)) return "video";
  if (isAudioMedia(media)) return "audio";
  const declaredTypes = [media.media_type, media.type]
    .map((mediaType) => mediaType?.trim().toLowerCase())
    .filter((mediaType): mediaType is string => Boolean(mediaType));
  if (declaredTypes.length === 0) return "image";
  return declaredTypes.some(
    (mediaType) => mediaType === "image" || mediaType.startsWith("image/"),
  )
    ? "image"
    : "unknown";
}

export function uniqueMediaSources(
  sources: Array<string | null | undefined>,
) {
  const seen = new Set<string>();
  return sources.filter((source): source is string => {
    const value = source?.trim();
    if (!value || seen.has(value)) return false;
    seen.add(value);
    return true;
  });
}

function mediaUrlPathname(source: string) {
  let pathname: string;
  try {
    pathname = new URL(source, "https://awayuki.invalid").pathname;
  } catch {
    [pathname = source] = source.split(/[?#]/, 1);
  }
  try {
    pathname = decodeURIComponent(pathname);
  } catch {
    // A malformed escape must not prevent the conservative extension check.
  }
  return pathname;
}

export function isLikelyNonImageMediaUrl(source: string) {
  const pathname = mediaUrlPathname(source);
  return /\.(?:mp4|webm|mov|m4v|m3u8|mkv|avi|mp3|m4a|aac|ogg|oga|opus|wav|flac)$/i.test(
    pathname,
  );
}

export function isAnimatedImageMedia(media: MediaAttachment) {
  if (isVideoMedia(media) || isAudioMedia(media)) return false;
  const hasAnimatedMime = [media.media_type, media.type].some((mediaType) => {
    const normalized = mediaType?.trim().toLowerCase();
    return normalized === "image/gif" || normalized === "image/apng";
  });
  if (hasAnimatedMime) return true;
  return [media.url, media.remote_url].some(
    (source) =>
      typeof source === "string" &&
      /\.(?:gif|apng)$/i.test(mediaUrlPathname(source)),
  );
}

export function thumbnailMediaSources(
  media: MediaAttachment,
  preference: MediaSourcePreference = "Local",
) {
  // Timeline thumbnails are always rendered with <img>. Never fall back from
  // a non-image preview to the original media URL: WebKit can attempt to decode
  // every MP4 frame as an image before it reports the format failure.
  if (mediaKind(media) !== "image") {
    const [preview] = uniqueMediaSources([media.preview_url]);
    if (!preview) return [];
    const normalizedPreview = preview.trim();
    const aliasesOriginal = [media.url, media.remote_url].some(
      (source) => source?.trim() === normalizedPreview,
    );
    return aliasesOriginal || isLikelyNonImageMediaUrl(preview)
      ? []
      : [preview];
  }
  if (isAnimatedImageMedia(media)) {
    return uniqueMediaSources(
      preference === "Remote"
        ? [media.remote_url, media.url, media.preview_url]
        : [media.url, media.remote_url, media.preview_url],
    );
  }
  return uniqueMediaSources(
    preference === "Remote"
      ? [media.remote_url, media.preview_url, media.url]
      : [media.preview_url, media.url, media.remote_url],
  );
}

export function previewMediaSources(
  media: MediaAttachment,
  video = false,
  preference: MediaSourcePreference = "Local",
) {
  if (video) {
    return uniqueMediaSources(
      preference === "Remote"
        ? [media.remote_url, media.url]
        : [media.url, media.remote_url],
    );
  }

  return uniqueMediaSources(
    preference === "Remote"
      ? [media.remote_url, media.url, media.preview_url]
      : [media.url, media.preview_url, media.remote_url],
  );
}

export function customEmojiSources(emoji: CustomEmojiSummary) {
  return uniqueMediaSources([emoji.url, emoji.staticUrl]);
}
