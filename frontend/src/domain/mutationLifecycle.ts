export type MutationPhase =
  | "idle"
  | "confirming"
  | "pending"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "uncertain";

export type MutationState = {
  key: string;
  operationId: string;
  phase: MutationPhase;
  error?: string;
};

export type MutationRunOptions<T> = {
  confirm?: () => Promise<boolean>;
  execute: (operationId: string) => Promise<T>;
  isUncertain?: (error: unknown) => boolean;
};

/** Deduplicates destructive UI actions and records their terminal state. */
export class MutationLifecycle {
  private active = new Map<string, Promise<unknown>>();
  private states = new Map<string, MutationState>();
  private scope = 0;

  constructor(
    private readonly onState: (state: MutationState) => void,
    private readonly onCancel: (operationId: string) => void = () => undefined,
  ) {}

  run<T>(key: string, options: MutationRunOptions<T>): Promise<T | undefined> {
    const existing = this.active.get(key);
    if (existing) return existing as Promise<T | undefined>;
    const operationId = crypto.randomUUID();
    const operationScope = this.scope;
    const run = this.execute(key, operationId, operationScope, options).finally(() => {
      if (this.active.get(key) === run) this.active.delete(key);
    });
    this.active.set(key, run);
    return run;
  }

  state(key: string) {
    return this.states.get(key);
  }

  /**
   * External writes cannot always be aborted. A scope change marks their
   * outcome uncertain and prevents a late response from mutating new account
   * state.
   */
  invalidateAll(reason = "Mutation crossed a resource scope") {
    this.scope += 1;
    for (const key of this.active.keys()) {
      const current = this.states.get(key);
      if (!current) continue;
      if (current.phase === "pending") this.onCancel(current.operationId);
      this.publish({ ...current, phase: "uncertain", error: reason });
    }
  }

  private async execute<T>(
    key: string,
    operationId: string,
    operationScope: number,
    options: MutationRunOptions<T>,
  ): Promise<T | undefined> {
    if (options.confirm) {
      this.publish({ key, operationId, phase: "confirming" });
      if (!(await options.confirm()) || this.scope !== operationScope) {
        this.publish({ key, operationId, phase: "cancelled" });
        return undefined;
      }
    }
    this.publish({ key, operationId, phase: "pending" });
    try {
      const value = await options.execute(operationId);
      if (this.scope !== operationScope) {
        this.publish({
          key,
          operationId,
          phase: "uncertain",
          error: "Mutation completed after its resource scope changed",
        });
        return undefined;
      }
      this.publish({ key, operationId, phase: "succeeded" });
      return value;
    } catch (error) {
      if (this.scope !== operationScope) {
        this.publish({
          key,
          operationId,
          phase: "uncertain",
          error: String(error),
        });
        return undefined;
      }
      this.publish({
        key,
        operationId,
        phase: options.isUncertain?.(error) ? "uncertain" : "failed",
        error: String(error),
      });
      return undefined;
    }
  }

  private publish(state: MutationState) {
    this.states.set(state.key, state);
    this.onState(state);
  }
}
