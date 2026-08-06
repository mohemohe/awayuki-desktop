import { describe, expect, it, vi } from "vitest";

import type { TimelineStatus } from "../../types/app";
import { initialComposeSlice } from "../slices/compose";
import type { AppStore } from "../appStore";
import { createComposeTargetActions } from "./composeTargetActions";

describe("compose target actions", () => {
  it("uses the replied-to visibility while preserving the prior selection", () => {
    let state = {
      ...initialComposeSlice(),
      composeText: "draft",
      visibility: "private",
    } as AppStore;
    const set = ((update: unknown) => {
      const patch =
        typeof update === "function"
          ? (update as (current: AppStore) => Partial<AppStore>)(state)
          : (update as Partial<AppStore>);
      state = { ...state, ...patch };
    }) as never;
    const focusComposer = vi.fn();
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });

    createComposeTargetActions({ set, focusComposer }).replyStatus(
      status({ acct: "alice@example.test", visibility: "unlisted" }),
    );

    expect(state.composeTarget).toMatchObject({
      kind: "reply",
      visibilityBeforeReply: "private",
    });
    expect(state.composeText).toBe("draft\nalice@example.test ");
    expect(state.visibility).toBe("unlisted");
    expect(focusComposer).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("keeps the original selection when changing reply targets", () => {
    let state = { ...initialComposeSlice(), visibility: "private" } as AppStore;
    const set = ((update: unknown) => {
      const patch =
        typeof update === "function"
          ? (update as (current: AppStore) => Partial<AppStore>)(state)
          : (update as Partial<AppStore>);
      state = { ...state, ...patch };
    }) as never;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });
    const actions = createComposeTargetActions({
      set,
      focusComposer: vi.fn(),
    });

    actions.replyStatus(status({ visibility: "unlisted" }));
    actions.replyStatus(status({ id: "2", visibility: "direct" }));

    expect(state.visibility).toBe("direct");
    expect(state.composeTarget).toMatchObject({
      kind: "reply",
      visibilityBeforeReply: "private",
    });
    vi.unstubAllGlobals();
  });

  it("skips the mention when replying to the acting account's own status", () => {
    let state = {
      ...initialComposeSlice(),
      snapshot: snapshotWithAccount({
        acct: "mohemohe@example.social",
        accountId: "42",
        serverDomain: "example.social",
      }),
    } as AppStore;
    const set = ((update: unknown) => {
      const patch =
        typeof update === "function"
          ? (update as (current: AppStore) => Partial<AppStore>)(state)
          : (update as Partial<AppStore>);
      state = { ...state, ...patch };
    }) as never;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });

    createComposeTargetActions({ set, focusComposer: vi.fn() }).replyStatus(
      status({
        acct: "@mohemohe",
        accountId: "42",
        serverDomain: "example.social",
      }),
    );

    expect(state.composeTarget).toMatchObject({ kind: "reply" });
    expect(state.composeText).toBe("");
    vi.unstubAllGlobals();
  });

  it("skips the mention for own remote statuses via acct fallback", () => {
    let state = {
      ...initialComposeSlice(),
      composeText: "draft",
      snapshot: snapshotWithAccount({
        acct: "mohemohe@example.social",
        accountId: "42",
        serverDomain: "example.social",
      }),
    } as AppStore;
    const set = ((update: unknown) => {
      const patch =
        typeof update === "function"
          ? (update as (current: AppStore) => Partial<AppStore>)(state)
          : (update as Partial<AppStore>);
      state = { ...state, ...patch };
    }) as never;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });

    createComposeTargetActions({ set, focusComposer: vi.fn() }).replyStatus(
      status({
        acct: "@mohemohe@example.social",
        accountId: "remote-9000",
        serverDomain: "other.server",
      }),
    );

    expect(state.composeText).toBe("draft");
    vi.unstubAllGlobals();
  });

  it("keeps the mention when replying to another account", () => {
    let state = {
      ...initialComposeSlice(),
      snapshot: snapshotWithAccount({
        acct: "mohemohe@example.social",
        accountId: "42",
        serverDomain: "example.social",
      }),
    } as AppStore;
    const set = ((update: unknown) => {
      const patch =
        typeof update === "function"
          ? (update as (current: AppStore) => Partial<AppStore>)(state)
          : (update as Partial<AppStore>);
      state = { ...state, ...patch };
    }) as never;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });

    createComposeTargetActions({ set, focusComposer: vi.fn() }).replyStatus(
      status({
        acct: "@alice@example.test",
        accountId: "7",
        serverDomain: "example.social",
      }),
    );

    expect(state.composeText).toBe("@alice@example.test ");
    vi.unstubAllGlobals();
  });
});

function snapshotWithAccount(account: {
  acct: string;
  accountId: string;
  serverDomain: string;
}) {
  return {
    activeAcct: account.acct,
    accounts: [account],
  } as unknown as AppStore["snapshot"];
}

function status(overrides: Partial<TimelineStatus>): TimelineStatus {
  return {
    id: "1",
    originalStatusId: "1",
    acct: "alice@example.test",
    content: "<p>hello</p>",
    visibility: "public",
    ...overrides,
  } as TimelineStatus;
}
