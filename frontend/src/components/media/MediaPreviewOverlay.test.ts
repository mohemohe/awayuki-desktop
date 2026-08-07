import { describe, expect, it, vi } from "vitest";

import { releaseVideoElement } from "./MediaPreviewOverlay";

describe("media preview video lifecycle", () => {
  it("stops playback and clears buffered WebKit media state", () => {
    const video = {
      pause: vi.fn(),
      removeAttribute: vi.fn(),
      load: vi.fn(),
    } as unknown as HTMLVideoElement;

    releaseVideoElement(video);

    expect(video.pause).toHaveBeenCalledOnce();
    expect(video.removeAttribute).toHaveBeenCalledWith("src");
    expect(video.load).toHaveBeenCalledOnce();
  });
});
