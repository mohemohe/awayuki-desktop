import type { CustomEmojiSummary, MediaAttachment } from "../types/app";

export type MediaSourcePreference = "Local" | "Remote";

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

export function thumbnailMediaSources(
  media: MediaAttachment,
  preference: MediaSourcePreference = "Local",
) {
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
