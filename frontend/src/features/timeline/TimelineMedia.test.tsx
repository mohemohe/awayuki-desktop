import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invokeTypedCommand } from "../../api/tauri";
import type { MediaAttachment } from "../../types/app";
import { MediaThumbnail } from "./TimelineMedia";

vi.mock("../../api/tauri", () => ({ invokeTypedCommand: vi.fn() }));

const noop = vi.fn();
const probe = vi.mocked(invokeTypedCommand);

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

function proxiedVideo(id: string): MediaAttachment {
  return {
    id,
    type: "video",
    url: `https://server.example/media_proxy/${id}/original`,
    preview_url: `https://server.example/media_proxy/${id}/small`,
    remote_url: `https://remote.example/${id}.m4v`,
  };
}

describe("MediaThumbnail", () => {
  beforeEach(() => {
    probe.mockReset();
  });

  it("never gives an opaque poster that resolves to video to an img", async () => {
    probe.mockResolvedValue(false);
    const { container } = renderThumbnail(proxiedVideo("resolves-to-video"));

    expect(container.querySelector("img")).toBeNull();
    expect(screen.queryByText("Media unavailable")).toBeNull();
    expect(await screen.findByText("Media unavailable")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByTestId("video-thumbnail-play-icon")).toBeInTheDocument();
  });

  it("shows an opaque poster once it is confirmed to be an image", async () => {
    probe.mockResolvedValue(true);
    const media = proxiedVideo("resolves-to-image");
    const { container } = renderThumbnail(media);

    await waitFor(() =>
      expect(container.querySelector("img")).toHaveAttribute(
        "src",
        media.preview_url,
      ),
    );
    expect(probe).toHaveBeenCalledWith("probe_media_image", {
      request: { url: media.preview_url },
    });
  });

  it("reuses a poster verdict across remounts", async () => {
    probe.mockResolvedValue(true);
    const media = proxiedVideo("remounted");
    const first = renderThumbnail(media);
    await waitFor(() => expect(first.container.querySelector("img")).not.toBeNull());
    first.unmount();

    const second = renderThumbnail(media);

    expect(second.container.querySelector("img")).not.toBeNull();
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it("does not cache a failed probe and keeps the img out", async () => {
    probe.mockRejectedValueOnce(new Error("offline")).mockResolvedValue(true);
    const media = proxiedVideo("transient-failure");
    const first = renderThumbnail(media);
    expect(await screen.findByText("Media unavailable")).toBeInTheDocument();
    expect(first.container.querySelector("img")).toBeNull();
    first.unmount();

    const second = renderThumbnail(media);

    await waitFor(() => expect(second.container.querySelector("img")).not.toBeNull());
    expect(probe).toHaveBeenCalledTimes(2);
  });

  it("does not probe a hidden sensitive video", () => {
    const media = proxiedVideo("sensitive");
    render(
      <MediaThumbnail
        media={media}
        sources={[media.preview_url ?? ""]}
        sensitive
        visible={false}
        onToggle={noop}
        onOpen={noop}
      />,
    );

    expect(probe).not.toHaveBeenCalled();
  });

  it("does not probe an opaque image thumbnail", () => {
    const { container } = renderThumbnail({
      id: "opaque-image",
      type: "image",
      url: "https://server.example/media_proxy/opaque-image/original",
      preview_url: "https://server.example/media_proxy/opaque-image/small",
    });

    expect(probe).not.toHaveBeenCalled();
    expect(container.querySelector("img")).not.toBeNull();
  });

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

  it("decodes an original image thumbnail synchronously", () => {
    const { container } = renderThumbnail({
      id: "original-fallback",
      type: "image",
      url: "https://media.example/original/image.jpg",
      remote_url: "https://remote.example/original/image.jpg",
      preview_url: null,
    });

    expect(container.querySelector("img")).toHaveAttribute("decoding", "sync");
  });

  it("also decodes distinct previews synchronously when their actual format is unknown", () => {
    const { container } = renderThumbnail({
      id: "preview",
      type: "image",
      url: "https://media.example/original/image.jpg",
      remote_url: "https://remote.example/original/image.jpg",
      preview_url: "https://media.example/preview/opaque-id",
    });

    expect(container.querySelector("img")).toHaveAttribute("decoding", "sync");
  });
});
