export type SettingSavePhase =
  | "idle"
  | "dirty"
  | "saving"
  | "saved"
  | "failed"
  | "cancelled"
  | "conflict";

export type SettingSaveState = {
  key: string;
  generation: number;
  phase: SettingSavePhase;
  draft: unknown;
  lastSaved: unknown;
  error?: string;
};

type PersistedResult<TResult> = {
  result: TResult;
  generation: number;
};

type Waiter = {
  generation: number;
  resolve: () => void;
};

type Entry<TResult> = {
  state: SettingSaveState;
  scope: number;
  timer?: number;
  inFlight?: Promise<PersistedResult<TResult>>;
  waiters: Waiter[];
};

export type SettingsMutationCoordinatorOptions<TResult> = {
  persist: (key: string, value: unknown) => Promise<TResult>;
  onState: (state: SettingSaveState) => void;
  onPersisted: (
    key: string,
    result: TResult,
    state: SettingSaveState,
  ) => void;
  debounceMs?: number;
};

/**
 * Serializes writes per setting key. The UI owns a draft immediately, while
 * only the newest generation may replace its last-saved value. Pending edits
 * are coalesced, which keeps text fields responsive without an IPC per keypress.
 */
export class SettingsMutationCoordinator<TResult> {
  private readonly entries = new Map<string, Entry<TResult>>();
  private scope = 0;
  private readonly debounceMs: number;

  constructor(
    private readonly options: SettingsMutationCoordinatorOptions<TResult>,
  ) {
    this.debounceMs = options.debounceMs ?? 400;
  }

  seed(key: string, value: unknown) {
    const existing = this.entries.get(key);
    if (existing && ["dirty", "saving"].includes(existing.state.phase)) return;
    const state: SettingSaveState = {
      key,
      generation: existing?.state.generation ?? 0,
      phase: "idle",
      draft: value,
      lastSaved: value,
    };
    this.entries.set(key, {
      state,
      scope: this.scope,
      waiters: existing?.waiters ?? [],
    });
    this.options.onState(state);
  }

  enqueue(key: string, value: unknown): Promise<void> {
    const existing = this.entries.get(key);
    const generation = (existing?.state.generation ?? 0) + 1;
    const entry: Entry<TResult> = existing ?? {
      state: {
        key,
        generation: 0,
        phase: "idle",
        draft: value,
        lastSaved: undefined,
      },
      scope: this.scope,
      waiters: [],
    };
    if (entry.timer !== undefined) window.clearTimeout(entry.timer);
    entry.scope = this.scope;
    entry.state = {
      ...entry.state,
      generation,
      phase: "dirty",
      draft: value,
      error: undefined,
    };
    this.entries.set(key, entry);
    this.options.onState(entry.state);

    const completion = new Promise<void>((resolve) => {
      entry.waiters.push({ generation, resolve });
    });
    entry.timer = window.setTimeout(() => {
      entry.timer = undefined;
      void this.drain(key);
    }, this.debounceMs);
    return completion;
  }

  async flush(key?: string) {
    const keys = key ? [key] : [...this.entries.keys()];
    for (const target of keys) {
      const entry = this.entries.get(target);
      if (!entry) continue;
      if (entry.timer !== undefined) {
        window.clearTimeout(entry.timer);
        entry.timer = undefined;
      }
      await this.drain(target);
    }
  }

  /** Cancels only unsent work. In-flight IPC may finish, but its old scope is ignored. */
  resetScope() {
    this.scope += 1;
    for (const entry of this.entries.values()) {
      if (entry.timer !== undefined) window.clearTimeout(entry.timer);
      entry.timer = undefined;
      const hadPending = entry.state.phase === "dirty" || entry.state.phase === "saving";
      entry.state = {
        ...entry.state,
        phase: hadPending ? "conflict" : "cancelled",
      };
      for (const waiter of entry.waiters) waiter.resolve();
      entry.waiters = [];
      this.options.onState(entry.state);
    }
  }

  state(key: string) {
    return this.entries.get(key)?.state;
  }

  private async drain(key: string): Promise<void> {
    const entry = this.entries.get(key);
    if (!entry) return;
    if (entry.inFlight) {
      await entry.inFlight.catch(() => undefined);
      const latest = this.entries.get(key);
      if (latest?.state.phase === "dirty") await this.drain(key);
      return;
    }
    if (entry.state.phase !== "dirty") return;

    const generation = entry.state.generation;
    const value = entry.state.draft;
    const operationScope = entry.scope;
    entry.state = { ...entry.state, phase: "saving", error: undefined };
    this.options.onState(entry.state);
    const inFlight = this.options
      .persist(key, value)
      .then((result) => ({ result, generation }));
    entry.inFlight = inFlight;

    try {
      const persisted = await inFlight;
      const current = this.entries.get(key);
      if (!current || current.scope !== operationScope || this.scope !== operationScope) {
        return;
      }
      const isLatest = current.state.generation === persisted.generation;
      if (isLatest) {
        current.state = {
          ...current.state,
          phase: "saved",
          lastSaved: current.state.draft,
          error: undefined,
        };
        this.options.onPersisted(key, persisted.result, current.state);
        this.options.onState(current.state);
      } else {
        current.state = { ...current.state, phase: "dirty" };
        this.options.onState(current.state);
      }
      this.resolveWaiters(current, persisted.generation);
    } catch (error) {
      const current = this.entries.get(key);
      if (!current || current.scope !== operationScope || this.scope !== operationScope) {
        return;
      }
      current.state = {
        ...current.state,
        phase: "failed",
        error: String(error),
      };
      this.resolveWaiters(current, current.state.generation);
      this.options.onState(current.state);
    } finally {
      const current = this.entries.get(key);
      if (current?.inFlight === inFlight) current.inFlight = undefined;
      if (current?.state.phase === "dirty") await this.drain(key);
    }
  }

  private resolveWaiters(entry: Entry<TResult>, generation: number) {
    const pending: Waiter[] = [];
    for (const waiter of entry.waiters) {
      if (waiter.generation <= generation) waiter.resolve();
      else pending.push(waiter);
    }
    entry.waiters = pending;
  }
}
