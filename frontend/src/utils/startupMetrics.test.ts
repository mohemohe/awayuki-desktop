import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  frontendStartupMetricsSnapshot,
  markFrontendModuleEvaluated,
  markFrontendReactCommit,
  resetFrontendStartupMetricsForTest,
  scheduleFrontendInteractiveMark,
} from "./startupMetrics";

describe("frontend startup metrics", () => {
  beforeEach(() => {
    resetFrontendStartupMetricsForTest();
    vi.restoreAllMocks();
  });

  it("records module, commit, interactive, and navigation milestones in memory", () => {
    const now = vi
      .spyOn(performance, "now")
      .mockReturnValueOnce(120)
      .mockReturnValueOnce(140)
      .mockReturnValueOnce(160);
    vi.spyOn(performance, "getEntriesByType").mockImplementation((type) => {
      if (type === "resource") {
        return [
          { initiatorType: "script", responseEnd: 80 },
          { initiatorType: "fetch", responseEnd: 100 },
        ] as unknown as PerformanceEntry[];
      }
      if (type === "navigation") {
        return [{ domInteractive: 150 }] as unknown as PerformanceEntry[];
      }
      return [];
    });
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });

    markFrontendModuleEvaluated();
    markFrontendReactCommit();
    scheduleFrontendInteractiveMark();

    expect(frontendStartupMetricsSnapshot()).toMatchObject({
      moduleEvaluatedMs: 120,
      lastInitialScriptResponseMs: 80,
      parseEvaluateAfterScriptMs: 40,
      domInteractiveMs: 150,
      firstReactCommitMs: 140,
      firstInteractiveMs: 160,
    });
    expect(now).toHaveBeenCalledTimes(3);
  });
});
