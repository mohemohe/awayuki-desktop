import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import type {
  AppSnapshot,
  MediaPreviewState,
  TimelineStatus,
} from "../../types/app";

const browserApi = vi.hoisted(() => ({ openExternalUrl: vi.fn() }));
vi.mock("../../utils/browser", () => browserApi);

import { MediaPreviewOverlay } from "./MediaPreviewOverlay";

beforeEach(() => {
  browserApi.openExternalUrl.mockReset();
  browserApi.openExternalUrl.mockResolvedValue(undefined);
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(
    () => undefined,
  );
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(
    () => undefined,
  );
  useAppStore.setState({
    snapshot: {
      settings: { confirmation: { media_source: "Local" } },
    } as AppSnapshot,
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("MediaPreviewOverlay audio", () => {
  it("renders audio originals with the media element, never img", () => {
    const preview = {
      src: "https://media.example/audio-preview.mp3",
      media: {
        id: "audio",
        type: "audio",
        url: "https://media.example/original.mp3",
        preview_url: "https://media.example/audio-preview.mp3",
        description: "Voice message",
      },
      status: {} as TimelineStatus,
    } satisfies MediaPreviewState;

    const { unmount } = render(<MediaPreviewOverlay preview={preview} />);

    const audio = screen.getByLabelText("Voice message");
    expect(audio).toBeInstanceOf(HTMLAudioElement);
    expect(audio).toHaveAttribute("src", "https://media.example/original.mp3");
    expect(audio).toHaveAttribute("controls");
    expect(audio).toHaveAttribute("preload", "metadata");
    expect(document.querySelector("img")).toBeNull();

    unmount();
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalled();
    expect(HTMLMediaElement.prototype.load).toHaveBeenCalled();
  });

  it("never sends a failed video original through the Image retry probe", async () => {
    vi.useFakeTimers();
    const imageConstructor = vi.fn(
      class {
        src = "";
      },
    );
    vi.stubGlobal("Image", imageConstructor);
    const preview = {
      src: "https://media.example/original.mp4",
      media: {
        id: "video",
        type: "video",
        url: "https://media.example/original.mp4",
        description: "Screen recording",
      },
      status: {} as TimelineStatus,
    } satisfies MediaPreviewState;

    const { unmount } = render(<MediaPreviewOverlay preview={preview} />);
    const video = screen.getByLabelText("Screen recording");
    expect(screen.getByText("Loading media")).toBeInTheDocument();
    fireEvent.error(video);
    await vi.runAllTimersAsync();

    expect(imageConstructor).not.toHaveBeenCalled();
    expect(document.querySelector("img")).toBeNull();
    unmount();
  });

  it("does not render or probe an explicit unknown original as an image", async () => {
    vi.useFakeTimers();
    const imageConstructor = vi.fn(
      class {
        src = "";
      },
    );
    vi.stubGlobal("Image", imageConstructor);
    const preview = {
      src: "https://media.example/original.bin",
      media: {
        id: "unknown",
        type: "unknown",
        url: "https://media.example/original.bin",
      },
      status: {} as TimelineStatus,
    } satisfies MediaPreviewState;

    const { unmount } = render(<MediaPreviewOverlay preview={preview} />);
    await vi.runAllTimersAsync();

    expect(screen.getByText("Media failed to load")).toBeInTheDocument();
    expect(document.querySelector("img,video,audio")).toBeNull();
    expect(imageConstructor).not.toHaveBeenCalled();
    unmount();
  });

  it("uses a safe unknown preview for display but opens the original", () => {
    const preview = {
      src: "https://media.example/original.bin",
      media: {
        id: "unknown-with-preview",
        type: "unknown",
        url: "https://media.example/original.bin",
        preview_url: "https://media.example/static-preview.jpg",
        description: "Unknown attachment",
      },
      status: {} as TimelineStatus,
    } satisfies MediaPreviewState;

    const { unmount } = render(<MediaPreviewOverlay preview={preview} />);
    expect(
      screen.getByRole("img", { name: "Unknown attachment" }),
    ).toHaveAttribute("src", "https://media.example/static-preview.jpg");

    fireEvent.click(screen.getByTitle("Open in browser"));
    expect(browserApi.openExternalUrl).toHaveBeenCalledWith(
      "https://media.example/original.bin",
    );
    unmount();
  });
});
