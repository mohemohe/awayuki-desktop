import { describe, expect, it, vi } from "vitest";
import {
  TranslationCancelledError,
  TranslationScheduler,
} from "./translationScheduler";

describe("TranslationScheduler", () => {
  it("bounds concurrency and starts visible work before background work", async () => {
    const scheduler = new TranslationScheduler(1);
    const releases: Array<() => void> = [];
    const started: string[] = [];
    const task = (name: string) => async () => {
      started.push(name);
      await new Promise<void>((resolve) => releases.push(resolve));
      return name;
    };

    const first = scheduler.schedule("first", task("first"));
    const background = scheduler.schedule("background", task("background"));
    const visible = scheduler.schedule("visible", task("visible"), 100);

    await vi.waitFor(() => expect(started).toEqual(["first"]));
    expect(scheduler.snapshot().running).toBe(1);
    releases.shift()?.();
    await vi.waitFor(() => expect(started).toEqual(["first", "visible"]));
    releases.shift()?.();
    await vi.waitFor(() =>
      expect(started).toEqual(["first", "visible", "background"]),
    );
    releases.shift()?.();

    await expect(
      Promise.all([first.promise, background.promise, visible.promise]),
    ).resolves.toEqual(["first", "background", "visible"]);
  });

  it("shares one task for the same content generation", async () => {
    const scheduler = new TranslationScheduler(3);
    const task = vi.fn(async () => "translated");
    const first = scheduler.schedule("same", task);
    const second = scheduler.schedule("same", task);

    await expect(Promise.all([first.promise, second.promise])).resolves.toEqual([
      "translated",
      "translated",
    ]);
    expect(task).toHaveBeenCalledTimes(1);
  });

  it("cancels queued work when its final consumer leaves", async () => {
    const scheduler = new TranslationScheduler(1);
    let release: (() => void) | undefined;
    const running = scheduler.schedule("running", async () => {
      await new Promise<void>((resolve) => (release = resolve));
      return "running";
    });
    const queuedTask = vi.fn(async () => "queued");
    const queued = scheduler.schedule("queued", queuedTask);

    queued.cancel();
    await expect(queued.promise).rejects.toBeInstanceOf(
      TranslationCancelledError,
    );
    expect(queuedTask).not.toHaveBeenCalled();
    release?.();
    await expect(running.promise).resolves.toBe("running");
  });

  it("keeps shared work alive until every consumer leaves", async () => {
    const scheduler = new TranslationScheduler(1);
    let release: ((value: string) => void) | undefined;
    const task = () =>
      new Promise<string>((resolve) => {
        release = resolve;
      });
    const first = scheduler.schedule("shared", task);
    const second = scheduler.schedule("shared", task);

    first.cancel();
    release?.("done");
    await expect(second.promise).resolves.toBe("done");
  });
});
