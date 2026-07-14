import type { ColumnSummary } from "../types/app";

export const ANALYTICAL_REFRESH_INITIAL_DELAY_MS = 2_000;
export const ANALYTICAL_REFRESH_COOLDOWN_MS = 30_000;

type DirtyColumn = {
  column: ColumnSummary;
  version: number;
};

type AnalyticalTimelineRefreshOptions = {
  refresh: (column: ColumnSummary) => Promise<void>;
  canAutoRefresh: (column: ColumnSummary) => boolean;
  initialDelayMs?: number;
  cooldownMs?: number;
  now?: () => number;
};

/**
 * Coalesces arbitrary SQLite-backed timeline invalidations into bounded waves.
 *
 * Custom SQL and YQ membership cannot be inferred from a generic stream event,
 * but running every query for every event turns a busy federation stream into a
 * permanent full-table scan. Visible columns are refreshed sequentially after
 * a short delay and no more than once per cooldown. Hidden/scrolled columns
 * stay dirty until the user activates them.
 */
export class AnalyticalTimelineRefreshCoordinator {
  private readonly dirty = new Map<string, DirtyColumn>();
  private readonly urgent = new Set<string>();
  private readonly failedVersions = new Map<string, number>();
  private readonly initialDelayMs: number;
  private readonly cooldownMs: number;
  private readonly now: () => number;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private timerDueAt = Number.POSITIVE_INFINITY;
  private running: Promise<void> | undefined;
  private nextAutomaticRefreshAt = 0;

  constructor(private readonly options: AnalyticalTimelineRefreshOptions) {
    this.initialDelayMs =
      options.initialDelayMs ?? ANALYTICAL_REFRESH_INITIAL_DELAY_MS;
    this.cooldownMs = options.cooldownMs ?? ANALYTICAL_REFRESH_COOLDOWN_MS;
    this.now = options.now ?? Date.now;
  }

  invalidate(column: ColumnSummary) {
    const current = this.dirty.get(column.id);
    this.dirty.set(column.id, {
      column,
      version: (current?.version ?? 0) + 1,
    });
    this.failedVersions.delete(column.id);
    if (this.options.canAutoRefresh(column)) this.schedule(false);
  }

  activate(column: ColumnSummary) {
    const current = this.dirty.get(column.id);
    if (!current) return;
    this.dirty.set(column.id, { ...current, column });
    this.failedVersions.delete(column.id);
    this.urgent.add(column.id);
    this.schedule(true);
  }

  cancel(columnId: string) {
    this.dirty.delete(columnId);
    this.urgent.delete(columnId);
    this.failedVersions.delete(columnId);
  }

  reset() {
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = undefined;
    this.timerDueAt = Number.POSITIVE_INFINITY;
    this.dirty.clear();
    this.urgent.clear();
    this.failedVersions.clear();
    this.nextAutomaticRefreshAt = 0;
  }

  async flushForTest() {
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = undefined;
    this.timerDueAt = Number.POSITIVE_INFINITY;
    await this.run(true);
  }

  isDirty(columnId: string) {
    return this.dirty.has(columnId);
  }

  private schedule(urgent: boolean) {
    if (this.running) return;
    const now = this.now();
    const delay = urgent
      ? 0
      : Math.max(
          this.initialDelayMs,
          Math.max(0, this.nextAutomaticRefreshAt - now),
        );
    const dueAt = now + delay;
    if (this.timer !== undefined && this.timerDueAt <= dueAt) return;
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timerDueAt = dueAt;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.timerDueAt = Number.POSITIVE_INFINITY;
      void this.run(false);
    }, delay);
  }

  private run(_force: boolean) {
    if (this.running) return this.running;
    const candidates = [...this.dirty.values()]
      .filter(
        ({ column, version }) =>
          this.urgent.has(column.id) ||
          (this.options.canAutoRefresh(column) &&
            this.failedVersions.get(column.id) !== version),
      )
      .sort((left, right) => {
        const urgentOrder =
          Number(this.urgent.has(right.column.id)) -
          Number(this.urgent.has(left.column.id));
        return (
          urgentOrder ||
          left.column.paneIndex - right.column.paneIndex ||
          left.column.position - right.column.position
        );
      });
    if (candidates.length === 0) return Promise.resolve();

    const work = async () => {
      for (const candidate of candidates) {
        const current = this.dirty.get(candidate.column.id);
        if (!current) continue;
        const isUrgent = this.urgent.delete(candidate.column.id);
        if (!isUrgent && !this.options.canAutoRefresh(current.column)) {
          continue;
        }
        try {
          await this.options.refresh(current.column);
          if (this.dirty.get(current.column.id)?.version === current.version) {
            this.dirty.delete(current.column.id);
            this.failedVersions.delete(current.column.id);
          }
        } catch {
          // Keep the invalidation dirty but block this exact version from the
          // automatic cooldown loop. A later commit increments the version,
          // while explicit activation retries it immediately.
          if (this.dirty.get(current.column.id)?.version === current.version) {
            this.failedVersions.set(current.column.id, current.version);
          }
        }
      }
      this.nextAutomaticRefreshAt = this.now() + this.cooldownMs;
    };

    this.running = work().finally(() => {
      this.running = undefined;
      if (this.urgent.size > 0) {
        this.schedule(true);
      } else if (
        [...this.dirty.values()].some(
          ({ column, version }) =>
            this.options.canAutoRefresh(column) &&
            this.failedVersions.get(column.id) !== version,
        )
      ) {
        this.schedule(false);
      }
    });
    return this.running;
  }
}
