import type { TimelineGap, TimelineStatus } from "../types/app";

export type TimelineDisplayItem =
  | { kind: "status"; status: TimelineStatus }
  | { kind: "gap"; gap: TimelineGap };

export function timelineGapKey(gap: TimelineGap) {
  return JSON.stringify([gap.timelineType, gap.sourceAcct]);
}

export function timelineGapResourceKey(columnId: string, gap: TimelineGap) {
  return `${columnId}:${timelineGapKey(gap)}`;
}

/** Keep posts at the exact API boundary above the manual gap control. */
export function timelineDisplayItems(
  statuses: readonly TimelineStatus[],
  gaps: readonly TimelineGap[],
): TimelineDisplayItem[] {
  return [
    ...statuses.map((status, order) => ({
      kind: "status" as const,
      status,
      timestamp: Date.parse(status.createdAt),
      order,
    })),
    ...gaps.map((gap, order) => ({
      kind: "gap" as const,
      gap,
      timestamp: Date.parse(gap.boundaryPosition),
      order,
    })),
  ]
    .sort((left, right) => {
      const leftTimestamp = Number.isFinite(left.timestamp)
        ? left.timestamp
        : Number.NEGATIVE_INFINITY;
      const rightTimestamp = Number.isFinite(right.timestamp)
        ? right.timestamp
        : Number.NEGATIVE_INFINITY;
      if (leftTimestamp !== rightTimestamp) return rightTimestamp - leftTimestamp;
      if (left.kind !== right.kind) return left.kind === "status" ? -1 : 1;
      return left.order - right.order;
    })
    .map((item) =>
      item.kind === "status"
        ? { kind: item.kind, status: item.status }
        : { kind: item.kind, gap: item.gap },
    );
}
