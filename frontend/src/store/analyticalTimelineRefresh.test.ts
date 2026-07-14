import { describe, expect, it, vi } from "vitest";

import type { ColumnSummary } from "../types/app";
import { AnalyticalTimelineRefreshCoordinator } from "./analyticalTimelineRefresh";

describe("AnalyticalTimelineRefreshCoordinator", () => {
  it("coalesces invalidations and refreshes heavy columns sequentially", async () => {
    const releases: Array<() => void> = [];
    const calls: string[] = [];
    let running = 0;
    let maxRunning = 0;
    const coordinator = new AnalyticalTimelineRefreshCoordinator({
      canAutoRefresh: () => true,
      refresh: async (column) => {
        calls.push(column.id);
        running += 1;
        maxRunning = Math.max(maxRunning, running);
        await new Promise<void>((resolve) => releases.push(resolve));
        running -= 1;
      },
    });
    const custom = column("custom", "custom", 0);
    const yq = column("yq", "yq", 1);

    coordinator.invalidate(custom);
    coordinator.invalidate(custom);
    coordinator.invalidate(yq);
    const flush = coordinator.flushForTest();

    await vi.waitFor(() => expect(calls).toEqual(["custom"]));
    releases.shift()?.();
    await vi.waitFor(() => expect(calls).toEqual(["custom", "yq"]));
    releases.shift()?.();
    await flush;

    expect(maxRunning).toBe(1);
    expect(coordinator.isDirty(custom.id)).toBe(false);
    expect(coordinator.isDirty(yq.id)).toBe(false);
    coordinator.reset();
  });

  it("keeps hidden columns dirty until they are activated", async () => {
    const refresh = vi.fn(async () => undefined);
    const hidden = column("hidden", "custom", 1);
    const coordinator = new AnalyticalTimelineRefreshCoordinator({
      canAutoRefresh: () => false,
      refresh,
    });

    coordinator.invalidate(hidden);
    await Promise.resolve();
    expect(refresh).not.toHaveBeenCalled();
    expect(coordinator.isDirty(hidden.id)).toBe(true);

    coordinator.activate(hidden);
    await coordinator.flushForTest();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(coordinator.isDirty(hidden.id)).toBe(false);
    coordinator.reset();
  });

  it("retains an invalidation that arrives while a query is running", async () => {
    let release: (() => void) | undefined;
    const visible = column("visible", "yq", 0);
    const coordinator = new AnalyticalTimelineRefreshCoordinator({
      canAutoRefresh: () => true,
      refresh: () => new Promise<void>((resolve) => (release = resolve)),
    });

    coordinator.invalidate(visible);
    const flush = coordinator.flushForTest();
    await vi.waitFor(() => expect(release).toBeDefined());
    coordinator.invalidate(visible);
    release?.();
    await flush;

    expect(coordinator.isDirty(visible.id)).toBe(true);
    coordinator.reset();
  });

  it("does not retry a failed query forever without a new commit or activation", async () => {
    vi.useFakeTimers();
    const refresh = vi.fn(async () => {
      throw new Error("invalid analytical query");
    });
    const visible = column("invalid", "custom", 0);
    const coordinator = new AnalyticalTimelineRefreshCoordinator({
      canAutoRefresh: () => true,
      refresh,
      initialDelayMs: 10,
      cooldownMs: 20,
    });

    coordinator.invalidate(visible);
    await vi.advanceTimersByTimeAsync(10);
    expect(refresh).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(200);
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(coordinator.isDirty(visible.id)).toBe(true);

    coordinator.invalidate(visible);
    await vi.advanceTimersByTimeAsync(20);
    expect(refresh).toHaveBeenCalledTimes(2);
    coordinator.reset();
    vi.useRealTimers();
  });
});

function column(id: string, columnType: string, position: number): ColumnSummary {
  return {
    id,
    columnType,
    name: id,
    maxStatuses: 100,
    paneIndex: 0,
    position,
  };
}
