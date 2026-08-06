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
});

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
