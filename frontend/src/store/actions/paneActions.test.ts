import { describe, expect, it, vi } from "vitest";

import type { ColumnSummary, TimelineStatus } from "../../types/app";
import type { DynamicPaneDescriptor } from "../slices/panes";
import { createPaneActions } from "./paneActions";

describe("pane actions", () => {
  it("maps thread intent to a source-preserving descriptor without Tauri", () => {
    const open = vi.fn(
      (_descriptor: DynamicPaneDescriptor, _options?: { load?: boolean }) =>
        ({ id: "opened" }) as ColumnSummary,
    );
    const actions = createPaneActions(open);

    actions.openThreadPane(
      status({
        id: "wrapper",
        originalStatusId: "remote-1",
        serverDomain: "social.example",
        sourceAcct: "alice@social.example",
      }),
    );

    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({
        resourceKey: expect.stringContaining("thread:"),
        column: expect.objectContaining({ columnType: "thread" }),
      }),
    );
    const descriptor = open.mock.calls[0][0];
    expect(JSON.parse(descriptor.column.columnParam ?? "{}")).toEqual({
      statusId: "remote-1",
      serverDomain: "social.example",
      sourceAcct: "alice@social.example",
    });
  });

  it("opens profiles without triggering the timeline loader", () => {
    const open = vi.fn(
      (_descriptor: DynamicPaneDescriptor, _options?: { load?: boolean }) =>
        ({ id: "opened" }) as ColumnSummary,
    );
    createPaneActions(open).openUserPane(
      status({
        accountId: "account-1",
        serverDomain: "social.example",
        sourceAcct: "viewer@social.example",
        acct: "author@social.example",
      }),
    );

    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({
        resourceKey: "profile:social.example:account-1:viewer@social.example",
      }),
      { load: false },
    );
  });
});

function status(overrides: Partial<TimelineStatus>): TimelineStatus {
  return {
    id: "status",
    originalStatusId: "status",
    serverDomain: "example.test",
    accountId: "account",
    acct: "author@example.test",
    displayName: "Author",
    avatar: "",
    ...overrides,
  } as TimelineStatus;
}
