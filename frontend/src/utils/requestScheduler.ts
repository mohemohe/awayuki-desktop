export type RequestLane =
  | "timeline"
  | "analytics"
  | "profile"
  | "autocomplete";

export type RequestTaskContext = {
  signal: AbortSignal;
  generation: number;
  isCurrent: () => boolean;
};

export type RequestScheduleOptions = {
  key: string;
  lane: RequestLane;
  priority?: number;
  replace?: boolean;
};

export type RequestLaneMetrics = {
  queued: number;
  running: number;
  completed: number;
  cancelled: number;
  maxRunning: number;
  p95DurationMs: number;
};

type QueuedRequest<T> = {
  key: string;
  lane: RequestLane;
  priority: number;
  order: number;
  generation: number;
  controller: AbortController;
  task: (context: RequestTaskContext) => Promise<T>;
  resolve: (value: T | PromiseLike<T>) => void;
  reject: (reason?: unknown) => void;
};

type ActiveRequest = {
  generation: number;
  controller: AbortController;
  lane: RequestLane;
};

export class RequestCancelledError extends Error {
  constructor(readonly requestKey: string) {
    super(`Request cancelled: ${requestKey}`);
    this.name = "RequestCancelledError";
  }
}

const EMPTY_METRICS = (): RequestLaneMetrics => ({
  queued: 0,
  running: 0,
  completed: 0,
  cancelled: 0,
  maxRunning: 0,
  p95DurationMs: 0,
});

/**
 * Small priority scheduler shared by UI reads. Requests are bounded per use
 * case, visible resources can jump ahead of background work, and a resource
 * key owns a generation so stale completions cannot be committed.
 */
export class RequestScheduler {
  private queue: QueuedRequest<unknown>[] = [];
  private active = new Map<string, ActiveRequest>();
  private generations = new Map<string, number>();
  private runningByLane = new Map<RequestLane, number>();
  private metricsByLane = new Map<RequestLane, RequestLaneMetrics>();
  private durationsByLane = new Map<RequestLane, number[]>();
  private order = 0;

  constructor(private readonly limits: Record<RequestLane, number>) {}

  /**
   * Resize one lane at runtime. Raising the limit starts queued work
   * immediately; lowering it never aborts running work and only throttles
   * future starts.
   */
  setLaneLimit(lane: RequestLane, limit: number) {
    const next = Math.max(1, Math.floor(limit));
    if (this.limits[lane] === next) return;
    this.limits[lane] = next;
    this.pump();
  }

  schedule<T>(
    options: RequestScheduleOptions,
    task: (context: RequestTaskContext) => Promise<T>,
  ): Promise<T> {
    if (options.replace !== false) this.cancel(options.key);
    const generation = (this.generations.get(options.key) ?? 0) + 1;
    this.generations.set(options.key, generation);
    const controller = new AbortController();

    const promise = new Promise<T>((resolve, reject) => {
      this.queue.push({
        key: options.key,
        lane: options.lane,
        priority: options.priority ?? 0,
        order: this.order++,
        generation,
        controller,
        task,
        resolve,
        reject,
      } as QueuedRequest<unknown>);
      this.refreshQueuedMetrics();
      this.pump();
    });
    return promise;
  }

  cancel(key: string) {
    const queued = this.queue.filter((request) => request.key === key);
    for (const request of queued) request.controller.abort();
    this.active.get(key)?.controller.abort();
    if (queued.length > 0 || this.active.has(key)) {
      const metrics = this.metricsFor(
        queued[0]?.lane ?? this.active.get(key)?.lane ?? "timeline",
      );
      metrics.cancelled += 1;
    }
    this.pump();
  }

  cancelPrefix(prefix: string) {
    const keys = new Set([
      ...this.queue
        .filter((request) => request.key.startsWith(prefix))
        .map((request) => request.key),
      ...[...this.active.keys()].filter((key) => key.startsWith(prefix)),
    ]);
    for (const key of keys) this.cancel(key);
  }

  cancelAll() {
    const keys = new Set([
      ...this.queue.map((request) => request.key),
      ...this.active.keys(),
    ]);
    for (const key of keys) this.cancel(key);
  }

  isCurrent(key: string, generation: number) {
    return (
      this.generations.get(key) === generation &&
      !this.active.get(key)?.controller.signal.aborted
    );
  }

  metrics(): Record<RequestLane, RequestLaneMetrics> {
    this.refreshQueuedMetrics();
    return {
      timeline: { ...this.metricsFor("timeline") },
      analytics: { ...this.metricsFor("analytics") },
      profile: { ...this.metricsFor("profile") },
      autocomplete: { ...this.metricsFor("autocomplete") },
    };
  }

  resetForTest() {
    this.cancelAll();
    this.queue = [];
    this.active.clear();
    this.generations.clear();
    this.runningByLane.clear();
    this.metricsByLane.clear();
    this.durationsByLane.clear();
    this.order = 0;
  }

  private pump() {
    this.rejectCancelledQueuedRequests();
    this.queue.sort(
      (left, right) =>
        right.priority - left.priority || left.order - right.order,
    );

    let started = true;
    while (started) {
      started = false;
      const index = this.queue.findIndex(
        (request) =>
          !request.controller.signal.aborted &&
          (this.runningByLane.get(request.lane) ?? 0) < this.limits[request.lane],
      );
      if (index < 0) break;
      const [request] = this.queue.splice(index, 1);
      this.start(request);
      started = true;
    }
    this.refreshQueuedMetrics();
  }

  private start(request: QueuedRequest<unknown>) {
    const running = (this.runningByLane.get(request.lane) ?? 0) + 1;
    this.runningByLane.set(request.lane, running);
    this.active.set(request.key, {
      generation: request.generation,
      controller: request.controller,
      lane: request.lane,
    });
    const metrics = this.metricsFor(request.lane);
    metrics.running = running;
    metrics.maxRunning = Math.max(metrics.maxRunning, running);
    const startedAt = performance.now();
    const context: RequestTaskContext = {
      signal: request.controller.signal,
      generation: request.generation,
      isCurrent: () =>
        !request.controller.signal.aborted &&
        this.generations.get(request.key) === request.generation,
    };

    const cancelled = new Promise<never>((_, reject) => {
      request.controller.signal.addEventListener(
        "abort",
        () => reject(new RequestCancelledError(request.key)),
        { once: true },
      );
    });
    void Promise.race([
      Promise.resolve().then(() => request.task(context)),
      cancelled,
    ])
      .then((value) => {
        if (!context.isCurrent()) {
          request.reject(new RequestCancelledError(request.key));
          return;
        }
        metrics.completed += 1;
        request.resolve(value);
      })
      .catch((error) => {
        request.reject(
          request.controller.signal.aborted
            ? new RequestCancelledError(request.key)
            : error,
        );
      })
      .finally(() => {
        const duration = performance.now() - startedAt;
        this.recordDuration(request.lane, duration);
        const nextRunning = Math.max(
          0,
          (this.runningByLane.get(request.lane) ?? 1) - 1,
        );
        this.runningByLane.set(request.lane, nextRunning);
        metrics.running = nextRunning;
        if (this.active.get(request.key)?.generation === request.generation) {
          this.active.delete(request.key);
        }
        this.pump();
      });
  }

  private rejectCancelledQueuedRequests() {
    const retained: QueuedRequest<unknown>[] = [];
    for (const request of this.queue) {
      if (request.controller.signal.aborted) {
        request.reject(new RequestCancelledError(request.key));
      } else {
        retained.push(request);
      }
    }
    this.queue = retained;
  }

  private metricsFor(lane: RequestLane) {
    let metrics = this.metricsByLane.get(lane);
    if (!metrics) {
      metrics = EMPTY_METRICS();
      this.metricsByLane.set(lane, metrics);
    }
    return metrics;
  }

  private refreshQueuedMetrics() {
    for (const lane of [
      "timeline",
      "analytics",
      "profile",
      "autocomplete",
    ] as const) {
      this.metricsFor(lane).queued = this.queue.filter(
        (request) => request.lane === lane && !request.controller.signal.aborted,
      ).length;
      this.metricsFor(lane).running = this.runningByLane.get(lane) ?? 0;
    }
  }

  private recordDuration(lane: RequestLane, duration: number) {
    const durations = this.durationsByLane.get(lane) ?? [];
    durations.push(duration);
    if (durations.length > 240) durations.shift();
    this.durationsByLane.set(lane, durations);
    const sorted = [...durations].sort((left, right) => left - right);
    const index = Math.max(0, Math.ceil(sorted.length * 0.95) - 1);
    this.metricsFor(lane).p95DurationMs = sorted[index] ?? duration;
  }
}

export const frontendRequestScheduler = new RequestScheduler({
  timeline: 4,
  analytics: 1,
  profile: 3,
  autocomplete: 2,
});
