import { describe, expect, it } from "vitest";
import {
  statusHoverBackgroundClass,
  statusVisibilityBackgroundClass,
} from "./TimelineStatusHelpers";

describe("statusVisibilityBackgroundClass", () => {
  it.each([
    ["public", ""],
    ["unlisted", "status-visibility-unlisted"],
    ["private", "status-visibility-private"],
    ["direct", "status-visibility-private"],
  ])("maps %s to the expected background", (visibility, expectedClass) => {
    expect(statusVisibilityBackgroundClass(true, visibility)).toBe(
      expectedClass,
    );
  });

  it("keeps the current background when the setting is disabled", () => {
    expect(statusVisibilityBackgroundClass(false, "unlisted")).toBe("");
    expect(statusVisibilityBackgroundClass(false, "private")).toBe("");
    expect(statusVisibilityBackgroundClass(false, "direct")).toBe("");
  });

  it("does not override accessibility visibility colors on hover", () => {
    expect(statusHoverBackgroundClass("status-visibility-unlisted")).toBe("");
    expect(statusHoverBackgroundClass("status-visibility-private")).toBe("");
    expect(statusHoverBackgroundClass("")).toBe("hover:bg-surface0/40");
  });
});
