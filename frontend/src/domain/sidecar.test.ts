import { describe, expect, it, vi } from "vitest";
import {
  SIDECAR_DEFAULT_WIDTH,
  SIDECAR_MIN_WIDTH,
  SidecarLifecycleManager,
  SidecarStyleRetryScheduler,
  isSupportedSidecarUrl,
  normalizeSidecarSettings,
  normalizeSidecarWidth,
} from "./sidecar";

describe("sidecar URL policy", () => {
  it.each([
    "https://example.com",
    "http://localhost:3000/path",
    " HTTPS://EXAMPLE.COM/path ",
  ])("allows http/https URLs with a host: %s", (url) => {
    expect(isSupportedSidecarUrl(url)).toBe(true);
  });

  it.each([
    "javascript:alert(1)",
    "file:///tmp/private",
    "data:text/html,hello",
    "https://",
    "not a URL",
  ])("rejects URLs outside policy: %s", (url) => {
    expect(isSupportedSidecarUrl(url)).toBe(false);
  });
});

describe("SidecarStyleRetryScheduler", () => {
  it("backs off per sidecar and stops after success", () => {
    const scheduled: Array<{ callback: () => void; delay: number }> = [];
    const cancelled: unknown[] = [];
    const scheduler = new SidecarStyleRetryScheduler(
      (callback, delay) => {
        scheduled.push({ callback, delay });
        return scheduled.length as unknown as ReturnType<typeof setTimeout>;
      },
      (timer) => cancelled.push(timer),
    );
    const retry = vi.fn();

    scheduler.retry("news", retry);
    scheduler.retry("news", retry);
    expect(scheduled.map((item) => item.delay)).toEqual([250]);
    scheduled[0]?.callback();
    expect(retry).toHaveBeenCalledOnce();
    scheduler.retry("news", retry);
    expect(scheduled.map((item) => item.delay)).toEqual([250, 500]);

    scheduler.succeed("news");
    expect(cancelled).toEqual([2]);
    expect(scheduler.delayFor("news")).toBe(250);
  });

  it("cancels timers for every sidecar during cleanup", () => {
    const cancelled: unknown[] = [];
    let timer = 0;
    const scheduler = new SidecarStyleRetryScheduler(
      () => (++timer) as unknown as ReturnType<typeof setTimeout>,
      (value) => cancelled.push(value),
    );
    scheduler.retry("first", () => undefined);
    scheduler.retry("second", () => undefined);
    scheduler.cancelAll();
    expect(cancelled).toEqual([1, 2]);
  });
});

describe("sidecar normalization", () => {
  it("normalizes widths and drops entries outside the URL policy", () => {
    expect(normalizeSidecarWidth(1)).toBe(SIDECAR_MIN_WIDTH);
    expect(normalizeSidecarWidth("invalid")).toBe(SIDECAR_DEFAULT_WIDTH);
    expect(
      normalizeSidecarSettings({
        entries: [
          {
            id: "ok",
            name: "  News  ",
            url: " https://example.com ",
            userStyleEnabled: false,
            userStyle: "",
            width: 1,
          },
          {
            id: "blocked",
            name: "Local",
            url: "file:///tmp/private",
            userStyleEnabled: false,
            userStyle: "",
            width: 500,
          },
        ],
        mainViewIndex: 9,
      }),
    ).toEqual({
      entries: [
        {
          id: "ok",
          name: "News",
          url: "https://example.com",
          userStyleEnabled: false,
          userStyle: "",
          width: SIDECAR_MIN_WIDTH,
        },
      ],
      mainViewIndex: 0,
    });
  });
});

describe("SidecarLifecycleManager", () => {
  it("aborts older generations and rejects their completions", () => {
    const manager = new SidecarLifecycleManager();
    const create = manager.begin("news", "creating");
    const navigate = manager.begin("news", "navigating");

    expect(create.signal.aborted).toBe(true);
    expect(manager.isCurrent(create)).toBe(false);
    expect(manager.transition(create, "ready")).toBe(false);
    expect(manager.isCurrent(navigate)).toBe(true);
    expect(manager.transition(navigate, "visible")).toBe(true);
    expect(manager.status("news")).toBe("visible");
  });

  it("cancels every in-flight operation during component cleanup", () => {
    const manager = new SidecarLifecycleManager();
    const first = manager.begin("first", "ready");
    const second = manager.begin("second", "creating");

    manager.cancelAll();

    expect(first.signal.aborted).toBe(true);
    expect(second.signal.aborted).toBe(true);
    expect(manager.ids()).toEqual([]);
  });
});
