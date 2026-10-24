import { hasTauriRuntime, invokeTypedCommand } from "../api/tauri";
import type { ComposeMediaAttachment } from "../types/app";
import type { ComposeTarget, ComposeVisibility } from "../store/slices/compose";

/** Session-only recovery data. The native process keeps this in RAM, never disk. */
export type ComposeRecoveryCheckpoint = {
  version: 1;
  accountAcct: string;
  composeText: string;
  composeTarget: ComposeTarget | null;
  visibility: ComposeVisibility;
  cwEnabled: boolean;
  spoilerText: string;
  pollEnabled: boolean;
  pollOptions: string[];
  pollMultiple: boolean;
  pollExpiresIn: number;
  pluginVisibilityOverride: ComposeVisibility | null;
  pluginSensitiveOverride: boolean | null;
  liveCommentaryEnabled: boolean;
  attachments: ComposeMediaAttachment[];
};

type RecoveryWrite = {
  checkpoint: ComposeRecoveryCheckpoint | null;
  recoveryBlocked: boolean;
  generation: string;
};

/** Serialize writes and coalesce edits while IPC is busy; never build a queue. */
export class FrontendRecoveryCoordinator {
  private checkpoint: ComposeRecoveryCheckpoint | null = null;
  private readonly blockers = new Set<string>();
  private ready = false;
  private generation: string | undefined;
  private restorePromise: Promise<void> | undefined;
  private pending: RecoveryWrite | undefined;
  private writePromise: Promise<void> | undefined;
  private lastCompletedSignature: string | undefined;

  constructor(
    private readonly read: () => Promise<unknown>,
    private readonly write: (request: RecoveryWrite) => Promise<unknown>,
  ) {}

  async restore(accountAcct: string | null) {
    this.restorePromise ??= this.read().then((value) => {
      if (!value || typeof value !== "object" ||
        !("generation" in value) || typeof value.generation !== "string" ||
        !value.generation || !("checkpoint" in value)) {
        throw new Error("Invalid frontend recovery handshake");
      }
      this.generation = value.generation;
      this.checkpoint = parseComposeRecoveryCheckpoint(value.checkpoint);
      this.ready = true;
    });
    // A failed restore leaves recovery disabled in the native process. Never
    // replace the only surviving draft with an empty initial React render.
    await this.restorePromise;
    if (this.checkpoint?.accountAcct !== accountAcct) this.checkpoint = null;
    return this.current(accountAcct);
  }

  current(accountAcct: string | null) {
    return this.checkpoint?.accountAcct === accountAcct ? this.checkpoint : null;
  }

  save(checkpoint: ComposeRecoveryCheckpoint, blocked: boolean) {
    if (!this.ready) return;
    try {
      JSON.stringify(checkpoint);
    } catch {
      // A cyclic remote quote must not make IPC serialization fail before the
      // native process can disable recovery. Keep the last valid checkpoint.
      void this.setBlocked("invalid-checkpoint", true).catch(() => undefined);
      return;
    }
    this.checkpoint = checkpoint;
    this.blockers.delete("invalid-checkpoint");
    void this.setBlocked("compose", blocked).catch(() => undefined);
  }

  setBlocked(reason: string, blocked: boolean) {
    if (blocked) this.blockers.add(reason);
    else this.blockers.delete(reason);
    if (!this.ready || !this.generation) return Promise.resolve();
    this.pending = {
      checkpoint: this.checkpoint,
      recoveryBlocked: this.blockers.size > 0,
      generation: this.generation,
    };
    return this.flush();
  }

  private flush(): Promise<void> {
    if (this.writePromise) return this.writePromise;
    const write = async () => {
      while (this.pending) {
        const request = this.pending;
        this.pending = undefined;
        const signature = JSON.stringify(request);
        if (signature === this.lastCompletedSignature) continue;
        await this.write(request);
        this.lastCompletedSignature = signature;
      }
    };
    this.writePromise = write().finally(() => {
      this.writePromise = undefined;
      // Deduplication can finish write() without an await. A blocker queued
      // before this finalizer then shares the finishing promise, so hand off
      // to a new drain and keep that acknowledgement pending until it completes.
      if (this.pending) return this.flush();
    });
    return this.writePromise;
  }
}

const recovery = new FrontendRecoveryCoordinator(
  () => hasTauriRuntime()
    ? invokeTypedCommand("restore_frontend_recovery")
    : Promise.resolve({ checkpoint: null, generation: "browser-preview" }),
  (request) => hasTauriRuntime()
    ? invokeTypedCommand("checkpoint_frontend_recovery", { request })
    : Promise.resolve(),
);

export const restoreFrontendRecovery = (accountAcct: string | null) =>
  recovery.restore(accountAcct);
export const currentComposeRecovery = (accountAcct: string | null) =>
  recovery.current(accountAcct);
export const saveComposeRecovery = (
  checkpoint: ComposeRecoveryCheckpoint,
  blocked: boolean,
) => recovery.save(checkpoint, blocked);
export const blockFrontendRecovery = (reason: string, blocked: boolean) =>
  recovery.setBlocked(reason, blocked);

export function clearComposeRecovery(accountAcct: string, composeText = "") {
  const checkpoint = recovery.current(accountAcct);
  if (!checkpoint) return;
  recovery.save({
    ...checkpoint,
    composeText,
    composeTarget: null,
    cwEnabled: false,
    spoilerText: "",
    pollEnabled: false,
    pollOptions: ["", ""],
    pollMultiple: false,
    pollExpiresIn: 24 * 60 * 60,
    pluginVisibilityOverride: null,
    pluginSensitiveOverride: null,
    attachments: [],
  }, false);
}

export function attachmentSurvivesReload(attachment: ComposeMediaAttachment) {
  // A blob URL belongs to the failed WebContent process. Completed uploads
  // without a remote preview must also stay protected until removed/submitted.
  return !attachment.uploading && /^https?:\/\//i.test(attachment.previewSrc);
}

function parseComposeRecoveryCheckpoint(value: unknown): ComposeRecoveryCheckpoint | null {
  if (!value || typeof value !== "object") return null;
  const draft = value as Partial<ComposeRecoveryCheckpoint>;
  const visibility = (item: unknown) =>
    typeof item === "string" && ["public", "unlisted", "private", "direct"].includes(item);
  if (
    draft.version !== 1 || typeof draft.accountAcct !== "string" ||
    typeof draft.composeText !== "string" || !visibility(draft.visibility) ||
    typeof draft.cwEnabled !== "boolean" || typeof draft.spoilerText !== "string" ||
    typeof draft.pollEnabled !== "boolean" || !Array.isArray(draft.pollOptions) ||
    !draft.pollOptions.every((option) => typeof option === "string") ||
    typeof draft.pollMultiple !== "boolean" || !Number.isFinite(draft.pollExpiresIn) ||
    !(draft.pluginVisibilityOverride === null || visibility(draft.pluginVisibilityOverride)) ||
    !(draft.pluginSensitiveOverride === null || typeof draft.pluginSensitiveOverride === "boolean") ||
    typeof draft.liveCommentaryEnabled !== "boolean" || !Array.isArray(draft.attachments) ||
    !draft.attachments.every((item) => item && typeof item.id === "string" &&
      typeof item.filename === "string" && typeof item.previewSrc === "string" &&
      attachmentSurvivesReload(item)) ||
    !(draft.composeTarget === null || (draft.composeTarget &&
      ["reply", "quote", "edit"].includes(draft.composeTarget.kind) &&
      draft.composeTarget.status && typeof draft.composeTarget.status.id === "string" &&
      typeof draft.composeTarget.status.serverDomain === "string"))
  ) return null;
  return draft as ComposeRecoveryCheckpoint;
}
