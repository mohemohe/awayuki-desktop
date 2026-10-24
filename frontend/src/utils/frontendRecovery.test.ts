import { describe, expect, it, vi } from "vitest";
import {
  attachmentSurvivesReload,
  FrontendRecoveryCoordinator,
  type ComposeRecoveryCheckpoint,
} from "./frontendRecovery";

function draft(overrides: Partial<ComposeRecoveryCheckpoint> = {}): ComposeRecoveryCheckpoint {
  return {
    version: 1,
    accountAcct: "alice@example.social",
    composeText: "Unsent private draft",
    composeTarget: null,
    visibility: "private",
    cwEnabled: true,
    spoilerText: "CW draft",
    pollEnabled: true,
    pollOptions: ["One", "Two"],
    pollMultiple: true,
    pollExpiresIn: 3600,
    pluginVisibilityOverride: "direct",
    pluginSensitiveOverride: true,
    liveCommentaryEnabled: false,
    attachments: [{
      id: "uploaded-123", filename: "image.png", previewSrc: "https://example.social/image.png",
    }],
    ...overrides,
  };
}

const restored = (checkpoint: unknown) => ({ checkpoint, generation: "document-1" });

describe("frontend recovery checkpoint", () => {
  it("restores the same-account text, CW, poll and uploaded media without submitting", async () => {
    const checkpoint = draft();
    const write = vi.fn().mockResolvedValue(undefined);
    const coordinator = new FrontendRecoveryCoordinator(async () => restored(checkpoint), write);
    expect(await coordinator.restore(checkpoint.accountAcct)).toEqual(checkpoint);
    expect(write).not.toHaveBeenCalled();
    expect(await coordinator.restore("bob@other.social")).toBeNull();
    expect(coordinator.current(checkpoint.accountAcct)).toBeNull();
  });

  it("never overwrites a surviving draft before restore succeeds", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    const coordinator = new FrontendRecoveryCoordinator(async () => { throw new Error("unavailable"); }, write);
    coordinator.save(draft({ composeText: "" }), false);
    await expect(coordinator.restore("alice@example.social")).rejects.toThrow("unavailable");
    coordinator.save(draft({ composeText: "" }), false);
    expect(write).not.toHaveBeenCalled();
  });

  it("uses its document generation and rejects acknowledgement after native recovery invalidates it", async () => {
    let nativeGeneration = "before-reload";
    const accepted: unknown[] = [];
    const write = vi.fn(async (request: { generation: string }) => {
      if (request.generation !== nativeGeneration) throw new Error("stale document");
      accepted.push(request);
    });
    const read = async () => ({ checkpoint: null, generation: nativeGeneration });
    const previousDocument = new FrontendRecoveryCoordinator(read, write);
    await previousDocument.restore("alice@example.social");
    previousDocument.save(draft(), false);
    await previousDocument.setBlocked("noop", false);
    nativeGeneration = "after-reload";
    const recoveredDocument = new FrontendRecoveryCoordinator(read, write);
    await recoveredDocument.restore("alice@example.social");
    await recoveredDocument.setBlocked("submit", true);
    await expect(previousDocument.setBlocked("submit", true)).rejects.toThrow("stale document");
    expect(accepted).toHaveLength(2);
    expect(accepted[1]).toMatchObject({ generation: "after-reload", recoveryBlocked: true });
  });

  it("keeps one outstanding IPC and coalesces later edits while acknowledging blockers", async () => {
    const completions: Array<() => void> = [];
    const write = vi.fn((_request: unknown) => new Promise<void>((resolve) => completions.push(resolve)));
    const coordinator = new FrontendRecoveryCoordinator(async () => restored(null), write);
    await coordinator.restore("alice@example.social");
    coordinator.save(draft({ composeText: "first" }), false);
    for (let index = 0; index < 1000; index += 1) {
      coordinator.save(draft({ composeText: `edit-${index}` }), false);
    }
    const protectedWrite = coordinator.setBlocked("submit", true);
    const acknowledged = vi.fn();
    void protectedWrite.then(acknowledged);
    expect(write).toHaveBeenCalledTimes(1);
    completions.shift()!();
    await Promise.resolve();
    expect(write).toHaveBeenCalledTimes(2);
    expect(write.mock.calls[1][0]).toMatchObject({
      checkpoint: { composeText: "edit-999" }, recoveryBlocked: true,
    });
    expect(acknowledged).not.toHaveBeenCalled();
    completions.shift()!();
    await protectedWrite;
    expect(acknowledged).toHaveBeenCalledOnce();
  });

  it("keeps settings protected when compose work becomes safe", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    const coordinator = new FrontendRecoveryCoordinator(async () => restored(null), write);
    await coordinator.restore("alice@example.social");
    await coordinator.setBlocked("settings", true);
    coordinator.save(draft(), false);
    await coordinator.setBlocked("submit", false);
    expect(write).toHaveBeenLastCalledWith({ ...restored(draft()), recoveryBlocked: true });
    await coordinator.setBlocked("settings", false);
    expect(write).toHaveBeenLastCalledWith({ ...restored(draft()), recoveryBlocked: false });
  });

  it("acknowledges a blocker queued immediately after a duplicate checkpoint is skipped", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    const coordinator = new FrontendRecoveryCoordinator(async () => restored(null), write);
    await coordinator.restore("alice@example.social");
    coordinator.save(draft(), false);
    await coordinator.setBlocked("noop", false);
    expect(write).toHaveBeenCalledTimes(1);

    // The duplicate drains synchronously, but its promise's finally callback
    // has not run when submit protection is queued in this same JavaScript task.
    let acceptBlock!: () => void;
    write.mockImplementationOnce(() => new Promise<void>((resolve) => { acceptBlock = resolve; }));
    coordinator.save(draft(), false);
    const protection = coordinator.setBlocked("submit", true);
    const acknowledged = vi.fn();
    void protection.then(acknowledged);
    await Promise.resolve();
    expect(write).toHaveBeenCalledTimes(2);
    expect(write).toHaveBeenLastCalledWith({ ...restored(draft()), recoveryBlocked: true });
    expect(acknowledged).not.toHaveBeenCalled();
    acceptBlock();
    await protection;
    expect(acknowledged).toHaveBeenCalledOnce();
  });

  it("rejects process-local blob previews and incomplete uploads", async () => {
    const attachment = draft().attachments[0];
    expect(attachmentSurvivesReload(attachment)).toBe(true);
    expect(attachmentSurvivesReload({ ...attachment, previewSrc: "blob:lost" })).toBe(false);
    expect(attachmentSurvivesReload({ ...attachment, uploading: true })).toBe(false);
    const coordinator = new FrontendRecoveryCoordinator(async () => restored(draft({
      attachments: [{ ...attachment, previewSrc: "blob:lost" }],
    })), vi.fn());
    expect(await coordinator.restore("alice@example.social")).toBeNull();
  });
});
