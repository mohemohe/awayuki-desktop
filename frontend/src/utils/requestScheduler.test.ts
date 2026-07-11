import { describe, expect, it, vi } from "vitest";
import {
  RequestCancelledError,
  RequestScheduler,
} from "./requestScheduler";

describe("RequestScheduler", () => {
  it("bounds each lane and gives visible work priority", async () => {
    const scheduler = createScheduler({ timeline: 1 });
    const releases: Array<() => void> = [];
    const order: string[] = [];
    const task = (name: string) => async () => {
      order.push(name);
      await new Promise<void>((resolve) => releases.push(resolve));
      return name;
    };

    const first = scheduler.schedule(
      { key: "timeline:first", lane: "timeline" },
      task("first"),
    );
    const background = scheduler.schedule(
      { key: "timeline:background", lane: "timeline", priority: 0 },
      task("background"),
    );
    const visible = scheduler.schedule(
      { key: "timeline:visible", lane: "timeline", priority: 100 },
      task("visible"),
    );

    await vi.waitFor(() => expect(order).toEqual(["first"]));
    expect(scheduler.metrics().timeline.maxRunning).toBe(1);
    releases.shift()?.();
    await vi.waitFor(() => expect(order).toEqual(["first", "visible"]));
    releases.shift()?.();
    await vi.waitFor(() =>
      expect(order).toEqual(["first", "visible", "background"]),
    );
    releases.shift()?.();

    await expect(Promise.all([first, background, visible])).resolves.toEqual([
      "first",
      "background",
      "visible",
    ]);
  });

  it("cancels an old query generation and discards its late completion", async () => {
    const scheduler = createScheduler();
    let releaseOld: ((value: string) => void) | undefined;
    const old = scheduler.schedule(
      { key: "autocomplete:compose", lane: "autocomplete" },
      () => new Promise<string>((resolve) => (releaseOld = resolve)),
    );
    const latest = scheduler.schedule(
      { key: "autocomplete:compose", lane: "autocomplete" },
      async () => "latest",
    );
    releaseOld?.("stale");

    await expect(old).rejects.toBeInstanceOf(RequestCancelledError);
    await expect(latest).resolves.toBe("latest");
  });

  it("cancels pane and account scopes independently", async () => {
    const scheduler = createScheduler();
    const never = () => new Promise<string>(() => undefined);
    const paneA = scheduler.schedule(
      { key: "profile:pane-a:posts", lane: "profile" },
      never,
    );
    const paneB = scheduler.schedule(
      { key: "profile:pane-b:posts", lane: "profile" },
      never,
    );

    scheduler.cancelPrefix("profile:pane-a:");
    await expect(paneA).rejects.toBeInstanceOf(RequestCancelledError);
    expect(scheduler.metrics().profile.running).toBe(1);

    scheduler.cancelAll();
    await expect(paneB).rejects.toBeInstanceOf(RequestCancelledError);
  });

  it("propagates cancellation through AbortSignal", async () => {
    const scheduler = createScheduler();
    let observedSignal: AbortSignal | undefined;
    const request = scheduler.schedule(
      { key: "timeline:abort", lane: "timeline" },
      async ({ signal }) => {
        observedSignal = signal;
        await new Promise<void>(() => undefined);
        return "unreachable";
      },
    );
    await vi.waitFor(() => expect(observedSignal).toBeDefined());
    scheduler.cancel("timeline:abort");

    expect(observedSignal?.aborted).toBe(true);
    await expect(request).rejects.toBeInstanceOf(RequestCancelledError);
  });
});

function createScheduler(overrides: Partial<Record<"timeline" | "profile" | "autocomplete", number>> = {}) {
  return new RequestScheduler({
    timeline: overrides.timeline ?? 2,
    profile: overrides.profile ?? 2,
    autocomplete: overrides.autocomplete ?? 1,
  });
}
