import { describe, expect, expectTypeOf, it } from "vitest";

import {
  IPC_COMMANDS,
  IPC_DTO_SCHEMAS,
  type IpcCommandName,
  IPC_UNKNOWN_ENUM_POLICY,
  type TypedIpcCommandArgs,
  type TypedIpcCommandResult,
} from "./generated/contract";
import {
  createMockFixture,
  MOCK_IMPLEMENTED_COMMANDS,
  mockInvoke,
  resetMockFixture,
  UnsupportedMockCommandError,
} from "./mock";
import type { TimelineStatus } from "../types/app";

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

  it("identifies KQ timelines distinctly in the development adapter", async () => {
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: {
        columnType: "kq",
        columnParam: 'where text contains "snow"',
        offset: 0,
        limit: 1,
      },
    });

    expect(statuses[0]?.id).toBe('KQ: where text contains "snow"-0');

    const page = await mockInvoke<{ statuses: TimelineStatus[]; hasMore: boolean }>(
      "load_more_timeline",
      {
        request: {
          columnType: "kq",
          columnParam: 'where text contains "snow"',
          offset: 1,
          limit: 1,
        },
      },
    );
    expect(page.statuses[0]?.id).toBe('KQ: where text contains "snow"-1');
  });

  it("documents preservation of unknown enum values", () => {
    expect(IPC_UNKNOWN_ENUM_POLICY).toEqual({
      preserveRawValue: true,
      unknownBehavior: "render-fallback-and-disable-unsupported-action",
    });
  });

  it("generates DTO fields and typed command signatures from the Rust registry", () => {
    expect(IPC_DTO_SCHEMAS.LoginInstanceRequest.fields).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          rustName: "domain",
          serializedName: "domain",
          type: "string",
          optional: false,
        }),
        expect.objectContaining({
          rustName: "operation_id",
          serializedName: "operationId",
          optional: true,
        }),
      ]),
    );
    expectTypeOf<TypedIpcCommandArgs["download_media"]>().toEqualTypeOf<{
      request: {
        operationId?: string | null;
        url: string;
        suggestedFilename?: string | null;
      };
    }>();
    expectTypeOf<TypedIpcCommandArgs["load_timeline"]>().toMatchTypeOf<{
      request: {
        columnType: string;
        accountAcct?: string | null;
        displayFilter?: unknown;
      };
    }>();
    expectTypeOf<
      TypedIpcCommandResult["load_more_timeline"]
    >().toMatchTypeOf<{ statuses: unknown[]; hasMore: boolean; gaps: unknown[] }>();
    expectTypeOf<
      TypedIpcCommandResult["load_timeline_gap"]
    >().toEqualTypeOf<TypedIpcCommandResult["load_more_timeline"]>();
    expectTypeOf<
      TypedIpcCommandResult["refresh_timeline"]
    >().toEqualTypeOf<TypedIpcCommandResult["load_more_timeline"]>();
    expectTypeOf<TypedIpcCommandArgs["account_timeline"]>().toMatchTypeOf<{
      request: {
        accountId: string;
        serverDomain: string;
        sourceAcct?: string | null;
        onlyMedia?: boolean | null;
        pinned?: boolean | null;
        cursor?: string | null;
      };
    }>();
    expectTypeOf<
      TypedIpcCommandResult["account_timeline"]
    >().toMatchTypeOf<{
      statuses: unknown[];
      hasMore: boolean;
      nextCursor?: string | null;
    }>();
    expectTypeOf<
      TypedIpcCommandResult["account_profile"]
    >().toMatchTypeOf<{ id: string; serverDomain: string }>();
    expectTypeOf<TypedIpcCommandArgs["account_profile"]>().toMatchTypeOf<{
      request: { operationId?: string | null };
    }>();
    expectTypeOf<TypedIpcCommandArgs["autocomplete_mentions"]>().toMatchTypeOf<{
      request: { operationId?: string | null };
    }>();
    expect(IPC_COMMANDS.account_profile.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.account_timeline.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.autocomplete_mentions.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.autocomplete_hashtags.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.load_timeline.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.load_more_timeline.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.load_timeline_gap.cancel).toBe("cooperative");
    expect(IPC_COMMANDS.refresh_timeline.cancel).toBe("cooperative");
    expectTypeOf<TypedIpcCommandArgs["status_action"]>().toMatchTypeOf<{
      request: {
        identity: {
          protocol: string;
          serverDomain: string;
          canonicalUri: string;
          remoteId: string;
        };
        actingAccountAcct: string;
        action: string;
      };
    }>();
    expectTypeOf<TypedIpcCommandArgs["vote_poll"]>().toMatchTypeOf<{
      request: { actingAccountAcct: string; choices: number[] };
    }>();
    expectTypeOf<TypedIpcCommandArgs["post_status"]>().toMatchTypeOf<{
      request: {
        actingAccountAcct: string;
        status: string;
        mediaIds?: string[] | null;
        poll?: { options: string[]; multiple: boolean; expiresIn: number } | null;
      };
    }>();
    expectTypeOf<
      TypedIpcCommandResult["finish_compose_media_upload"]
    >().toMatchTypeOf<{ id: string }>();
    expectTypeOf<
      TypedIpcCommandResult["login_with_instance_domain"]
    >().toEqualTypeOf<ReturnType<typeof createMockFixture>>();
    expectTypeOf<
      TypedIpcCommandResult["cancel_media_download"]
    >().toEqualTypeOf<boolean>();
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
