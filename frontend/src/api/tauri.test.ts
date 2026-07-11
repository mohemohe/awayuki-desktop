import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriInvoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauriInvoke }));
vi.mock("./mock", () => ({ mockInvoke: vi.fn() }));

import { invokeCommand, invokeReadCommand } from "./tauri";
import { IpcAppError } from "./ipcErrors";

describe("Tauri IPC retry policy", () => {
  beforeEach(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    tauriInvoke.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("does not resend a mutation when its completed response is lost", async () => {
    let completedSideEffects = 0;
    tauriInvoke.mockImplementationOnce(async () => {
      completedSideEffects += 1;
      throw new Error("IPC custom protocol failed after backend completion");
    });

    await expect(
      invokeCommand("post_status", { request: { status: "hello" } }),
    ).rejects.toMatchObject({ code: "internal" });

    expect(completedSideEffects).toBe(1);
    expect(tauriInvoke).toHaveBeenCalledTimes(1);
    expect(tauriInvoke.mock.calls[0][1].request.operationId).toMatch(
      /^[0-9a-f-]{36}$/,
    );
  });

  it("retries an explicitly classified read once after a transient error", async () => {
    vi.useFakeTimers();
    tauriInvoke
      .mockRejectedValueOnce(new Error("Load failed"))
      .mockResolvedValueOnce({ version: "test" });

    const result = invokeReadCommand("refresh_timeline", {
      request: { columnType: "home" },
    });
    await vi.advanceTimersByTimeAsync(75);

    await expect(result).resolves.toEqual({ version: "test" });
    expect(tauriInvoke).toHaveBeenCalledTimes(2);
    expect(tauriInvoke.mock.calls[0][1].request.operationId).toBe(
      tauriInvoke.mock.calls[1][1].request.operationId,
    );
  });

  it("does not retry a read after a non-transient application error", async () => {
    tauriInvoke.mockRejectedValueOnce(new Error("database is corrupt"));

    const promise = invokeReadCommand("app_snapshot");
    await expect(promise).rejects.toBeInstanceOf(IpcAppError);
    await expect(promise).rejects.toMatchObject({ code: "internal" });
    expect(tauriInvoke).toHaveBeenCalledTimes(1);
  });

  it("uses a structured code and never logs or throws the raw cause", async () => {
    tauriInvoke.mockRejectedValueOnce({
      code: "rate_limited",
      messageKey: "errors.rate_limited",
      retryable: true,
      requestId: "11111111-1111-4111-8111-111111111111",
      cause: "Authorization: Bearer secret-token",
    });

    const promise = invokeReadCommand("app_snapshot");
    await expect(promise).rejects.toMatchObject({
      code: "rate_limited",
      requestId: "11111111-1111-4111-8111-111111111111",
    });
    await expect(promise).rejects.not.toThrow(/secret-token/);
    expect(tauriInvoke).toHaveBeenCalledTimes(1);
  });
});
