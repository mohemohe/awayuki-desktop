import { describe, expect, it } from "vitest";
import type { ColumnSummary } from "../types/app";
import { flattenPanes, groupColumnsByPane, normalizeColumns } from "./columns";

function column(
  columnType: string,
  accountAcct: string | null,
): ColumnSummary {
  return {
    id: columnType,
    columnType,
    columnParam: null,
    name: columnType,
    maxStatuses: 100,
    paneIndex: 0,
    position: 0,
    accountAcct,
  };
}

describe("column persistence normalization", () => {
  it("removes stale account bindings from unified and SQLite-global columns", () => {
    for (const columnType of [
      "home",
      "public",
      "notification",
      "bookmarks",
      "favourites",
      "custom",
      "yq",
      "kq",
      "search",
      "thread",
    ]) {
      const [normalized] = normalizeColumns([
        column(columnType, "stale@example.test"),
      ]);
      expect(normalized.accountAcct, columnType).toBeNull();
    }
  });

  it("retains the explicit source of account-bound columns", () => {
    for (const columnType of ["local", "hashtag", "list"]) {
      const [normalized] = normalizeColumns([
        column(columnType, "source@example.test"),
      ]);
      expect(normalized.accountAcct, columnType).toBe("source@example.test");
    }
  });

  it("defaults legacy panes to notifications on and persists pane sound overrides", () => {
    const legacy = column("notification", null);
    const [pane] = groupColumnsByPane([legacy]);
    expect(pane).toMatchObject({
      desktopNotifications: true,
      notificationSound: null,
    });

    const [saved] = flattenPanes([
      {
        ...pane,
        desktopNotifications: false,
        notificationSound: "Message",
      },
    ]);
    expect(saved).toMatchObject({
      desktopNotifications: false,
      notificationSound: "Message",
    });
  });
});
