import { describe, expect, it } from "vitest";
import { createMockFixture } from "../../api/mock";
import type { TimelineStatus } from "../../types/app";
import { initialComposeSlice, reduceComposeSlice } from "./compose";

describe("compose slice reducer", () => {
  it.each([
    ["unchanged", "Hello & world\nSecond line", ""],
    ["edited", "Updated text", "Updated text"],
    ["whitespace edited", "Hello & world\nSecond line ", "Hello & world\nSecond line "],
  ])("handles %s text when an edit is cancelled", async (_, composeText, expected) => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const next = reduceComposeSlice(
      {
        composeText,
        composeTarget: {
          kind: "edit",
          status: {
            ...statuses[0]!,
            content: "<p>Hello &amp; world</p><p>Second line</p>",
          },
        },
        visibility: "private",
      },
      { type: "clearTarget" },
    );

    expect(next.composeText).toBe(expected);
    expect(next.composeTarget).toBeNull();
    expect(next.visibility).toBe("private");
  });

  it("sets a target without mutating unrelated visibility", async () => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const initial = { ...initialComposeSlice(), visibility: "private" as const };
    const next = reduceComposeSlice(initial, {
      type: "setTarget",
      target: { kind: "quote", status: statuses[0]! },
    });
    expect(next.composeTarget?.kind).toBe("quote");
    expect(next.visibility).toBe("private");
    expect(initial.composeTarget).toBeNull();
  });

  it("resets to a clean draft", () => {
    const fixture = createMockFixture();
    expect(fixture.activeAcct).toBeTruthy();
    expect(
      reduceComposeSlice(
        { composeText: "draft", composeTarget: null, visibility: "direct" },
        { type: "reset" },
      ),
    ).toEqual(initialComposeSlice());
  });

  it("restores the prior visibility after a reply is sent", async () => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const next = reduceComposeSlice(
      {
        composeText: "reply",
        composeTarget: {
          kind: "reply",
          status: statuses[0]!,
          visibilityBeforeReply: "public",
        },
        visibility: "private",
      },
      { type: "clearDraft" },
    );

    expect(next).toEqual({
      composeText: "",
      composeTarget: null,
      visibility: "public",
    });
  });

  it("restores the prior visibility when a reply is cancelled", async () => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const next = reduceComposeSlice(
      {
        composeText: "reply",
        composeTarget: {
          kind: "reply",
          status: statuses[0]!,
          visibilityBeforeReply: "unlisted",
        },
        visibility: "direct",
      },
      { type: "clearTarget" },
    );

    expect(next.visibility).toBe("unlisted");
    expect(next.composeTarget).toBeNull();
    expect(next.composeText).toBe("reply");
  });

  it("clears the untouched mention when a reply is cancelled", async () => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const status = statuses[0]!;
    const next = reduceComposeSlice(
      {
        composeText: `${status.acct.trim()} `,
        composeTarget: {
          kind: "reply",
          status,
          visibilityBeforeReply: "public",
        },
        visibility: "direct",
      },
      { type: "clearTarget" },
    );

    expect(next.composeText).toBe("");
    expect(next.composeTarget).toBeNull();
    expect(next.visibility).toBe("public");
  });

  it("keeps edited text when a reply is cancelled", async () => {
    const { mockInvoke } = await import("../../api/mock");
    const statuses = await mockInvoke<TimelineStatus[]>("load_timeline", {
      request: { columnType: "home", limit: 1 },
    });
    const status = statuses[0]!;
    const editedText = `${status.acct.trim()} hello`;
    const next = reduceComposeSlice(
      {
        composeText: editedText,
        composeTarget: {
          kind: "reply",
          status,
          visibilityBeforeReply: "public",
        },
        visibility: "direct",
      },
      { type: "clearTarget" },
    );

    expect(next.composeText).toBe(editedText);
    expect(next.composeTarget).toBeNull();
  });
});
