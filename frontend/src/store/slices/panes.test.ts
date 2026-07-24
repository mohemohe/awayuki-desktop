import { describe, expect, it } from "vitest";
import type { ColumnSummary } from "../../types/app";
import {
  dynamicPaneResourceKey,
  reduceOpenOrFocusDynamicPane,
} from "./panes";

const persisted: ColumnSummary = {
  id: "home",
  columnType: "home",
  name: "Home",
  maxStatuses: 100,
  paneIndex: 2,
  position: 0,
};

const search = (id: string): ColumnSummary => ({
  id,
  columnType: "search",
  columnParam: "rust",
  name: "Search: rust",
  maxStatuses: 100,
  paneIndex: 3,
  position: 0,
  dynamic: true,
});

describe("pane slice reducer", () => {
  it("creates the next pane without mutating input", () => {
    const dynamicColumns: ColumnSummary[] = [];
    const result = reduceOpenOrFocusDynamicPane(
      {
        persistedColumns: [persisted],
        dynamicColumns,
        activeTabs: {},
      },
      {
        resourceKey: "search:rust",
        column: {
          id: "search-rust",
          columnType: "search",
          columnParam: "rust",
          name: "Search: rust",
          maxStatuses: 100,
          paneIndex: 0,
          position: 0,
        },
      },
    );

    expect(result.created).toBe(true);
    expect(result.column.paneIndex).toBe(3);
    expect(result.state.activeTabs).toEqual({ 3: "search-rust" });
    expect(dynamicColumns).toEqual([]);
  });

  it("focuses an existing resource and preserves the column", () => {
    const existing = search("existing");
    const result = reduceOpenOrFocusDynamicPane(
      {
        persistedColumns: [persisted],
        dynamicColumns: [existing],
        activeTabs: {},
      },
      {
        resourceKey: dynamicPaneResourceKey(existing),
        column: {
          id: "replacement",
          columnType: "search",
          columnParam: "rust",
          name: "replacement",
          maxStatuses: 100,
          paneIndex: 0,
          position: 0,
        },
      },
    );

    expect(result.created).toBe(false);
    expect(result.column).toBe(existing);
    expect(result.state.pendingScrollPaneIndex).toBe(3);
  });
});
