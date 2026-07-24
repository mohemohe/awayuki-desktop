import { describe, expect, it } from "vitest";
import { moveQueueItem } from "./mediaQueue";

describe("compose media queue", () => {
  it("moves an item without mutating the draft", () => {
    const draft = ["a", "b", "c"];
    expect(moveQueueItem(draft, 2, 0)).toEqual(["c", "a", "b"]);
    expect(draft).toEqual(["a", "b", "c"]);
  });

  it("preserves identity for invalid moves", () => {
    const draft = ["a"];
    expect(moveQueueItem(draft, 0, 2)).toBe(draft);
  });
});
