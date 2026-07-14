import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { ComposeMediaAttachment } from "../../types/app";
import { ComposeAttachmentStrip } from "./ComposeAttachmentStrip";

const attachments: ComposeMediaAttachment[] = [
  {
    id: "first",
    filename: "first.png",
    previewSrc: "",
    uploading: false,
    media_type: "image",
  },
  {
    id: "second",
    filename: "second.png",
    previewSrc: "",
    uploading: false,
    media_type: "image",
  },
];

describe("ComposeAttachmentStrip", () => {
  it("reorders media from the keyboard and requests a live announcement", async () => {
    const user = userEvent.setup();
    const onMove = vi.fn();
    render(
      <ComposeAttachmentStrip
        attachments={attachments}
        onMove={onMove}
        onRemove={() => undefined}
      />,
    );

    const first = screen.getByRole("button", {
      name: /Reorder media.*first\.png/i,
    });
    first.focus();
    await user.keyboard("{ArrowRight}");

    expect(onMove).toHaveBeenCalledWith(0, 1, true);
  });
});
