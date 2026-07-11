import { beforeEach, describe, expect, it } from "vitest";
import {
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
    }

    const stream = renderMetricSnapshot("timeline:stream-100eps");
    expect(stream.commits).toBe(300);
    expect(stream.sampleCount).toBe(240);
    expect(stream.p95DurationMs).toBeGreaterThan(0);
    expect(renderMetricSnapshot("timeline:scroll").sampleCount).toBe(240);
    expect(renderMetricSnapshot("profile:open").sampleCount).toBe(240);
  });
});
