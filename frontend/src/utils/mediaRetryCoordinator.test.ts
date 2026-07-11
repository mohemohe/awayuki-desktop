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
});
