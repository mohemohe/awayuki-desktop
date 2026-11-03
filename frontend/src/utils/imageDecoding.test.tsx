import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Avatar } from "../components/common/Avatar";
import { CustomEmojiText } from "../components/common/CustomEmoji";
import { MediaThumbnail } from "../features/timeline/TimelineMedia";
import { ComposeAttachmentPreview } from "../features/compose/ComposeAttachmentPreview";

describe("app image decoding", () => {
  it("selects synchronous decoding before React assigns animated or opaque image sources", () => {
    const assignments: Array<{ source: string; decoding: string | null }> = [];
    const setAttribute = Element.prototype.setAttribute;
    vi.spyOn(Element.prototype, "setAttribute").mockImplementation(function (
      this: Element, name: string, value: string,
    ) {
      if (this instanceof HTMLImageElement && name === "src") {
        assignments.push({ source: value, decoding: this.getAttribute("decoding") });
      }
      return setAttribute.call(this, name, value);
    });
    const urls = [
      "https://media.example/avatar.gif",
      "https://media.example/emoji.webp",
      "https://media.example/opaque-preview",
      "blob:animated-upload",
    ];
    const { container } = render(<>
      <Avatar src={urls[0]} label="avatar" />
      <CustomEmojiText text=":party:" emojis={[{
        shortcode: "party", url: urls[1], staticUrl: "https://media.example/static.png",
      }]} />
      <MediaThumbnail media={{ id: "image", type: "image", url: urls[2] }}
        sources={[urls[2]]} sensitive={false} visible
        onToggle={() => undefined} onOpen={() => undefined} />
      <ComposeAttachmentPreview attachment={{
        id: "upload", filename: "animation.webp", previewSrc: urls[3],
      }} />
    </>);

    expect(assignments).toEqual(urls.map((source) => ({ source, decoding: "sync" })));
    expect(Array.from(container.querySelectorAll("img"), (image) => image.getAttribute("src")))
      .toEqual(urls);
  });
});
