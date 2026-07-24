import { describe, expect, it } from "vitest";
import type { MediaAttachment } from "../types/app";
import { isVideoMedia, previewMediaSources } from "./media";

const gifvAttachment: MediaAttachment = {
  id: "116940484718637238",
  type: "gifv",
  url: "https://media.example/original/video.mp4",
  preview_url: "https://media.example/small/preview.gif",
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
});
