import type React from "react";

export type RenderMetricSnapshot = {
  commits: number;
  sampleCount: number;
  averageDurationMs: number;
  p95DurationMs: number;
  lastDurationMs: number;
};

const SAMPLE_LIMIT = 240;
const samples = new Map<string, number[]>();
const commitCounts = new Map<string, number>();

export const recordReactCommit: React.ProfilerOnRenderCallback = (
  id,
  _phase,
  actualDuration,
) => {
  recordRenderDuration(id, actualDuration);
};

export function recordRenderDuration(id: string, durationMs: number) {
  const durations = samples.get(id) ?? [];
  durations.push(Math.max(0, durationMs));
  if (durations.length > SAMPLE_LIMIT) durations.shift();
  samples.set(id, durations);
  commitCounts.set(id, (commitCounts.get(id) ?? 0) + 1);
}

export function renderMetricSnapshot(id: string): RenderMetricSnapshot {
  const durations = samples.get(id) ?? [];
  const sorted = [...durations].sort((left, right) => left - right);
  const p95Index = Math.max(0, Math.ceil(sorted.length * 0.95) - 1);
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
  };
}

export function resetRenderMetricsForTest() {
  samples.clear();
  commitCounts.clear();
}
