import { afterEach, describe, expect, it, vi } from "vitest";
import {
  blurHashCacheSnapshot,
  blurHashToDataUrl,
  clearBlurHashCache,
} from "./blurhash";
import {
  mediaRetryCacheSnapshot,
  scheduleMediaProbe,
  setMediaProbeForTesting,
} from "./mediaRetryCoordinator";
import {
  clearTranslationCache,
  translationCache,
} from "../features/timeline/translation";

afterEach(() => {
  clearBlurHashCache();
  clearTranslationCache();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("eight-hour synthetic timeline scroll", () => {
  it("keeps cache items, weights, and offscreen retry timers bounded", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-12T00:00:00Z"));
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    const restoreProbe = setMediaProbeForTesting(vi.fn(async () => true));
    const validBlurHash = "LEHV6nWB2yk8pyo0adR*.7kCMdnj";

    // Thirty-second churn over eight hours: each step represents newly visible
    // statuses plus a media item that leaves the virtualized viewport before
    // its delayed retry begins.
    for (let step = 0; step < 8 * 60 * 2; step += 1) {
      translationCache.set(`status:${step}:engine:content`, {
        text: `translated-${step}`.repeat(8),
      });
      blurHashToDataUrl(validBlurHash, 1 + (step % 320), 1);

      const controller = new AbortController();
      const probe = scheduleMediaProbe(
        `https://media.example/${step}.png`,
        30_000,
        controller.signal,
      );
      controller.abort();
      await expect(probe).resolves.toBe(false);
      await vi.advanceTimersByTimeAsync(30_000);
    }

    expect(translationCache.size).toBeLessThanOrEqual(500);
    expect(translationCache.weight).toBeLessThanOrEqual(2 * 1024 * 1024);
    expect(blurHashCacheSnapshot().items).toBeLessThanOrEqual(256);
    expect(blurHashCacheSnapshot().weight).toBeLessThanOrEqual(8 * 1024 * 1024);
    expect(mediaRetryCacheSnapshot()).toEqual({ inFlight: 0, negative: 0 });
    expect(vi.getTimerCount()).toBe(0);
    restoreProbe();
  });
});
