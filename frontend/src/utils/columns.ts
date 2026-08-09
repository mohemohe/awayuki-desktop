import type {
  ColumnSummary,
  PaneGroup,
  TimelineDisplayFilter,
} from "../types/app";
import { t } from "../i18n";
import {
  configurableTimelineTypes,
  timelineDescriptor,
  timelineTypeRequiresAccount,
} from "../domain/timelineDescriptors";

/** @deprecated Import the descriptor registry for new behavior. */
export const timelineTypes = configurableTimelineTypes;

export function groupColumnsByPane(columns: ColumnSummary[]): PaneGroup[] {
  const map = new Map<number, ColumnSummary[]>();
  for (const column of columns) {
    const group = map.get(column.paneIndex) ?? [];
    group.push({
      ...column,
      maxStatuses: Math.max(1, Number(column.maxStatuses) || 100),
    });
    map.set(column.paneIndex, group);
  }
  return [...map.entries()]
    .sort(([a], [b]) => a - b)
    .map(([paneIndex, tabs]) => ({
      paneIndex,
      desktopNotifications: tabs[0]?.desktopNotifications ?? true,
      notificationSound: tabs[0]?.notificationSound ?? null,
      tabs: tabs.sort((a, b) => a.position - b.position),
    }));
}

export function normalizeColumns(columns: ColumnSummary[]): ColumnSummary[] {
  const sorted = [...columns].sort(
    (a, b) =>
      a.paneIndex - b.paneIndex ||
      a.position - b.position ||
      a.name.localeCompare(b.name),
  );
  const paneIndexMap = new Map<number, number>();
  const positions = new Map<number, number>();
  return sorted.map((column) => {
    if (!paneIndexMap.has(column.paneIndex))
      paneIndexMap.set(column.paneIndex, paneIndexMap.size);
    const paneIndex = paneIndexMap.get(column.paneIndex)!;
    const position = positions.get(paneIndex) ?? 0;
    positions.set(paneIndex, position + 1);
    return {
      ...column,
      paneIndex,
      position,
      maxStatuses: Math.max(1, Number(column.maxStatuses) || 100),
      accountAcct: timelineTypeRequiresAccount(column.columnType)
        ? column.accountAcct
        : null,
      displayFilter: normalizeDisplayFilter(column.displayFilter),
    };
  });
}

export function flattenPanes(panes: PaneGroup[]): ColumnSummary[] {
  return panes.flatMap((pane, paneIndex) =>
    pane.tabs.map((tab, position) => ({
      ...tab,
      paneIndex,
      position,
      desktopNotifications: pane.desktopNotifications,
      notificationSound: pane.notificationSound ?? null,
      maxStatuses: Math.max(1, Number(tab.maxStatuses) || 100),
    })),
  );
}

export function reconcileActiveTabs(
  columns: ColumnSummary[],
  current: Record<number, string>,
): Record<number, string> {
  const next: Record<number, string> = {};
  for (const pane of groupColumnsByPane(columns)) {
    const activeId = pane.tabs.some((tab) => tab.id === current[pane.paneIndex])
      ? current[pane.paneIndex]
      : pane.tabs[0]?.id;
    if (activeId) next[pane.paneIndex] = activeId;
  }
  return next;
}

export function createColumn(
  paneIndex: number,
  position: number,
  columnType = "home",
): ColumnSummary {
  return {
    id:
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random()}`,
    columnType,
    columnParam: null,
    name: defaultTimelineName(columnType),
    maxStatuses: 100,
    paneIndex,
    position,
    displayFilter: defaultDisplayFilter(),
    desktopNotifications: true,
    notificationSound: null,
  };
}

export function defaultDisplayFilter(): TimelineDisplayFilter {
  return {
    enabled: false,
    excludeBoosts: false,
    excludeMedia: false,
    includeMedia: false,
  };
}

export function normalizeDisplayFilter(
  filter?: TimelineDisplayFilter | null,
): TimelineDisplayFilter {
  return {
    ...defaultDisplayFilter(),
    ...(filter ?? {}),
  };
}

export function timelineTypeSupportsDisplayFilter(columnType: string) {
  return timelineDescriptor(columnType)?.supportsDisplayFilter ?? false;
}

export function timelineDisplayFilterApplies(column: ColumnSummary) {
  const filter = normalizeDisplayFilter(column.displayFilter);
  return (
    timelineTypeSupportsDisplayFilter(column.columnType) &&
    filter.enabled &&
    (filter.excludeBoosts || filter.excludeMedia || filter.includeMedia)
  );
}

export function defaultTimelineName(columnType: string) {
  return timelineDescriptor(columnType)?.defaultName ?? columnType;
}

export function displayTimelineName(column: ColumnSummary) {
  const descriptor = timelineDescriptor(column.columnType);
  if (!descriptor) {
    return column.name || t("timeline.unknown", { type: column.columnType });
  }
  const defaultName = descriptor.defaultName;
  if (column.name === defaultName) return t(descriptor.labelId);
  if (column.name.startsWith("Search: ")) {
    return `${t("timeline.search")}: ${column.name.slice("Search: ".length)}`;
  }
  return column.name;
}
