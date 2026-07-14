export type FrontendStartupMetrics = {
  moduleEvaluatedMs: number;
  lastInitialScriptResponseMs: number;
  parseEvaluateAfterScriptMs: number;
  domInteractiveMs: number;
  firstReactCommitMs: number;
  firstInteractiveMs: number;
  jsHeapUsedBytes: number;
  jsHeapLimitBytes: number;
};

type PerformanceWithMemory = Performance & {
  memory?: {
    usedJSHeapSize?: number;
    jsHeapSizeLimit?: number;
  };
};

const metrics: FrontendStartupMetrics = {
  moduleEvaluatedMs: 0,
  lastInitialScriptResponseMs: 0,
  parseEvaluateAfterScriptMs: 0,
  domInteractiveMs: 0,
  firstReactCommitMs: 0,
  firstInteractiveMs: 0,
  jsHeapUsedBytes: 0,
  jsHeapLimitBytes: 0,
};
let interactiveScheduled = false;

/** Record the end of static module evaluation before React starts rendering. */
export function markFrontendModuleEvaluated() {
  if (metrics.moduleEvaluatedMs > 0) return;
  metrics.moduleEvaluatedMs = elapsedNow();
  const scriptResponses = performance
    .getEntriesByType("resource")
    .filter(
      (entry): entry is PerformanceResourceTiming =>
        "initiatorType" in entry &&
        (entry as PerformanceResourceTiming).initiatorType === "script",
    )
    .map((entry) => entry.responseEnd);
  metrics.lastInitialScriptResponseMs = Math.max(0, ...scriptResponses);
  metrics.parseEvaluateAfterScriptMs = Math.max(
    0,
    metrics.moduleEvaluatedMs - metrics.lastInitialScriptResponseMs,
  );
  refreshNavigationAndHeapMetrics();
}

export function markFrontendReactCommit() {
  if (metrics.firstReactCommitMs === 0) {
    metrics.firstReactCommitMs = elapsedNow();
  }
  refreshNavigationAndHeapMetrics();
}

/** Mark the first painted frame after the usable application snapshot commits. */
export function scheduleFrontendInteractiveMark() {
  if (metrics.firstInteractiveMs > 0 || interactiveScheduled) return;
  interactiveScheduled = true;
  const complete = () => {
    metrics.firstInteractiveMs = elapsedNow();
    interactiveScheduled = false;
    refreshNavigationAndHeapMetrics();
  };
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(() => requestAnimationFrame(complete));
  } else {
    setTimeout(complete, 0);
  }
}

export function frontendStartupMetricsSnapshot(): FrontendStartupMetrics {
  refreshNavigationAndHeapMetrics();
  return { ...metrics };
}

export function resetFrontendStartupMetricsForTest() {
  for (const key of Object.keys(metrics) as Array<keyof FrontendStartupMetrics>) {
    metrics[key] = 0;
  }
  interactiveScheduled = false;
}

function refreshNavigationAndHeapMetrics() {
  const navigation = performance.getEntriesByType(
    "navigation",
  )[0] as PerformanceNavigationTiming | undefined;
  if (navigation?.domInteractive) {
    metrics.domInteractiveMs = Math.max(0, navigation.domInteractive);
  }
  const memory = (performance as PerformanceWithMemory).memory;
  metrics.jsHeapUsedBytes = finiteInteger(memory?.usedJSHeapSize);
  metrics.jsHeapLimitBytes = finiteInteger(memory?.jsHeapSizeLimit);
}

function elapsedNow() {
  return Math.max(0, Math.round(performance.now()));
}

function finiteInteger(value: number | undefined) {
  return Number.isFinite(value) ? Math.max(0, Math.round(value ?? 0)) : 0;
}
