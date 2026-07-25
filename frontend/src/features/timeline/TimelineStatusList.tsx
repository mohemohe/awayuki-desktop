import React from "react";
import PerfectScrollbar from "perfect-scrollbar";
import {
  Virtuoso,
  type ContextProp,
  type VirtuosoHandle,
} from "react-virtuoso";
import { Loader2 } from "lucide-react";
import { t } from "../../i18n";
import type { ColumnSummary, TimelineStatus } from "../../types/app";
import { TabList } from "../../components/primitives/Tabs";
import { StatusItem } from "./TimelineStatusItem";
import {
  markNextRenderScenario,
  measureNextPaint,
} from "../../utils/renderMetrics";

const TIMELINE_TOP_TRIM_THRESHOLD_PX = 200;
const SCROLL_TOP_PURGE_FALLBACK_MS = 1800;

type TimelineVirtuosoContext = {
  scrollHeader?: React.ReactNode;
  emptyState?: React.ReactNode;
  hasMore: boolean;
  isLoadingMore: boolean;
  onLoadMore: () => void;
};

function TimelineScrollHeader({
  context,
}: ContextProp<TimelineVirtuosoContext>) {
  return <>{context.scrollHeader}</>;
}

function TimelineEmptyPlaceholder({
  context,
}: ContextProp<TimelineVirtuosoContext>) {
  return (
    <>
      {context.scrollHeader}
      {context.emptyState}
    </>
  );
}

function TimelineVirtuosoFooter({
  context,
}: ContextProp<TimelineVirtuosoContext>) {
  return (
    <TimelineLoadMoreFooter
      hasMore={context.hasMore}
      isLoadingMore={context.isLoadingMore}
      onLoadMore={context.onLoadMore}
    />
  );
}

export function TimelineTabScroller({
  children,
  updateKey,
}: {
  children: React.ReactNode;
  updateKey: string;
}) {
  const containerRef = React.useRef<HTMLDivElement | null>(null);
  const scrollbarRef = React.useRef<PerfectScrollbar | null>(null);
  const [scrollable, setScrollable] = React.useState(false);

  React.useEffect(() => {
    const element = containerRef.current;
    if (!element) return;
    const updateScrollable = () => {
      scrollbar.update();
      setScrollable(element.scrollWidth - element.clientWidth > 4);
    };

    const scrollbar = new PerfectScrollbar(element, {
      suppressScrollY: true,
      useBothWheelAxes: true,
      wheelPropagation: true,
    });
    scrollbarRef.current = scrollbar;

    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(updateScrollable);
    resizeObserver?.observe(element);
    const content = element.firstElementChild;
    if (content) resizeObserver?.observe(content);

    updateScrollable();

    return () => {
      resizeObserver?.disconnect();
      scrollbar.destroy();
      if (scrollbarRef.current === scrollbar) scrollbarRef.current = null;
    };
  }, []);

  React.useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      const element = containerRef.current;
      const scrollbar = scrollbarRef.current;
      if (!element || !scrollbar) return;
      scrollbar.update();
      setScrollable(element.scrollWidth - element.clientWidth > 4);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [updateKey]);

  return (
    <div
      ref={containerRef}
      className="timeline-tab-scroll min-w-0 flex-1"
      data-scrollable={scrollable ? "true" : "false"}
    >
      <TabList
        label={t("Timeline")}
        className="flex h-full min-w-max items-stretch"
      >
        {children}
      </TabList>
    </div>
  );
}

export function TimelineStatusList({
  column,
  statuses,
  virtualized,
  scrollTopRequest,
  isLoading,
  isLoadingMore,
  hasMore,
  threadMode = false,
  scrollHeader,
  emptyState,
  onLoadMore,
  onNearTopChange,
  onScrollTopComplete,
}: {
  column: ColumnSummary;
  statuses: TimelineStatus[];
  virtualized: boolean;
  scrollTopRequest: number;
  isLoading: boolean;
  isLoadingMore: boolean;
  hasMore: boolean;
  threadMode?: boolean;
  scrollHeader?: React.ReactNode;
  emptyState?: React.ReactNode;
  onLoadMore: () => void;
  onNearTopChange: (nearTop: boolean) => void;
  onScrollTopComplete: () => void;
}) {
  const itemKeys = React.useMemo(() => timelineItemKeys(statuses), [statuses]);
  const threadDepths = React.useMemo(
    () => (threadMode ? threadDepthMap(statuses) : new Map<string, number>()),
    [statuses, threadMode],
  );
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const virtuosoRef = React.useRef<VirtuosoHandle | null>(null);
  const pendingScrollTopRef = React.useRef(false);
  const scrollTopStartedRef = React.useRef(false);
  const scrollTopFallbackRef = React.useRef<number | null>(null);
  const onScrollTopCompleteRef = React.useRef(onScrollTopComplete);
  const canLoadMore = hasMore && !isLoading && !isLoadingMore;
  const hasScrollHeader = scrollHeader !== undefined && scrollHeader !== null;

  React.useEffect(() => {
    onScrollTopCompleteRef.current = onScrollTopComplete;
  }, [onScrollTopComplete]);

  const clearScrollTopFallback = React.useCallback(() => {
    if (scrollTopFallbackRef.current === null) return;
    window.clearTimeout(scrollTopFallbackRef.current);
    scrollTopFallbackRef.current = null;
  }, []);

  const completeScrollTop = React.useCallback(() => {
    if (!pendingScrollTopRef.current) return;
    pendingScrollTopRef.current = false;
    scrollTopStartedRef.current = false;
    clearScrollTopFallback();
    onScrollTopCompleteRef.current();
  }, [clearScrollTopFallback]);

  const handleLoadMore = React.useCallback(() => {
    if (!canLoadMore) return;
    onLoadMore();
  }, [canLoadMore, onLoadMore]);
  const virtuosoContext = React.useMemo<TimelineVirtuosoContext>(
    () => ({
      scrollHeader,
      emptyState,
      hasMore,
      isLoadingMore,
      onLoadMore: handleLoadMore,
    }),
    [emptyState, handleLoadMore, hasMore, isLoadingMore, scrollHeader],
  );

  React.useEffect(() => {
    if (scrollTopRequest === 0) return;
    pendingScrollTopRef.current = true;
    scrollTopStartedRef.current = false;
    clearScrollTopFallback();
    scrollTopFallbackRef.current = window.setTimeout(
      completeScrollTop,
      SCROLL_TOP_PURGE_FALLBACK_MS,
    );
    if (virtualized) {
      if (hasScrollHeader) {
        virtuosoRef.current?.scrollTo({ top: 0, behavior: "smooth" });
      } else {
        virtuosoRef.current?.scrollToIndex({
          index: 0,
          align: "start",
          behavior: "smooth",
        });
      }
      return;
    }
    const element = listRef.current;
    element?.scrollTo({ top: 0, behavior: "smooth" });
    if (!element || element.scrollTop <= 0) {
      window.requestAnimationFrame(completeScrollTop);
    }
  }, [
    clearScrollTopFallback,
    completeScrollTop,
    hasScrollHeader,
    scrollTopRequest,
    virtualized,
  ]);

  const handleScroll = React.useCallback(
    (event: React.UIEvent<HTMLDivElement>) => {
      const element = event.currentTarget;
      if (pendingScrollTopRef.current) {
        if (element.scrollTop <= 0) completeScrollTop();
      } else {
        markNextRenderScenario("timeline:scroll");
        measureNextPaint("timeline:scroll");
        onNearTopChange(element.scrollTop <= TIMELINE_TOP_TRIM_THRESHOLD_PX);
      }
      const distanceToBottom =
        element.scrollHeight - element.scrollTop - element.clientHeight;
      if (distanceToBottom < 600) handleLoadMore();
    },
    [completeScrollTop, handleLoadMore, onNearTopChange],
  );

  const handleVirtuosoAtTopStateChange = React.useCallback(
    (atTop: boolean) => {
      if (pendingScrollTopRef.current && atTop) return;
      markNextRenderScenario("timeline:scroll");
      measureNextPaint("timeline:scroll");
      onNearTopChange(atTop);
    },
    [onNearTopChange],
  );

  const handleVirtuosoScrolling = React.useCallback(
    (scrolling: boolean) => {
      if (!pendingScrollTopRef.current) return;
      if (scrolling) {
        scrollTopStartedRef.current = true;
        return;
      }
      if (scrollTopStartedRef.current) completeScrollTop();
    },
    [completeScrollTop],
  );

  React.useEffect(() => clearScrollTopFallback, [clearScrollTopFallback]);

  if (virtualized) {
    return (
      <Virtuoso
        ref={virtuosoRef}
        className="min-h-0 flex-1 overflow-x-hidden"
        data={statuses}
        context={virtuosoContext}
        increaseViewportBy={{ top: 800, bottom: 1200 }}
        computeItemKey={(index) => itemKeys[index]}
        endReached={handleLoadMore}
        atTopThreshold={TIMELINE_TOP_TRIM_THRESHOLD_PX}
        atTopStateChange={handleVirtuosoAtTopStateChange}
        isScrolling={handleVirtuosoScrolling}
        components={{
          Header:
            statuses.length > 0 && hasScrollHeader
              ? TimelineScrollHeader
              : undefined,
          EmptyPlaceholder:
            statuses.length === 0 &&
            (hasScrollHeader || (emptyState !== undefined && emptyState !== null))
              ? TimelineEmptyPlaceholder
              : undefined,
          Footer:
            statuses.length === 0 &&
            emptyState !== undefined &&
            emptyState !== null
              ? undefined
              : TimelineVirtuosoFooter,
        }}
        itemContent={(_, status) => (
          <StatusItem
            column={column}
            status={status}
            threadDepth={threadDepths.get(status.originalStatusId || status.id)}
          />
        )}
      />
    );
  }

  return (
    <div
      ref={listRef}
      className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
      onScroll={handleScroll}
    >
      {scrollHeader}
      {statuses.length === 0 ? emptyState : null}
      {statuses.map((status, index) => (
        <StatusItem
          key={itemKeys[index]}
          column={column}
          status={status}
          threadDepth={threadDepths.get(status.originalStatusId || status.id)}
        />
      ))}
      {threadMode || (statuses.length === 0 && emptyState) ? null : (
        <TimelineLoadMoreFooter
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          onLoadMore={handleLoadMore}
        />
      )}
    </div>
  );
}

function threadDepthMap(statuses: TimelineStatus[]) {
  const byId = new Map<string, TimelineStatus>();
  for (const status of statuses) {
    byId.set(status.originalStatusId || status.id, status);
  }

  const memo = new Map<string, number>();
  const depthFor = (status: TimelineStatus, visiting = new Set<string>()) => {
    const id = status.originalStatusId || status.id;
    const cached = memo.get(id);
    if (cached !== undefined) return cached;
    if (visiting.has(id)) return 0;

    const parentId = status.inReplyToId ?? undefined;
    const parent = parentId ? byId.get(parentId) : undefined;
    if (!parent) {
      memo.set(id, 0);
      return 0;
    }

    visiting.add(id);
    const depth = Math.min(depthFor(parent, visiting) + 1, 8);
    visiting.delete(id);
    memo.set(id, depth);
    return depth;
  };

  for (const status of statuses) depthFor(status);
  return memo;
}

function TimelineLoadMoreFooter({
  hasMore,
  isLoadingMore,
  onLoadMore,
}: {
  hasMore: boolean;
  isLoadingMore: boolean;
  onLoadMore: () => void;
}) {
  return (
    <div className="border-t border-surface0 p-3">
      {hasMore ? (
        <button
          type="button"
          className="btn btn-outline btn-sm w-full"
          onClick={onLoadMore}
          disabled={isLoadingMore}
        >
          {isLoadingMore ? (
            <>
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("Loading")}
            </>
          ) : (
            t("Load More")
          )}
        </button>
      ) : (
        <div className="py-2 text-center text-xs text-subtext0">
          {t("No more statuses.")}
        </div>
      )}
    </div>
  );
}

function timelineItemKeys(statuses: TimelineStatus[]) {
  const seen = new Map<string, number>();
  return statuses.map((status) => {
    const identity = timelineRenderIdentity(status);
    const count = seen.get(identity) ?? 0;
    seen.set(identity, count + 1);
    return count === 0 ? identity : `${identity}:duplicate:${count}`;
  });
}

function timelineRenderIdentity(status: TimelineStatus) {
  if (status.notificationId) {
    return `${status.serverDomain}:notification:${status.notificationId}`;
  }
  if (status.originalStatusId && status.originalStatusId !== status.id) {
    return `${status.serverDomain}:status:${status.id}:original:${status.originalStatusId}`;
  }
  return `${status.serverDomain}:status:${status.id}`;
}
