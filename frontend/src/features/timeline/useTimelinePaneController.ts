import React from "react";
import { timelineDescriptor } from "../../domain/timelineDescriptors";
import { useAppStore } from "../../store/appStore";
import type { PaneGroup, TimelineStatus } from "../../types/app";

const EMPTY_STATUSES: TimelineStatus[] = [];

/** Store selectors and scroll lifecycle for one pane; the view stays declarative. */
export function useTimelinePaneController(
  pane: PaneGroup,
  activeColumnId?: string,
) {
  const loadTimeline = useAppStore((state) => state.loadTimeline);
  const loadMoreTimeline = useAppStore((state) => state.loadMoreTimeline);
  const setTimelineNearTop = useAppStore((state) => state.setTimelineNearTop);
  const trimTimelineToMaxStatuses = useAppStore(
    (state) => state.trimTimelineToMaxStatuses,
  );
  const setActiveTab = useAppStore((state) => state.setActiveTab);
  const closeDynamicPane = useAppStore((state) => state.closeDynamicPane);
  const column =
    pane.tabs.find((tab) => tab.id === activeColumnId) ?? pane.tabs[0];
  const columnId = column?.id;
  const statuses = useAppStore((state) =>
    columnId ? (state.timelines[columnId] ?? EMPTY_STATUSES) : EMPTY_STATUSES,
  );
  const isLoading = useAppStore((state) =>
    columnId ? state.loading[columnId] : false,
  );
  const isLoadingMore = useAppStore((state) =>
    columnId ? state.loadingMore[columnId] : false,
  );
  const hasMore = useAppStore((state) =>
    columnId ? state.timelineHasMore[columnId] !== false : false,
  );
  const unread = useAppStore((state) =>
    columnId ? (state.timelineUnread[columnId] ?? 0) : 0,
  );
  const resourceError = useAppStore((state) =>
    columnId ? state.resourceStates[`timeline:${columnId}`]?.error : undefined,
  );
  const timelineRenderer = useAppStore(
    (state) => state.snapshot?.settings.performance.timeline_renderer ?? "List",
  );
  const [scrollTopRequest, requestScrollTop] = React.useReducer(
    (value) => value + 1,
    0,
  );
  const paneScrollerRef = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    if (scrollTopRequest === 0) return;
    paneScrollerRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  }, [scrollTopRequest]);

  const scrollActiveTimelineToTop = React.useCallback(() => {
    if (column) requestScrollTop();
  }, [column]);
  const handleNearTopChange = React.useCallback(
    (nearTop: boolean) => {
      if (column) setTimelineNearTop(column, nearTop);
    },
    [column, setTimelineNearTop],
  );
  const handleScrollTopComplete = React.useCallback(() => {
    if (column) trimTimelineToMaxStatuses(column);
  }, [column, trimTimelineToMaxStatuses]);

  const descriptor = column ? timelineDescriptor(column.columnType) : undefined;
  return {
    column,
    statuses,
    isLoading,
    isLoadingMore,
    hasMore,
    unread,
    resourceError,
    timelineRenderer,
    dynamicPane: pane.tabs.some((tab) => tab.dynamic),
    contextPane:
      descriptor?.loadStrategy === "thread" ||
      descriptor?.loadStrategy === "airContext",
    scrollTopRequest,
    paneScrollerRef,
    scrollActiveTimelineToTop,
    handleNearTopChange,
    handleScrollTopComplete,
    loadTimeline,
    loadMoreTimeline,
    setActiveTab,
    closeDynamicPane,
  };
}
