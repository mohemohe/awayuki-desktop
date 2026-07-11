import { describe, expect, it } from "vitest";

import {
  IPC_COMMANDS,
  type IpcCommandName,
  IPC_UNKNOWN_ENUM_POLICY,
} from "./generated/contract";
import {
  createMockFixture,
  MOCK_IMPLEMENTED_COMMANDS,
  mockInvoke,
  resetMockFixture,
  UnsupportedMockCommandError,
} from "./mock";

describe("generated IPC contract", () => {
  it("provides operational metadata for every command", () => {
    for (const [name, metadata] of Object.entries(IPC_COMMANDS)) {
      expect(name).toMatch(/^[a-z][a-z0-9_]*$/);
      expect(metadata.timeoutMs).toBeGreaterThan(0);
      expect(metadata.argsType).not.toBe("");
      expect(metadata.resultType).not.toBe("");
      expect(["read", "mutation"]).toContain(metadata.kind);
      expect(["unsupported", "cooperative"]).toContain(metadata.cancel);
    }
  });

  it("has an explicit development adapter entry for every generated command", () => {
    expect([...MOCK_IMPLEMENTED_COMMANDS].sort()).toEqual(
      Object.keys(IPC_COMMANDS).sort(),
    );
  });

  it("creates isolated fixtures and can reset mutated adapter state", async () => {
    const first = createMockFixture();
    const second = createMockFixture();
    first.accounts[0]!.displayName = "mutated";
    expect(second.accounts[0]!.displayName).not.toBe("mutated");

    await mockInvoke("switch_active_account", {
      acct: second.accounts[1]!.acct,
    });
    resetMockFixture();
    const reset = await mockInvoke<typeof second>("app_snapshot");
    expect(reset.activeAcct).toBe(second.accounts[0]!.acct);
  });

  it("documents preservation of unknown enum values", () => {
    expect(IPC_UNKNOWN_ENUM_POLICY).toEqual({
      preserveRawValue: true,
      unknownBehavior: "render-fallback-and-disable-unsupported-action",
    });
  });

  it("returns safe diagnostics and echoes frontend support health", async () => {
    const frontend = {
      activeOperations: 1,
      completedOperations: 2,
      failedOperations: 3,
      streamSequenceGaps: 4,
      streamResyncs: 5,
      pendingStreamEvents: 6,
    };
    const diagnostics = await mockInvoke<Record<string, unknown>>(
      "diagnostics_snapshot",
    );
    const bundle = await mockInvoke<{
      frontend: typeof frontend;
      environment: { persistence: string };
      recentEvents: unknown[];
    }>("support_bundle", { request: { frontend } });

    expect(diagnostics).toMatchObject({ activeOperations: 0, apiRequests: 0 });
    expect(bundle.frontend).toEqual(frontend);
    expect(bundle.environment.persistence).toBe("sqlite_only_portable");
    expect(bundle.recentEvents).toEqual([]);
  });

  it("fails explicitly for a mock command outside the generated contract", async () => {
    await expect(
      mockInvoke("not_a_real_command" as IpcCommandName),
    ).rejects.toBeInstanceOf(UnsupportedMockCommandError);
  });
});
