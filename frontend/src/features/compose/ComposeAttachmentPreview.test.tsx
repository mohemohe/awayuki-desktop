import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComposeMediaAttachment } from "../../types/app";
import {
  ComposeAttachmentPreview,
  composeVideoPoster,
} from "./ComposeAttachmentPreview";

let pauseSpy: ReturnType<typeof vi.spyOn>;
let loadSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  pauseSpy = vi
    .spyOn(HTMLMediaElement.prototype, "pause")
    .mockImplementation(() => undefined);
  loadSpy = vi
    .spyOn(HTMLMediaElement.prototype, "load")
    .mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

function attachment(
  overrides: Partial<ComposeMediaAttachment>,
): ComposeMediaAttachment {
  return {
    id: "media",
    filename: "attachment",
    previewSrc: "",
    ...overrides,
  };
}

describe("ComposeAttachmentPreview", () => {
  it("renders local video blobs with bounded video metadata loading, never img", () => {
    const { container, unmount } = render(
      <ComposeAttachmentPreview
        attachment={attachment({
          filename: "recording.mp4",
          media_type: "video",
          previewSrc: "blob:recording",
        })}
      />,
    );

    const video = screen.getByLabelText("recording.mp4");
    expect(video).toBeInstanceOf(HTMLVideoElement);
    expect(video).toHaveAttribute("src", "blob:recording");
    expect(video).toHaveAttribute("preload", "metadata");
    expect(video).toHaveAttribute("playsinline");
    expect(video).toHaveProperty("muted", true);
    expect(container.querySelector("img")).toBeNull();

    unmount();
    expect(pauseSpy).toHaveBeenCalled();
    expect(loadSpy).toHaveBeenCalled();
  });

  it("never uses an original video URL as an image poster", () => {
    const uploaded = attachment({
      filename: "recording.mp4",
      media_type: "video",
      previewSrc: "https://media.example/recording.mp4",
      preview_url: "https://media.example/recording.mp4",
      url: "https://media.example/recording.mp4",
      remote_url: "https://remote.example/recording.mp4",
    });

    expect(composeVideoPoster(uploaded)).toBeUndefined();
    const { container, unmount } = render(
      <ComposeAttachmentPreview attachment={uploaded} />,
    );
    expect(screen.getByLabelText("recording.mp4")).not.toHaveAttribute(
      "poster",
    );
    expect(container.querySelector("img")).toBeNull();
    unmount();
  });

  it("rejects signed video URLs as posters while accepting a static poster", () => {
    expect(
      composeVideoPoster(
        attachment({
          media_type: "video",
          preview_url: "https://media.example/preview.mp4?token=signed#frame",
          url: "https://media.example/original.mp4",
        }),
      ),
    ).toBeUndefined();
    expect(
      composeVideoPoster(
        attachment({
          media_type: "video",
          preview_url: "https://media.example/preview.jpg?token=signed#poster",
          url: "https://media.example/original.mp4",
        }),
      ),
    ).toBe("https://media.example/preview.jpg?token=signed#poster");
  });

  it("renders audio as a non-image attachment", () => {
    const { container } = render(
      <ComposeAttachmentPreview
        attachment={attachment({
          filename: "voice.mp3",
          media_type: "audio",
          previewSrc: "https://media.example/voice.mp3",
          preview_url: "https://media.example/voice.mp3",
          url: "https://media.example/voice.mp3",
        })}
      />,
    );

    expect(screen.getByText("voice.mp3")).toBeInTheDocument();
    expect(container.querySelector("img,video")).toBeNull();
  });

  it.each(["image/gif", "image/apng"])(
    "keeps the animated %s original ahead of a static preview",
    (mediaType) => {
      const source = `https://media.example/original/${
        mediaType === "image/gif" ? "animation.gif" : "animation.png"
      }`;
      render(
        <ComposeAttachmentPreview
          attachment={attachment({
            filename: mediaType,
            media_type: mediaType,
            previewSrc: "https://media.example/preview/static.png",
            preview_url: "https://media.example/preview/static.png",
            url: source,
          })}
        />,
      );

      expect(screen.getByRole("img", { name: mediaType })).toHaveAttribute(
        "src",
        source,
      );
    },
  );
});
