import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clearMediaRetryCache,
  mediaRetryCacheSnapshot,
  scheduleMediaProbe,
  setMediaProbeForTesting,
} from "./mediaRetryCoordinator";

afterEach(() => {
  clearMediaRetryCache();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("media retry coordinator", () => {
  it("shares one probe for the same failed URL", async () => {
    vi.useFakeTimers();
    const probe = vi.fn(async () => true);
    const restore = setMediaProbeForTesting(probe);

    const first = scheduleMediaProbe("https://example.test/avatar.png", 10);
    const second = scheduleMediaProbe("https://example.test/avatar.png", 10);
    await vi.runAllTimersAsync();

    expect(await first).toBe(true);
    expect(await second).toBe(true);
    expect(probe).toHaveBeenCalledTimes(1);
    restore();
  });

  it("cancels a delayed probe after its final viewport consumer leaves", async () => {
    vi.useFakeTimers();
    const probe = vi.fn(async () => true);
    const restore = setMediaProbeForTesting(probe);
    const controller = new AbortController();
    const result = scheduleMediaProbe(
      "https://example.test/offscreen.png",
      10_000,
      controller.signal,
    );

    controller.abort();
    await expect(result).resolves.toBe(false);
    await vi.runAllTimersAsync();
    expect(probe).not.toHaveBeenCalled();
    restore();
  });

  it("keeps a shared delayed probe while another viewport consumer remains", async () => {
    vi.useFakeTimers();
    const probe = vi.fn(async () => true);
    const restore = setMediaProbeForTesting(probe);
    const firstController = new AbortController();
    const secondController = new AbortController();
    const first = scheduleMediaProbe(
      "https://example.test/shared.png",
      10,
      firstController.signal,
    );
    const second = scheduleMediaProbe(
      "https://example.test/shared.png",
      10,
      secondController.signal,
    );

    firstController.abort();
    await expect(first).resolves.toBe(false);
    await vi.runAllTimersAsync();
    await expect(second).resolves.toBe(true);
    expect(probe).toHaveBeenCalledTimes(1);
    restore();
  });

  it("aborts a started probe after its final viewport consumer leaves", async () => {
    vi.useFakeTimers();
    let probeSignal: AbortSignal | undefined;
    const probe = vi.fn((_url: string, signal?: AbortSignal) => {
      probeSignal = signal;
      return new Promise<boolean>((resolve) => {
        signal?.addEventListener("abort", () => resolve(false), { once: true });
      });
    });
    const restore = setMediaProbeForTesting(probe);
    const controller = new AbortController();
    const result = scheduleMediaProbe(
      "https://example.test/started.webp",
      0,
      controller.signal,
    );

    await vi.advanceTimersByTimeAsync(300);
    expect(probe).toHaveBeenCalledTimes(1);
    expect(probeSignal?.aborted).toBe(false);

    controller.abort();
    await expect(result).resolves.toBe(false);
    expect(probeSignal?.aborted).toBe(true);
    expect(mediaRetryCacheSnapshot().inFlight).toBe(0);
    restore();
  });

  it("times out a started probe that never settles", async () => {
    vi.useFakeTimers();
    let probeSignal: AbortSignal | undefined;
    const probe = vi.fn((_url: string, signal?: AbortSignal) => {
      probeSignal = signal;
      return new Promise<boolean>(() => undefined);
    });
    const restore = setMediaProbeForTesting(probe);
    const result = scheduleMediaProbe("https://example.test/stalled.webp", 0);

    await vi.advanceTimersByTimeAsync(300);
    expect(probe).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(15_000);

    await expect(result).resolves.toBe(false);
    expect(probeSignal?.aborted).toBe(true);
    expect(mediaRetryCacheSnapshot().inFlight).toBe(0);
    restore();
  });

  it("clears the browser image source and handlers when a probe is aborted", async () => {
    vi.useFakeTimers();
    class FakeImage {
      static latest: FakeImage | undefined;
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;
      src = "";
      removeAttribute = vi.fn((name: string) => {
        if (name === "src") this.src = "";
      });

      constructor() {
        FakeImage.latest = this;
      }
    }
    vi.stubGlobal("Image", FakeImage);
    const controller = new AbortController();
    const result = scheduleMediaProbe(
      "https://example.test/browser.webp",
      0,
      controller.signal,
    );

    await vi.advanceTimersByTimeAsync(300);
    const image = FakeImage.latest;
    expect(image?.src).toBe("https://example.test/browser.webp");
    controller.abort();

    await expect(result).resolves.toBe(false);
    expect(image?.onload).toBeNull();
    expect(image?.onerror).toBeNull();
    expect(image?.removeAttribute).toHaveBeenCalledWith("src");
    expect(image?.src).toBe("");
  });
});
