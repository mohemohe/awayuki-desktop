import type { SidecarEntry, SidecarSettings } from "../types/app";

export const SIDECAR_MIN_WIDTH = 160;
export const SIDECAR_DEFAULT_WIDTH = 500;

export function isSupportedSidecarUrl(value: string) {
  try {
    const url = new URL(value.trim());
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      url.hostname.length > 0
    );
  } catch {
    return false;
  }
}

export function normalizeSidecarWidth(width: number | string) {
  const parsed = Number(width);
  if (!Number.isFinite(parsed) || parsed <= 0) return SIDECAR_DEFAULT_WIDTH;
  return Math.max(SIDECAR_MIN_WIDTH, Math.floor(parsed));
}

export function normalizeSidecarUserStyle(userStyle: string | undefined) {
  return userStyle ?? "";
}

export function effectiveSidecarUserStyle(sidecar: SidecarEntry) {
  return sidecar.userStyleEnabled
    ? normalizeSidecarUserStyle(sidecar.userStyle)
    : "";
}

export function normalizeSidecarSettings(
  settings?: SidecarSettings,
): SidecarSettings {
  const entries =
    settings?.entries
      .filter((entry) => isSupportedSidecarUrl(entry.url))
      .map((entry) => ({
        ...entry,
        name: entry.name.trim() || "Sidecar",
        url: entry.url.trim(),
        userStyleEnabled: entry.userStyleEnabled ?? false,
        userStyle: normalizeSidecarUserStyle(entry.userStyle),
        width: normalizeSidecarWidth(entry.width),
      })) ?? [];
  return { entries, mainViewIndex: 0 };
}

export function sidecarWebviewLabel(id: string) {
  return `sidecar-${id}`.replace(/[^a-zA-Z0-9-/:_]/g, "_");
}

export type SidecarLifecycleStatus =
  | "creating"
  | "ready"
  | "visible"
  | "navigating"
  | "closing"
  | "failed";

export type SidecarOperation = {
  id: string;
  generation: number;
  signal: AbortSignal;
};

type SidecarLifecycleRecord = {
  generation: number;
  status: SidecarLifecycleStatus;
  controller: AbortController;
};

/**
 * Owns logical cancellation for sidecar operations. Tauri IPC itself is not
 * abortable, so every completion must prove that its generation is current
 * before it can publish state or apply a later layout step.
 */
export class SidecarLifecycleManager {
  private readonly records = new Map<string, SidecarLifecycleRecord>();
  private readonly generations = new Map<string, number>();

  begin(id: string, status: SidecarLifecycleStatus): SidecarOperation {
    this.records.get(id)?.controller.abort();
    const generation = (this.generations.get(id) ?? 0) + 1;
    const controller = new AbortController();
    this.generations.set(id, generation);
    this.records.set(id, { generation, status, controller });
    return { id, generation, signal: controller.signal };
  }

  isCurrent(operation: SidecarOperation) {
    const record = this.records.get(operation.id);
    return (
      !operation.signal.aborted &&
      record?.generation === operation.generation &&
      record.controller.signal === operation.signal
    );
  }

  transition(
    operation: SidecarOperation,
    status: SidecarLifecycleStatus,
  ) {
    const record = this.records.get(operation.id);
    if (!record || !this.isCurrent(operation)) return false;
    record.status = status;
    return true;
  }

  status(id: string) {
    return this.records.get(id)?.status;
  }

  remove(operation: SidecarOperation) {
    if (!this.isCurrent(operation)) return false;
    this.records.get(operation.id)?.controller.abort();
    this.records.delete(operation.id);
    return true;
  }

  ids() {
    return [...this.records.keys()];
  }

  cancelAll() {
    for (const record of this.records.values()) {
      record.controller.abort();
    }
    this.records.clear();
  }
}

type RetryTimer = ReturnType<typeof setTimeout>;

/** One bounded retry timer per sidecar; success/close/unmount owns cancellation. */
export class SidecarStyleRetryScheduler {
  private readonly attempts = new Map<string, number>();
  private readonly timers = new Map<string, RetryTimer>();

  constructor(
    private readonly schedule: (callback: () => void, delayMs: number) => RetryTimer =
      (callback, delayMs) => setTimeout(callback, delayMs),
    private readonly cancel: (timer: RetryTimer) => void = (timer) =>
      clearTimeout(timer),
  ) {}

  retry(id: string, callback: () => void) {
    if (this.timers.has(id)) return;
    const attempt = this.attempts.get(id) ?? 0;
    const delayMs = Math.min(4_000, 250 * 2 ** attempt);
    this.attempts.set(id, attempt + 1);
    const timer = this.schedule(() => {
      this.timers.delete(id);
      callback();
    }, delayMs);
    this.timers.set(id, timer);
  }

  succeed(id: string) {
    this.remove(id);
  }

  remove(id: string) {
    const timer = this.timers.get(id);
    if (timer !== undefined) this.cancel(timer);
    this.timers.delete(id);
    this.attempts.delete(id);
  }

  cancelAll() {
    for (const timer of this.timers.values()) this.cancel(timer);
    this.timers.clear();
    this.attempts.clear();
  }

  delayFor(id: string) {
    const attempt = this.attempts.get(id) ?? 0;
    return Math.min(4_000, 250 * 2 ** Math.max(0, attempt - 1));
  }
}
