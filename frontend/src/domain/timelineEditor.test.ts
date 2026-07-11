import { describe, expect, it } from "vitest";
import {
  createTimelineEditorState,
  reduceTimelineEditor,
} from "./timelineEditor";
import type { ColumnSummary } from "../types/app";

describe("timeline editor reducer", () => {
  it("updates panes and selection atomically", () => {
    const initial = createTimelineEditorState([
      { paneIndex: 0, tabs: [column("home", 0, 0)] },
    ]);
    const addedPane = reduceTimelineEditor(initial, { type: "addPane" });
    expect(addedPane.selectedPane).toBe(1);
    expect(addedPane.selectedTabId).toBe(addedPane.panes[1].tabs[0].id);

    const addedTab = reduceTimelineEditor(addedPane, { type: "addTab" });
    expect(addedTab.panes[1].tabs).toHaveLength(2);
    expect(addedTab.selectedTabId).toBe(addedTab.panes[1].tabs[1].id);

    const removed = reduceTimelineEditor(addedTab, { type: "removePane" });
    expect(removed).toMatchObject({
      selectedPane: 0,
      selectedTabId: "home",
    });
    expect(removed.panes[0].paneIndex).toBe(0);
  });

  it("normalizes pane and tab positions after moves", () => {
    const initial = createTimelineEditorState([
      {
        paneIndex: 0,
        tabs: [column("a", 0, 0), column("b", 0, 1)],
      },
      { paneIndex: 1, tabs: [column("c", 1, 0)] },
    ]);
    const movedTab = reduceTimelineEditor(initial, {
      type: "moveTab",
      from: 0,
      to: 1,
    });
    expect(movedTab.panes[0].tabs.map((tab) => [tab.id, tab.position])).toEqual([
      ["b", 0],
      ["a", 1],
    ]);

    const movedPane = reduceTimelineEditor(movedTab, {
      type: "movePane",
      from: 0,
      to: 1,
    });
    expect(movedPane.panes.map((pane) => pane.paneIndex)).toEqual([0, 1]);
    expect(movedPane.panes[1].tabs.every((tab) => tab.paneIndex === 1)).toBe(true);
  });
});

function column(id: string, paneIndex: number, position: number): ColumnSummary {
  return {
    id,
    paneIndex,
    position,
    columnType: "home",
    name: "Home",
    maxStatuses: 100,
  };
}
