import { describe, expect, it } from "vitest";
import { createMockFixture } from "../../api/mock";
import type { TimelineStatus } from "../../types/app";
import { initialComposeSlice, reduceComposeSlice } from "./compose";

describe("compose slice reducer", () => {
  it("sets a target without mutating unrelated visibility", async () => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const initial = { ...initialComposeSlice(), visibility: "private" as const };
    const next = reduceComposeSlice(initial, {
      type: "setTarget",
      target: { kind: "quote", status: statuses[0]! },
    });
    expect(next.composeTarget?.kind).toBe("quote");
    expect(next.visibility).toBe("private");
    expect(initial.composeTarget).toBeNull();
  });

  it("resets to a clean draft", () => {
    const fixture = createMockFixture();
    expect(fixture.activeAcct).toBeTruthy();
    expect(
      reduceComposeSlice(
        { composeText: "draft", composeTarget: null, visibility: "direct" },
        { type: "reset" },
      ),
    ).toEqual(initialComposeSlice());
  });
});
