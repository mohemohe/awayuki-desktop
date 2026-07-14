export type TranslationLease<T> = {
  promise: Promise<T>;
  cancel: () => void;
};

type TranslationJob = {
  key: string;
  priority: number;
  order: number;
  consumers: Set<symbol>;
  controller: AbortController;
  task: (signal: AbortSignal) => Promise<unknown>;
  promise: Promise<unknown>;
  resolve: (value: unknown) => void;
  reject: (reason?: unknown) => void;
  running: boolean;
};

export class TranslationCancelledError extends Error {
  constructor(readonly translationKey: string) {
    super(`Translation cancelled: ${translationKey}`);
    this.name = "TranslationCancelledError";
  }
}

/** Bounded, visible-priority, single-flight queue for translation IPC. */
export class TranslationScheduler {
  private readonly jobs = new Map<string, TranslationJob>();
  private readonly queue: TranslationJob[] = [];
  private running = 0;
  private order = 0;

  constructor(private readonly concurrency = 3) {
    if (!Number.isInteger(concurrency) || concurrency < 1) {
      throw new Error("Translation concurrency must be a positive integer");
    }
  }

  schedule<T>(
    key: string,
    task: (signal: AbortSignal) => Promise<T>,
    priority = 0,
  ): TranslationLease<T> {
    const consumer = Symbol(key);
    let job = this.jobs.get(key);
    if (job) {
      job.consumers.add(consumer);
      job.priority = Math.max(job.priority, priority);
      this.sortQueue();
    } else {
      let resolve!: (value: unknown) => void;
      let reject!: (reason?: unknown) => void;
      const promise = new Promise<unknown>((nextResolve, nextReject) => {
        resolve = nextResolve;
        reject = nextReject;
      });
      // A cancelled consumer may intentionally ignore the shared promise.
      void promise.catch(() => undefined);
      job = {
        key,
        priority,
        order: this.order++,
        consumers: new Set([consumer]),
        controller: new AbortController(),
        task,
        promise,
        resolve,
        reject,
        running: false,
      };
      this.jobs.set(key, job);
      this.queue.push(job);
      this.sortQueue();
      this.pump();
    }

    let cancelled = false;
    return {
      promise: job.promise as Promise<T>,
      cancel: () => {
        if (cancelled) return;
        cancelled = true;
        this.release(job!, consumer);
      },
    };
  }

  snapshot() {
    return {
      queued: this.queue.filter((job) => !job.running).length,
      running: this.running,
      jobs: this.jobs.size,
    };
  }

  private release(job: TranslationJob, consumer: symbol) {
    job.consumers.delete(consumer);
    if (job.consumers.size > 0) return;
    job.controller.abort();
    if (!job.running) {
      this.removeQueued(job);
      this.jobs.delete(job.key);
      job.reject(new TranslationCancelledError(job.key));
    }
  }

  private pump() {
    while (this.running < this.concurrency) {
      const job = this.queue.find(
        (candidate) => !candidate.running && candidate.consumers.size > 0,
      );
      if (!job) return;
      job.running = true;
      this.running += 1;
      void job
        .task(job.controller.signal)
        .then((value) => {
          if (job.controller.signal.aborted || job.consumers.size === 0) {
            job.reject(new TranslationCancelledError(job.key));
          } else {
            job.resolve(value);
          }
        })
        .catch((error) => job.reject(error))
        .finally(() => {
          this.running -= 1;
          this.jobs.delete(job.key);
          this.removeQueued(job);
          this.pump();
        });
    }
  }

  private removeQueued(job: TranslationJob) {
    const index = this.queue.indexOf(job);
    if (index >= 0) this.queue.splice(index, 1);
  }

  private sortQueue() {
    this.queue.sort(
      (left, right) => right.priority - left.priority || left.order - right.order,
    );
  }
}

export const translationScheduler = new TranslationScheduler(3);
