import { describe, expect, it } from "vitest";
import type { MediaAttachment } from "../types/app";
import {
  isAnimatedImageMedia,
  isAudioMedia,
  isLikelyNonImageMediaUrl,
  isVideoMedia,
  mediaKind,
  previewMediaSources,
  thumbnailMediaSources,
} from "./media";

const gifvAttachment: MediaAttachment = {
  id: "116940484718637238",
  type: "gifv",
  url: "https://media.example/original/video.mp4",
  preview_url: "https://media.example/small/preview.jpg",
};

describe("media preview source selection", () => {
  it("treats Mastodon gifv attachments as playable video", () => {
    expect(isVideoMedia(gifvAttachment)).toBe(true);
  });

  it("selects the original video instead of the gifv thumbnail", () => {
    expect(
      previewMediaSources(gifvAttachment, isVideoMedia(gifvAttachment)),
    ).toEqual(["https://media.example/original/video.mp4"]);
  });

  it.each(["Local", "Remote"] as const)(
    "keeps %s gifv thumbnails on the image preview URL only",
    (preference) => {
      expect(thumbnailMediaSources(gifvAttachment, preference)).toEqual([
        "https://media.example/small/preview.jpg",
      ]);
    },
  );

  it("does not fall back to a video original when its image preview is absent", () => {
    expect(
      thumbnailMediaSources({
        ...gifvAttachment,
        preview_url: null,
        remote_url: "https://remote.example/original/video.mp4",
      }),
    ).toEqual([]);
  });

  it.each(["url", "remote_url"] as const)(
    "rejects a video preview URL that aliases the %s original",
    (originalField) => {
      const original = "https://media.example/original/video.mp4";
      expect(
        thumbnailMediaSources({
          ...gifvAttachment,
          url: original,
          remote_url: "https://remote.example/original/video.mp4",
          preview_url:
            originalField === "url"
              ? original
              : "https://remote.example/original/video.mp4",
        }),
      ).toEqual([]);
    },
  );

  it.each([
    "https://media.example/preview.mp4?token=signed#frame",
    "https://media.example/preview.MP3?download=1",
    "https://media.example/preview%2Ewebm?token=signed",
  ])("rejects a non-image preview URL despite query or hash: %s", (preview) => {
    expect(isLikelyNonImageMediaUrl(preview)).toBe(true);
    expect(
      thumbnailMediaSources({
        ...gifvAttachment,
        preview_url: preview,
      }),
    ).toEqual([]);
  });

  it("accepts a distinct static poster URL", () => {
    const poster = "https://media.example/preview.jpg?token=signed#poster";
    expect(isLikelyNonImageMediaUrl(poster)).toBe(false);
    expect(
      thumbnailMediaSources({
        ...gifvAttachment,
        preview_url: poster,
      }),
    ).toEqual([poster]);
  });

  it("keeps audio originals out of image thumbnail sources", () => {
    const audio: MediaAttachment = {
      id: "audio",
      type: "audio",
      url: "https://media.example/original/audio.mp3",
      remote_url: "https://remote.example/original/audio.mp3",
      preview_url: "https://media.example/small/audio-waveform.png",
    };

    expect(isAudioMedia(audio)).toBe(true);
    expect(thumbnailMediaSources(audio, "Local")).toEqual([
      "https://media.example/small/audio-waveform.png",
    ]);
    expect(thumbnailMediaSources(audio, "Remote")).toEqual([
      "https://media.example/small/audio-waveform.png",
    ]);
  });

  it("keeps explicit unknown and non-image originals out of image thumbnails", () => {
    for (const mediaType of ["unknown", "application/pdf"]) {
      const media: MediaAttachment = {
        id: mediaType,
        type: mediaType,
        url: "https://media.example/original/file.bin",
        remote_url: "https://remote.example/original/file.bin",
        preview_url: "https://media.example/preview/file.jpg",
      };
      expect(mediaKind(media)).toBe("unknown");
      expect(thumbnailMediaSources(media)).toEqual([
        "https://media.example/preview/file.jpg",
      ]);
      expect(
        thumbnailMediaSources({ ...media, preview_url: media.url }),
      ).toEqual([]);
    }
  });

  it("keeps legacy attachments without a declared type on the image path", () => {
    const media: MediaAttachment = {
      id: "legacy",
      url: "https://media.example/original/legacy.jpg",
      preview_url: "https://media.example/preview/legacy.jpg",
    };
    expect(mediaKind(media)).toBe("image");
    expect(thumbnailMediaSources(media)).toEqual([
      "https://media.example/preview/legacy.jpg",
      "https://media.example/original/legacy.jpg",
    ]);
  });

  it("keeps ordinary image attachments on the image preview path", () => {
    expect(
      isVideoMedia({
        id: "image",
        type: "image",
        url: "https://media.example/original/image.png",
        preview_url: "https://media.example/small/image.png",
      }),
    ).toBe(false);
  });

  it("preserves animated GIF sources instead of restricting them to a preview", () => {
    const media: MediaAttachment = {
      id: "animated-gif",
      type: "image",
      media_type: "image",
      url: "https://media.example/original/animated.gif?token=signed",
      remote_url: "https://remote.example/original/animated.gif",
      preview_url: "https://media.example/small/static.jpg",
    };
    expect(isAnimatedImageMedia(media)).toBe(true);
    expect(thumbnailMediaSources(media)).toEqual([
      "https://media.example/original/animated.gif?token=signed",
      "https://remote.example/original/animated.gif",
      "https://media.example/small/static.jpg",
    ]);
  });

  it("preserves animated PNG originals when the MIME is available", () => {
    const media: MediaAttachment = {
      id: "animated-png-mime",
      type: "image",
      media_type: "image/apng",
      url: "https://media.example/original/animated.png",
      preview_url: "https://media.example/small/static.png",
    };
    expect(isAnimatedImageMedia(media)).toBe(true);
    expect(thumbnailMediaSources(media)).toEqual([
      "https://media.example/original/animated.png",
      "https://media.example/small/static.png",
    ]);
  });

  it("detects an animated PNG from its .apng original URL", () => {
    const media: MediaAttachment = {
      id: "animated-png-extension",
      type: "image",
      media_type: "image",
      url: "https://media.example/original/animated.apng?token=signed",
      preview_url: "https://media.example/small/static.png",
    };
    expect(isAnimatedImageMedia(media)).toBe(true);
    expect(thumbnailMediaSources(media)).toEqual([
      "https://media.example/original/animated.apng?token=signed",
      "https://media.example/small/static.png",
    ]);
  });

  it("preserves animated PNG sources with remote preference", () => {
    expect(
      thumbnailMediaSources(
        {
          id: "animated-png",
          type: "image",
          media_type: "image/apng",
          url: "https://media.example/original/animated.png",
          remote_url: "https://remote.example/original/animated.png",
          preview_url: "https://media.example/small/animated.png",
        },
        "Remote",
      ),
    ).toEqual([
      "https://remote.example/original/animated.png",
      "https://media.example/original/animated.png",
      "https://media.example/small/animated.png",
    ]);
  });
});
