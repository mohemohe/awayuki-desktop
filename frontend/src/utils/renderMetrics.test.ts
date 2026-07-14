import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  frontendRenderMetricsSnapshot,
  markNextRenderScenario,
  measureNextPaint,
  recordFrameDuration,
  recordReactCommit,
  recordRenderDuration,
  renderMetricSnapshot,
  resetRenderMetricsForTest,
} from "./renderMetrics";

describe("render metrics", () => {
  beforeEach(resetRenderMetricsForTest);

  it("records burst/scroll/profile commits with a bounded sample window", () => {
    for (let index = 1; index <= 300; index += 1) {
      recordRenderDuration("timeline:stream-100eps", index / 10);
      recordRenderDuration("timeline:scroll", index / 20);
      recordRenderDuration("profile:open", index / 30);
      recordFrameDuration("timeline:stream-100eps", index / 5);
    }

    const stream = renderMetricSnapshot("timeline:stream-100eps");
    expect(stream.commits).toBe(300);
    expect(stream.sampleCount).toBe(240);
    expect(stream.p95DurationMs).toBeGreaterThan(0);
    expect(stream.frameSampleCount).toBe(240);
    expect(stream.frameP95DurationMs).toBeGreaterThan(0);
    expect(renderMetricSnapshot("timeline:scroll").sampleCount).toBe(240);
    expect(renderMetricSnapshot("profile:open").sampleCount).toBe(240);
  });

  it("aggregates anonymous stream, scroll, and profile scenarios", () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    });

    markNextRenderScenario("timeline:stream");
    recordReactCommit("timeline:scroll:pane-4", "update", 7.4, 8, 0, 0);
    frames.shift()?.(0);
    markNextRenderScenario("timeline:scroll");
    recordReactCommit("timeline:scroll:pane-4", "update", 3.2, 4, 0, 0);
    frames.shift()?.(0);
    recordReactCommit("profile:open:private-column-id", "mount", 5.6, 6, 0, 0);

    expect(frontendRenderMetricsSnapshot()).toEqual({
      timelineStream: {
        commits: 1,
        sampleCount: 1,
        averageDurationMs: 7,
        p95DurationMs: 7,
        lastDurationMs: 7,
        frameSampleCount: 0,
        frameAverageDurationMs: 0,
        frameP95DurationMs: 0,
        lastFrameDurationMs: 0,
      },
      timelineScroll: {
        commits: 1,
        sampleCount: 1,
        averageDurationMs: 3,
        p95DurationMs: 3,
        lastDurationMs: 3,
        frameSampleCount: 0,
        frameAverageDurationMs: 0,
        frameP95DurationMs: 0,
        lastFrameDurationMs: 0,
      },
      profileOpen: {
        commits: 1,
        sampleCount: 1,
        averageDurationMs: 6,
        p95DurationMs: 6,
        lastDurationMs: 6,
        frameSampleCount: 0,
        frameAverageDurationMs: 0,
        frameP95DurationMs: 0,
        lastFrameDurationMs: 0,
      },
    });
  });

  it("coalesces paint measurements and records the second animation frame", () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    });
    const now = vi
      .spyOn(performance, "now")
      .mockReturnValueOnce(10)
      .mockReturnValueOnce(42);

    measureNextPaint("timeline:stream");
    measureNextPaint("timeline:stream");
    expect(frames).toHaveLength(1);
    frames.shift()?.(16);
    expect(frames).toHaveLength(1);
    frames.shift()?.(32);

    expect(renderMetricSnapshot("timeline:stream")).toMatchObject({
      frameSampleCount: 1,
      frameAverageDurationMs: 32,
      frameP95DurationMs: 32,
      lastFrameDurationMs: 32,
    });
    now.mockRestore();
  });
});
