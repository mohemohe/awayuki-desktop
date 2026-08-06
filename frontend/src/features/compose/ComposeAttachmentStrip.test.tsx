import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { ComposeMediaAttachment } from "../../types/app";
import { ComposeAttachmentStrip } from "./ComposeAttachmentStrip";

const attachments: ComposeMediaAttachment[] = [
  {
    id: "first",
    filename: "first.png",
    previewSrc: "data:image/png;base64,first",
    uploading: false,
    media_type: "image",
  },
  {
    id: "second",
    filename: "second.png",
    previewSrc: "data:image/png;base64,second",
    uploading: false,
    media_type: "image",
  },
];

describe("ComposeAttachmentStrip", () => {
  it("reorders media by dragging across the attachment cards", () => {
    const onMove = vi.fn();
    render(
      <ComposeAttachmentStrip
        attachments={attachments}
        onMove={onMove}
        onRemove={() => undefined}
      />,
    );
    const firstHandle = screen.getByRole("button", {
      name: /Reorder media.*first\.png/i,
    });
    const secondHandle = screen.getByRole("button", {
      name: /Reorder media.*second\.png/i,
    });
    const firstCard = firstHandle.parentElement!;
    const secondCard = secondHandle.parentElement!;
    vi.spyOn(firstCard, "getBoundingClientRect").mockReturnValue({
      left: 0,
      width: 80,
    } as DOMRect);
    vi.spyOn(secondCard, "getBoundingClientRect").mockReturnValue({
      left: 84,
      width: 80,
    } as DOMRect);

    expect(firstHandle).toHaveClass("inset-0");
    fireEvent.mouseDown(firstHandle, { button: 0 });
    fireEvent.mouseMove(firstCard.parentElement!, { clientX: 124 });
    fireEvent.mouseUp(firstCard.parentElement!);

    expect(onMove).toHaveBeenCalledWith(0, 1);
  });

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
