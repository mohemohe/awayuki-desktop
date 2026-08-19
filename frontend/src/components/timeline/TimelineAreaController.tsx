import React from "react";
import {
  ChevronUp,
  Loader2,
  MoreHorizontal,
  RefreshCw,
  X,
} from "lucide-react";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { PaneGroup } from "../../types/app";
import { displayTimelineName } from "../../utils/columns";
import { recordReactCommit } from "../../utils/renderMetrics";
import { Tab } from "../primitives/Tabs";
import { useTimelinePaneController } from "../../features/timeline/useTimelinePaneController";
import { useAppLocale } from "../../hooks/useAppLocale";
import {
  EmptyTimelinePaneView,
  TimelineAreaView,
  TimelinePaneView,
} from "./TimelineAreaView";
import {
  TimelineStatusList,
  TimelineTabScroller,
} from "../../features/timeline/TimelineStatusList";
import { UserProfilePane } from "../../features/timeline/UserProfilePane";

const VIRTUAL_LIST_THRESHOLD = 80;

export function TimelineAreaController({
  panes,
  activeTabs,
}: {
  panes: PaneGroup[];
  activeTabs: Record<number, string>;
}) {
  useAppLocale();
  const pendingScrollPaneIndex = useAppStore(
    (state) => state.pendingScrollPaneIndex,
  );
  const paneRenderKey = React.useMemo(
    () => panes.map((pane) => pane.paneIndex).join(":"),
    [panes],
  );

  React.useEffect(() => {
    if (pendingScrollPaneIndex === undefined) return;

    const frame = window.requestAnimationFrame(() => {
      const element = document.querySelector<HTMLElement>(
        `[data-pane-index="${pendingScrollPaneIndex}"]`,
      );
      if (!element) return;

      element.scrollIntoView({
        behavior: "smooth",
        block: "nearest",
        inline: "end",
      });
      useAppStore.getState().clearPendingPaneScroll(pendingScrollPaneIndex);
    });

    return () => window.cancelAnimationFrame(frame);
  }, [paneRenderKey, pendingScrollPaneIndex]);

  return (
    <TimelineAreaView
      onRender={recordReactCommit}
      panes={panes.map((pane) => ({
        paneIndex: pane.paneIndex,
        content: (
          <TimelinePaneController
            pane={pane}
            activeColumnId={activeTabs[pane.paneIndex]}
          />
        ),
      }))}
    />
  );
}

function TimelinePaneController({
  pane,
  activeColumnId,
}: {
  pane: PaneGroup;
  activeColumnId?: string;
}) {
  const {
    column,
    statuses,
    gaps,
    loadingTimelineGaps,
    timelineGapErrors,
    isLoading,
    isLoadingMore,
    hasMore,
    unread,
    resourceError,
    timelineRenderer,
    dynamicPane,
    contextPane: threadPane,
    scrollTopRequest,
    paneScrollerRef,
    scrollActiveTimelineToTop,
    handleNearTopChange,
    handleScrollTopComplete,
    loadTimeline,
    loadMoreTimeline,
    loadTimelineGap,
    setActiveTab,
    closeDynamicPane,
  } = useTimelinePaneController(pane, activeColumnId);

  if (!column) {
    return (
      <EmptyTimelinePaneView
        title={t("Empty Pane")}
        message={t("No timeline tabs.")}
      />
    );
  }

  return (
    <TimelinePaneView
      paneIndex={pane.paneIndex}
      header={
        <>
        <TimelineTabScroller
          updateKey={pane.tabs.map((tab) => tab.id).join(":")}
        >
          {pane.tabs.map((tab) => {
            const selected = tab.id === column.id;
            return (
              <Tab
                key={tab.id}
                selected={selected}
                className={`h-full min-w-20 max-w-36 border-r border-surface0 px-3 text-left text-sm ${selected ? "bg-base text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
                onSelect={() => setActiveTab(pane.paneIndex, tab)}
                title={displayTimelineName(tab)}
              >
                <span className="block truncate">{displayTimelineName(tab)}</span>
              </Tab>
            );
          })}
        </TimelineTabScroller>
        <div className="flex shrink-0 items-center gap-1 px-1">
          {isLoading ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-blue" />
          ) : null}
          <button
            className="btn btn-ghost btn-xs relative"
            onClick={scrollActiveTimelineToTop}
            title={t("Scroll to top")}
            aria-label={t("Scroll to top")}
          >
            <ChevronUp className="h-3.5 w-3.5" />
            {unread > 0 ? (
              <span
                className="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-blue"
                aria-hidden="true"
              />
            ) : null}
          </button>
          <button
            className="btn btn-ghost btn-xs"
            onClick={() => void loadTimeline(column, true)}
            title={t("Refresh")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </button>
          {dynamicPane ? (
            <button
              className="btn btn-ghost btn-xs"
              onClick={() => closeDynamicPane(pane.paneIndex)}
              title={t("Close pane")}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          ) : null}
          <button
            className="btn btn-ghost btn-xs"
            title={t("Timeline settings")}
            onClick={() =>
              useAppStore.setState({
                settingsOpen: true,
                selectedSettings: "Timeline",
              })
            }
          >
            <MoreHorizontal className="h-3.5 w-3.5" />
          </button>
        </div>
        </>
      }
    >
      {resourceError && statuses.length === 0 ? (
        <div
          ref={paneScrollerRef}
          className="grid min-h-0 flex-1 place-items-center overflow-y-auto p-4"
          role="alert"
        >
          <div className="max-w-sm text-center text-xs text-red">
            <p>{resourceError}</p>
            <button
              type="button"
              className="btn btn-secondary btn-xs mt-3"
              onClick={() => void loadTimeline(column, true)}
            >
              {t("Retry")}
            </button>
          </div>
        </div>
      ) : column.columnType === "profile" && column.profile ? (
        <div
          ref={paneScrollerRef}
          className="min-h-0 flex-1 overflow-hidden"
        >
          <React.Profiler
            id={`profile:open:${column.id}`}
            onRender={recordReactCommit}
          >
            <UserProfilePane
              column={column}
              scrollTopRequest={scrollTopRequest}
            />
          </React.Profiler>
        </div>
      ) : statuses.length === 0 && isLoading ? (
        <div
          ref={paneScrollerRef}
          className="grid min-h-0 flex-1 place-items-center overflow-x-hidden overflow-y-auto text-xs text-subtext0"
        >
          <span className="inline-flex items-center gap-2">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("Loading")}
          </span>
        </div>
      ) : statuses.length === 0 && !isLoading && !hasMore ? (
        <div
          ref={paneScrollerRef}
          className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
        >
          <div className="p-4 text-xs text-subtext0">
            {t("timeline.empty")}
          </div>
        </div>
      ) : (
        <TimelineStatusList
          key={column.id}
          column={column}
          statuses={statuses}
          gaps={gaps}
          loadingTimelineGaps={loadingTimelineGaps}
          timelineGapErrors={timelineGapErrors}
          virtualized={
            !threadPane &&
            (timelineRenderer === "VirtualList" ||
              statuses.length > VIRTUAL_LIST_THRESHOLD)
          }
          scrollTopRequest={scrollTopRequest}
          isLoading={isLoading}
          isLoadingMore={isLoadingMore}
          hasMore={!threadPane && gaps.length === 0 && hasMore}
          hideLoadMore={gaps.length > 0}
          threadMode={threadPane}
          onLoadMore={() => void loadMoreTimeline(column)}
          onLoadGap={(gap) => void loadTimelineGap(column, gap)}
          onNearTopChange={handleNearTopChange}
          onScrollTopComplete={handleScrollTopComplete}
        />
      )}
    </TimelinePaneView>
  );
}
