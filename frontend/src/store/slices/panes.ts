import type { ColumnSummary } from "../../types/app";

export type PaneSliceState = {
  persistedColumns: ColumnSummary[];
  dynamicColumns: ColumnSummary[];
  activeTabs: Record<number, string>;
  pendingScrollPaneIndex?: number;
};

export type DynamicPaneDescriptor = {
  /** Stable domain identity. It must not contain translated display text. */
  resourceKey: string;
  /** paneIndex/position/dynamic are normalized by the reducer. */
  column: ColumnSummary;
  updateExisting?: (
    current: ColumnSummary,
  ) => Pick<ColumnSummary, "name" | "profile">;
};

export type OpenDynamicPaneResult = {
  state: Pick<
    PaneSliceState,
    "dynamicColumns" | "activeTabs" | "pendingScrollPaneIndex"
  >;
  column: ColumnSummary;
  created: boolean;
};

export function dynamicPaneResourceKey(column: ColumnSummary): string {
  if (column.columnType === "profile") {
    return [
      "profile",
      column.profile?.serverDomain ?? "",
      column.profile?.accountId ?? "",
    ].join(":");
  }
  return [column.columnType, column.columnParam ?? ""].join(":");
}

/**
 * Pure transition shared by every dynamic-pane entry point. Loading and
 * scrolling are deliberately left to the store use-case action.
 */
export function reduceOpenOrFocusDynamicPane(
  state: PaneSliceState,
  descriptor: DynamicPaneDescriptor,
): OpenDynamicPaneResult {
  const existing = state.dynamicColumns.find(
    (column) => dynamicPaneResourceKey(column) === descriptor.resourceKey,
  );
  if (existing) {
    const patch = descriptor.updateExisting?.(existing);
    const column = patch ? { ...existing, ...patch } : existing;
    return {
      column,
      created: false,
      state: {
        dynamicColumns: patch
          ? state.dynamicColumns.map((candidate) =>
              candidate.id === existing.id ? column : candidate,
            )
          : state.dynamicColumns,
        activeTabs: {
          ...state.activeTabs,
          [existing.paneIndex]: existing.id,
        },
        pendingScrollPaneIndex: existing.paneIndex,
      },
    };
  }

  const nextPaneIndex = [
    ...state.persistedColumns,
    ...state.dynamicColumns,
  ].reduce(
    (maximum, column) => Math.max(maximum, column.paneIndex),
    -1,
  ) + 1;
  const column: ColumnSummary = {
    ...descriptor.column,
    paneIndex: nextPaneIndex,
    position: 0,
    dynamic: true,
  };
  return {
    column,
    created: true,
    state: {
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: {
        ...state.activeTabs,
        [nextPaneIndex]: column.id,
      },
      pendingScrollPaneIndex: nextPaneIndex,
    },
  };
}
