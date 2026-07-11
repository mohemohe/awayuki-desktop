import { describe, expect, it } from "vitest";
import type { ColumnSummary } from "../types/app";
import { normalizeColumns } from "./columns";

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
});
