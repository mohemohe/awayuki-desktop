import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { MediaAttachment } from "../../types/app";
import { MediaThumbnail } from "./TimelineMedia";

const noop = vi.fn();

function renderThumbnail(media: MediaAttachment) {
  return render(
    <MediaThumbnail
      media={media}
      sources={[media.preview_url ?? media.url ?? ""]}
      sensitive={false}
      visible
      onToggle={noop}
      onOpen={noop}
    />,
  );
}

describe("MediaThumbnail", () => {
  it("centers a play icon over video thumbnails", () => {
    renderThumbnail({
      id: "video",
      type: "video",
      url: "https://media.example/original/video.mp4",
      preview_url: "https://media.example/small/video.jpg",
    });

    expect(screen.getByTestId("video-thumbnail-play-icon")).toHaveClass(
      "left-1/2",
      "top-1/2",
      "-translate-x-1/2",
      "-translate-y-1/2",
    );
  });

  it("keeps a playable placeholder when a video has no safe static preview", () => {
    render(
      <MediaThumbnail
        media={{
          id: "video-without-preview",
          type: "video",
          url: "https://media.example/original/video.mp4",
          preview_url: "https://media.example/original/video.mp4",
        }}
        sources={[]}
        sensitive={false}
        visible
        onToggle={noop}
        onOpen={noop}
      />,
    );

    expect(screen.getByText("Media unavailable")).toBeInTheDocument();
    expect(screen.getByTestId("video-thumbnail-play-icon")).toBeInTheDocument();
    expect(screen.getByTitle("Open media preview")).toBeInTheDocument();
  });

  it("does not show the video play icon for audio thumbnails", () => {
    renderThumbnail({
      id: "audio",
      type: "audio",
      url: "https://media.example/original/audio.mp3",
      preview_url: "https://media.example/small/audio-waveform.png",
    });

    expect(screen.queryByTestId("video-thumbnail-play-icon")).toBeNull();
  });

  it("keeps an audio placeholder without adding a video play icon", () => {
    render(
      <MediaThumbnail
        media={{
          id: "audio-without-preview",
          type: "audio",
          url: "https://media.example/original/audio.mp3",
        }}
        sources={[]}
        sensitive={false}
        visible
        onToggle={noop}
        onOpen={noop}
      />,
    );

    expect(screen.getByText("Media unavailable")).toBeInTheDocument();
    expect(screen.queryByTestId("video-thumbnail-play-icon")).toBeNull();
    expect(screen.getByTitle("Open media preview")).toBeInTheDocument();
  });

  it.each(["image/gif", "image/apng"])(
    "does not mark animated %s images as video",
    (mediaType) => {
      renderThumbnail({
        id: mediaType,
        type: "image",
        media_type: mediaType,
        url: `https://media.example/original/${mediaType}`,
      });

      expect(screen.queryByTestId("video-thumbnail-play-icon")).toBeNull();
    },
  );
});
