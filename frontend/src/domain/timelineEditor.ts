import type { ColumnSummary, PaneGroup } from "../types/app";
import { createColumn, defaultTimelineName } from "../utils/columns";

export type TimelineEditorState = {
  panes: PaneGroup[];
  selectedPane: number;
  selectedTabId: string | null;
};

export type TimelineEditorAction =
  | { type: "reset"; panes: PaneGroup[] }
  | { type: "selectPane"; index: number }
  | { type: "selectTab"; id: string }
  | { type: "addPane" }
  | { type: "removePane" }
  | { type: "addTab" }
  | { type: "removeTab" }
  | { type: "movePane"; from: number; to: number }
  | { type: "moveTab"; from: number; to: number }
  | {
      type: "updatePane";
      patch: Partial<Pick<PaneGroup, "desktopNotifications" | "notificationSound">>;
    }
  | { type: "updateTab"; patch: Partial<ColumnSummary> };

export function createTimelineEditorState(
  panes: PaneGroup[],
): TimelineEditorState {
  return {
    panes,
    selectedPane: 0,
    selectedTabId: panes[0]?.tabs[0]?.id ?? null,
  };
}

/** One pure transition owns panes, active pane, and active tab. */
export function reduceTimelineEditor(
  state: TimelineEditorState,
  action: TimelineEditorAction,
): TimelineEditorState {
  if (action.type === "reset") return createTimelineEditorState(action.panes);
  if (action.type === "selectPane") {
    return {
      ...state,
      selectedPane: action.index,
      selectedTabId: state.panes[action.index]?.tabs[0]?.id ?? null,
    };
  }
  if (action.type === "selectTab") {
    return { ...state, selectedTabId: action.id };
  }
  if (action.type === "addPane") {
    const paneIndex = state.panes.length;
    const tab = createColumn(paneIndex, 0);
    return {
      panes: [
        ...state.panes,
        {
          paneIndex,
          tabs: [tab],
          desktopNotifications: true,
          notificationSound: null,
        },
      ],
      selectedPane: paneIndex,
      selectedTabId: tab.id,
    };
  }
  if (action.type === "removePane") {
    if (!state.panes[state.selectedPane]) return state;
    const panes = state.panes
      .filter((_, index) => index !== state.selectedPane)
      .map((pane, paneIndex) => ({
        ...pane,
        paneIndex,
        tabs: pane.tabs.map((tab) => ({ ...tab, paneIndex })),
      }));
    const selectedPane = Math.max(
      0,
      Math.min(state.selectedPane, panes.length - 1),
    );
    return {
      panes,
      selectedPane,
      selectedTabId: panes[selectedPane]?.tabs[0]?.id ?? null,
    };
  }
  if (action.type === "addTab") {
    const pane = state.panes[state.selectedPane];
    if (!pane) return state;
    const tab = createColumn(state.selectedPane, pane.tabs.length);
    return {
      ...state,
      panes: state.panes.map((item, index) =>
        index === state.selectedPane
          ? { ...item, tabs: [...item.tabs, tab] }
          : item,
      ),
      selectedTabId: tab.id,
    };
  }
  if (action.type === "removeTab") {
    const pane = state.panes[state.selectedPane];
    if (!pane || !state.selectedTabId) return state;
    const tabs = pane.tabs
      .filter((tab) => tab.id !== state.selectedTabId)
      .map((tab, position) => ({ ...tab, position }));
    return {
      ...state,
      panes: state.panes.map((item, index) =>
        index === state.selectedPane ? { ...item, tabs } : item,
      ),
      selectedTabId: tabs[0]?.id ?? null,
    };
  }
  if (action.type === "movePane") {
    if (
      action.from === action.to ||
      !state.panes[action.from] ||
      !state.panes[action.to]
    ) {
      return state;
    }
    const panes = [...state.panes];
    const [pane] = panes.splice(action.from, 1);
    panes.splice(action.to, 0, pane);
    const normalized = panes.map((item, paneIndex) => ({
      ...item,
      paneIndex,
      tabs: item.tabs.map((tab) => ({ ...tab, paneIndex })),
    }));
    return {
      panes: normalized,
      selectedPane: action.to,
      selectedTabId: normalized[action.to]?.tabs[0]?.id ?? null,
    };
  }
  if (action.type === "moveTab") {
    const pane = state.panes[state.selectedPane];
    if (
      action.from === action.to ||
      !pane?.tabs[action.from] ||
      !pane.tabs[action.to]
    ) {
      return state;
    }
    const tabs = [...pane.tabs];
    const [tab] = tabs.splice(action.from, 1);
    tabs.splice(action.to, 0, tab);
    const normalizedTabs = tabs.map((item, position) => ({
      ...item,
      position,
    }));
    return {
      ...state,
      panes: state.panes.map((item, index) =>
        index === state.selectedPane
          ? { ...item, tabs: normalizedTabs }
          : item,
      ),
      selectedTabId: tab.id,
    };
  }

  if (action.type === "updatePane") {
    return {
      ...state,
      panes: state.panes.map((pane, index) =>
        index === state.selectedPane ? { ...pane, ...action.patch } : pane,
      ),
    };
  }

  const pane = state.panes[state.selectedPane];
  if (!pane || !state.selectedTabId) return state;
  return {
    ...state,
    panes: state.panes.map((item, index) => {
      if (index !== state.selectedPane) return item;
      return {
        ...item,
        tabs: item.tabs.map((tab) =>
          tab.id === state.selectedTabId
            ? patchTimelineTab(tab, action.patch)
            : tab,
        ),
      };
    }),
  };
}

function patchTimelineTab(
  tab: ColumnSummary,
  patch: Partial<ColumnSummary>,
): ColumnSummary {
  const hasColumnParamPatch = Object.prototype.hasOwnProperty.call(
    patch,
    "columnParam",
  );
  const nextType = patch.columnType ?? tab.columnType;
  const nextName =
    patch.columnType && tab.name === defaultTimelineName(tab.columnType)
      ? defaultTimelineName(nextType)
      : tab.name;
  return {
    ...tab,
    name: nextName,
    ...patch,
    columnParam:
      patch.columnType &&
      !["hashtag", "list", "feed", "custom", "search", "yq", "kq"].includes(
        nextType,
      )
        ? null
        : hasColumnParamPatch
          ? (patch.columnParam ?? null)
          : tab.columnParam,
  };
}
