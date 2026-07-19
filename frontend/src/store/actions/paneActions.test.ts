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

  it("anchors AIR context to the notification operation time", () => {
    const open = vi.fn(
      (_descriptor: DynamicPaneDescriptor, _options?: { load?: boolean }) =>
        ({ id: "opened" }) as ColumnSummary,
    );

    createPaneActions(open).openAirContextPane(
      status({
        id: "notification-1",
        originalStatusId: "target-post",
        serverDomain: "social.example",
        sourceAcct: "viewer@social.example",
        notificationAccountId: "actor-1",
        notificationAcct: "actor@social.example",
        createdAt: "2026-07-18T20:11:49Z",
        originalCreatedAt: "2026-07-18T20:11:30Z",
      }),
    );

    const descriptor = open.mock.calls[0][0];
    expect(JSON.parse(descriptor.column.columnParam ?? "{}")).toEqual({
      statusId: "target-post",
      serverDomain: "social.example",
      accountId: "actor-1",
      accountAcct: "actor@social.example",
      notificationCreatedAt: "2026-07-18T20:11:49Z",
      sourceAcct: "viewer@social.example",
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
