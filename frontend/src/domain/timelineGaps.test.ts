import { describe, expect, it } from "vitest";
import type { TimelineGap, TimelineStatus } from "../types/app";
import { timelineDisplayItems } from "./timelineGaps";

describe("timelineDisplayItems", () => {
  it("places a gap after every status at the exact API boundary", () => {
    const statuses = [
      { id: "new", createdAt: "2026-01-01T03:00:00.000Z" },
      { id: "same-a", createdAt: "2026-01-01T02:00:00.000Z" },
      { id: "same-b", createdAt: "2026-01-01T02:00:00.000Z" },
      { id: "old", createdAt: "2026-01-01T01:00:00.000Z" },
    ] as TimelineStatus[];
    const gap = {
      timelineType: "home",
      sourceAcct: "alice@example.test",
      boundaryStatusId: "same-b",
      boundaryServerDomain: "example.test",
      boundaryPosition: "2026-01-01T02:00:00.000Z",
      nextMaxStatusId: "same-b",
    } satisfies TimelineGap;

    expect(
      timelineDisplayItems(statuses, [gap]).map((item) =>
        item.kind === "status" ? item.status.id : "gap",
      ),
    ).toEqual(["new", "same-a", "same-b", "gap", "old"]);
  });
});
