import { describe, expect, it } from "vitest";
import { reduceOverlaySlice } from "./overlays";

describe("overlay slice reducer", () => {
  it("closing media does not dismiss an unrelated confirmation", () => {
    const confirmation = {
      id: "confirm-1",
      title: "Delete",
      message: "Delete?",
      confirmLabel: "Delete",
    };
    const next = reduceOverlaySlice(
      { mediaPreview: null, confirmationDialog: confirmation },
      { type: "closeMedia" },
    );
    expect(next.mediaPreview).toBeNull();
    expect(next.confirmationDialog).toBe(confirmation);
  });
});
