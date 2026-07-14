import { describe, expect, it, vi } from "vitest";

import type { TimelineStatus } from "../../types/app";
import { initialComposeSlice } from "../slices/compose";
import type { AppStore } from "../appStore";
import { createComposeTargetActions } from "./composeTargetActions";

describe("compose target actions", () => {
  it("reduces reply intent and schedules focus without a Tauri mock", () => {
    let state = { ...initialComposeSlice(), composeText: "draft" } as AppStore;
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
      status({ acct: "alice@example.test" }),
    );

    expect(state.composeTarget).toMatchObject({ kind: "reply" });
    expect(state.composeText).toBe("draft\nalice@example.test ");
    expect(focusComposer).toHaveBeenCalledTimes(1);
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
