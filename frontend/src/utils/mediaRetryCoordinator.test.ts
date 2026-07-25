import { afterEach, describe, expect, it, vi } from "vitest";
import {
  scheduleMediaProbe,
  setMediaProbeForTesting,
} from "./mediaRetryCoordinator";

afterEach(() => {
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
});
