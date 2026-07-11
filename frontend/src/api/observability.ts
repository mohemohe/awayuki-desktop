export type FrontendHealthSnapshot = {
  activeOperations: number;
  completedOperations: number;
  failedOperations: number;
  streamSequenceGaps: number;
  streamResyncs: number;
  pendingStreamEvents: number;
};

const health: FrontendHealthSnapshot = {
  activeOperations: 0,
  completedOperations: 0,
  failedOperations: 0,
  streamSequenceGaps: 0,
  streamResyncs: 0,
  pendingStreamEvents: 0,
};

export function startUiOperation() {
  health.activeOperations += 1;
  return createUuid();
}

export function completeUiOperation(failed: boolean) {
  health.activeOperations = Math.max(0, health.activeOperations - 1);
  if (failed) health.failedOperations += 1;
  else health.completedOperations += 1;
}

export function recordFrontendStreamGap() {
  health.streamSequenceGaps += 1;
}

export function recordFrontendStreamResync() {
  health.streamResyncs += 1;
}

export function setPendingStreamEvents(count: number) {
  health.pendingStreamEvents = Math.max(0, Math.trunc(count));
}

export function frontendHealthSnapshot(): FrontendHealthSnapshot {
  return { ...health };
}

function createUuid() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  // A valid UUID is required only as a correlation identifier. It is neither
  // a credential nor persisted state.
  return "10000000-1000-4000-8000-100000000000".replace(/[018]/g, (char) =>
    (
      Number(char) ^
      (Math.floor(Math.random() * 256) & (15 >> (Number(char) / 4)))
    ).toString(16),
  );
}

