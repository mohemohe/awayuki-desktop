import type React from "react";

export type RenderMetricSnapshot = {
  commits: number;
  sampleCount: number;
  averageDurationMs: number;
  p95DurationMs: number;
  lastDurationMs: number;
  frameSampleCount: number;
  frameAverageDurationMs: number;
  frameP95DurationMs: number;
  lastFrameDurationMs: number;
};

export type FrontendRenderMetricsSnapshot = {
  timelineStream: RenderMetricSnapshot;
  timelineScroll: RenderMetricSnapshot;
  profileOpen: RenderMetricSnapshot;
};

export type RenderScenario =
  | "timeline:stream"
  | "timeline:scroll"
  | "profile:open";

const SAMPLE_LIMIT = 240;
const samples = new Map<string, number[]>();
const commitCounts = new Map<string, number>();
const frameSamples = new Map<string, number[]>();
const pendingPaintScenarios = new Set<RenderScenario>();
let activeScenario: RenderScenario | undefined;
let scenarioGeneration = 0;

export const recordReactCommit: React.ProfilerOnRenderCallback = (
  id,
  _phase,
  actualDuration,
) => {
  recordRenderDuration(id, actualDuration);
  if (id.startsWith("profile:open:")) {
    recordRenderDuration("profile:open", actualDuration);
  }
  if (activeScenario) recordRenderDuration(activeScenario, actualDuration);
};

/** Attribute the next React commit before the following animation frame. */
export function markNextRenderScenario(scenario: RenderScenario) {
  activeScenario = scenario;
  const generation = ++scenarioGeneration;
  const clear = () => {
    if (scenarioGeneration === generation) activeScenario = undefined;
  };
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(clear);
  else queueMicrotask(clear);
}

export function recordRenderDuration(id: string, durationMs: number) {
  const durations = samples.get(id) ?? [];
  durations.push(Math.max(0, durationMs));
  if (durations.length > SAMPLE_LIMIT) durations.shift();
  samples.set(id, durations);
  commitCounts.set(id, (commitCounts.get(id) ?? 0) + 1);
}

/** Measure scheduling + React commit + WebView paint without retaining IDs. */
export function measureNextPaint(scenario: RenderScenario) {
  if (
    pendingPaintScenarios.has(scenario) ||
    typeof requestAnimationFrame !== "function"
  ) {
    return;
  }
  pendingPaintScenarios.add(scenario);
  const startedAt = performance.now();
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      recordFrameDuration(scenario, performance.now() - startedAt);
      pendingPaintScenarios.delete(scenario);
    });
  });
}

export function recordFrameDuration(id: string, durationMs: number) {
  const durations = frameSamples.get(id) ?? [];
  durations.push(Math.max(0, durationMs));
  if (durations.length > SAMPLE_LIMIT) durations.shift();
  frameSamples.set(id, durations);
}

export function renderMetricSnapshot(id: string): RenderMetricSnapshot {
  const durations = samples.get(id) ?? [];
  const sorted = [...durations].sort((left, right) => left - right);
  const p95Index = Math.max(0, Math.ceil(sorted.length * 0.95) - 1);
  const frames = frameSamples.get(id) ?? [];
  const sortedFrames = [...frames].sort((left, right) => left - right);
  const frameP95Index = Math.max(0, Math.ceil(sortedFrames.length * 0.95) - 1);
  return {
    commits: commitCounts.get(id) ?? 0,
    sampleCount: durations.length,
    averageDurationMs:
      durations.length === 0
        ? 0
        : durations.reduce((sum, duration) => sum + duration, 0) /
          durations.length,
    p95DurationMs: sorted[p95Index] ?? 0,
    lastDurationMs: durations[durations.length - 1] ?? 0,
    frameSampleCount: frames.length,
    frameAverageDurationMs:
      frames.length === 0
        ? 0
        : frames.reduce((sum, duration) => sum + duration, 0) / frames.length,
    frameP95DurationMs: sortedFrames[frameP95Index] ?? 0,
    lastFrameDurationMs: frames[frames.length - 1] ?? 0,
  };
}

export function frontendRenderMetricsSnapshot(): FrontendRenderMetricsSnapshot {
  return {
    timelineStream: roundedSnapshot("timeline:stream"),
    timelineScroll: roundedSnapshot("timeline:scroll"),
    profileOpen: roundedSnapshot("profile:open"),
  };
}

export function resetFrontendRenderMetrics() {
  samples.clear();
  commitCounts.clear();
  frameSamples.clear();
  pendingPaintScenarios.clear();
  activeScenario = undefined;
  scenarioGeneration += 1;
}

export const resetRenderMetricsForTest = resetFrontendRenderMetrics;

function roundedSnapshot(id: string): RenderMetricSnapshot {
  const snapshot = renderMetricSnapshot(id);
  return Object.fromEntries(
    Object.entries(snapshot).map(([key, value]) => [key, Math.round(value)]),
  ) as RenderMetricSnapshot;
}
