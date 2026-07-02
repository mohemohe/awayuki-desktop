import React from "react";
import PerfectScrollbar from "perfect-scrollbar";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import {
  Bookmark,
  ChevronUp,
  Eye,
  EyeOff,
  Languages,
  Loader2,
  MessageCircle,
  MoreHorizontal,
  Quote,
  RefreshCw,
  Repeat2,
  Star,
  X,
} from "lucide-react";
import { invokeCommand } from "../../api/tauri";
import { accountSourceColorHex } from "../../constants/accountSourceColors";
import { statusIdentity, useAppStore } from "../../store/appStore";
import type {
  AccountProfileSummary,
  AccountRelationshipSummary,
  AppearanceSettings,
  ColumnSummary,
  ConfirmationSettings,
  PaneGroup,
  PollSummary,
  TimelineStatus,
} from "../../types/app";
import {
  copyToClipboard,
  getClientPlatform,
  openExternalUrl,
} from "../../utils/browser";
import { displayTimelineName } from "../../utils/columns";
import {
  formatCompactNumber,
  formatTime,
  htmlToPlainText,
  statusPlainText,
  statusUrl,
} from "../../utils/format";
import { blurHashToDataUrl } from "../../utils/blurhash";
import { thumbnailMediaSources, uniqueMediaSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";
import { confirmFollowAction } from "../../utils/confirmation";
import { Avatar } from "../common/Avatar";
import {
  CustomEmojiText,
  StatusHtmlWithCustomEmojis,
} from "../common/CustomEmoji";
import { PostMenuPopover } from "../common/PostMenuPopover";
import { VisibilityIcon } from "../common/VisibilityIcon";
import { appLocale, t } from "../../i18n";

const EMPTY_STATUSES: TimelineStatus[] = [];
const VIRTUAL_LIST_THRESHOLD = 80;
const TIMELINE_TOP_TRIM_THRESHOLD_PX = 200;
const SCROLL_TOP_PURGE_FALLBACK_MS = 1800;
const translationCache = new Map<string, CachedTranslation>();

type TranslationState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "translated"; text: string; sourceLanguage?: string | null }
  | { kind: "error"; message: string };

type CachedTranslation = {
  text: string;
  sourceLanguage?: string | null;
};

type TranslateStatusResponse = {
  text: string;
  sourceLanguage?: string | null;
  targetLanguage: string;
};

type TranslationEngine = ConfirmationSettings["translation_engine"];

function elapsedUiMs(startedAt: number) {
  return (performance.now() - startedAt).toFixed(1);
}

function targetTranslationLanguage() {
  return appLocale === "ja" ? "ja" : "en";
}

function shouldOfferTranslation(status: TimelineStatus, plainText: string) {
  if (!plainText.trim()) return false;
  const language = status.language?.trim().toLowerCase();
  if (!language) return true;
  if (appLocale === "ja") return !language.startsWith("ja");
  return !language.startsWith("en");
}

function translationCacheKey(
  status: TimelineStatus,
  targetLanguage: string,
  translationEngine: TranslationEngine,
) {
  return `${statusIdentity(status)}:${targetLanguage}:${translationEngine}:${hashString(status.content)}`;
}

function hashString(value: string) {
  let hash = 0;
  for (let index = 0; index < value.length; index += 1) {
    hash = (hash * 31 + value.charCodeAt(index)) | 0;
  }
  return hash.toString(36);
}

function languageDisplayName(language?: string | null) {
  const value = language?.trim();
  if (!value) return t("Unknown language");
  try {
    return new Intl.DisplayNames([navigator.language], {
      type: "language",
    }).of(value) ?? value;
  } catch {
    return value;
  }
}

function translatedTextToHtml(text: string) {
  return text
    .trim()
    .split(/\n{2,}/)
    .map(
      (paragraph) =>
        `<p>${escapeHtml(paragraph).replace(/\n/g, "<br>")}</p>`,
    )
    .join("");
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function TimelineArea({
  panes,
  activeTabs,
}: {
  panes: PaneGroup[];
  activeTabs: Record<number, string>;
}) {
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
    <div className="min-h-0 flex-1 overflow-x-auto bg-base-200">
      <div className="flex h-full min-w-full">
        {panes.map((pane) => (
          <TimelinePane
            key={pane.paneIndex}
            pane={pane}
            activeColumnId={activeTabs[pane.paneIndex]}
          />
        ))}
      </div>
    </div>
  );
}

function TimelinePane({
  pane,
  activeColumnId,
}: {
  pane: PaneGroup;
  activeColumnId?: string;
}) {
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
  const timelineRenderer = useAppStore(
    (state) => state.snapshot?.settings.performance.timeline_renderer ?? "List",
  );
  const dynamicPane = pane.tabs.some((tab) => tab.dynamic);
  const threadPane =
    column.columnType === "thread" || column.columnType === "airContext";
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
    if (!column) return;
    requestScrollTop();
  }, [column]);
  const handleNearTopChange = React.useCallback(
    (nearTop: boolean) => {
      if (!column) return;
      setTimelineNearTop(column, nearTop);
    },
    [column, setTimelineNearTop],
  );
  const handleScrollTopComplete = React.useCallback(() => {
    if (!column) return;
    trimTimelineToMaxStatuses(column);
  }, [column, trimTimelineToMaxStatuses]);

  if (!column) {
    return (
      <section className="flex h-full min-w-[360px] flex-1 flex-col border-r border-surface0 bg-base">
        <div className="flex h-8 shrink-0 items-center border-b border-surface0 bg-base-300 px-2 text-xs text-subtext0">
          {t("Empty Pane")}
        </div>
        <div className="grid flex-1 place-items-center text-xs text-subtext0">
          {t("No timeline tabs.")}
        </div>
      </section>
    );
  }

  return (
    <section
      className="flex h-full min-w-[360px] flex-1 flex-col border-r border-surface0 bg-base"
      data-pane-index={pane.paneIndex}
    >
      <div className="flex h-8 shrink-0 items-stretch border-b border-surface0 bg-base-300">
        <TimelineTabScroller
          updateKey={pane.tabs.map((tab) => tab.id).join(":")}
        >
          {pane.tabs.map((tab) => {
            const selected = tab.id === column.id;
            return (
              <button
                key={tab.id}
                className={`h-full min-w-20 max-w-36 border-r border-surface0 px-3 text-left text-sm ${selected ? "bg-base text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
                onClick={() => setActiveTab(pane.paneIndex, tab)}
                title={displayTimelineName(tab)}
              >
                <span className="block truncate">{displayTimelineName(tab)}</span>
              </button>
            );
          })}
        </TimelineTabScroller>
        <div className="flex shrink-0 items-center gap-1 px-1">
          {isLoading ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-blue" />
          ) : null}
          <button
            className="btn btn-ghost btn-xs"
            onClick={scrollActiveTimelineToTop}
            title={t("Scroll to top")}
            aria-label={t("Scroll to top")}
          >
            <ChevronUp className="h-3.5 w-3.5" />
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
      </div>
      {column.columnType === "profile" && column.profile ? (
        <div
          ref={paneScrollerRef}
          className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
        >
          <UserProfilePane column={column} />
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
            {t("No statuses loaded.")}
          </div>
        </div>
      ) : (
        <TimelineStatusList
          key={column.id}
          column={column}
          statuses={statuses}
          virtualized={
            !threadPane &&
            (timelineRenderer === "VirtualList" ||
              statuses.length > VIRTUAL_LIST_THRESHOLD)
          }
          scrollTopRequest={scrollTopRequest}
          isLoading={isLoading}
          isLoadingMore={isLoadingMore}
          hasMore={!threadPane && hasMore}
          threadMode={threadPane}
          onLoadMore={() => void loadMoreTimeline(column)}
          onNearTopChange={handleNearTopChange}
          onScrollTopComplete={handleScrollTopComplete}
        />
      )}
    </section>
  );
}

function TimelineTabScroller({
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
      <div className="flex h-full min-w-max items-stretch">{children}</div>
    </div>
  );
}

function TimelineStatusList({
  column,
  statuses,
  virtualized,
  scrollTopRequest,
  isLoading,
  isLoadingMore,
  hasMore,
  threadMode = false,
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
      virtuosoRef.current?.scrollToIndex({
        index: 0,
        align: "start",
        behavior: "smooth",
      });
      return;
    }
    const element = listRef.current;
    element?.scrollTo({ top: 0, behavior: "smooth" });
    if (!element || element.scrollTop <= 0) {
      window.requestAnimationFrame(completeScrollTop);
    }
  }, [clearScrollTopFallback, completeScrollTop, scrollTopRequest, virtualized]);

  const handleScroll = React.useCallback(
    (event: React.UIEvent<HTMLDivElement>) => {
      const element = event.currentTarget;
      if (pendingScrollTopRef.current) {
        if (element.scrollTop <= 0) completeScrollTop();
      } else {
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
        increaseViewportBy={{ top: 800, bottom: 1200 }}
        computeItemKey={(index) => itemKeys[index]}
        endReached={handleLoadMore}
        atTopThreshold={TIMELINE_TOP_TRIM_THRESHOLD_PX}
        atTopStateChange={handleVirtuosoAtTopStateChange}
        isScrolling={handleVirtuosoScrolling}
        components={{
          Footer: () => (
            <TimelineLoadMoreFooter
              hasMore={hasMore}
              isLoadingMore={isLoadingMore}
              onLoadMore={handleLoadMore}
            />
          ),
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
      {statuses.map((status, index) => (
        <StatusItem
          key={itemKeys[index]}
          column={column}
          status={status}
          threadDepth={threadDepths.get(status.originalStatusId || status.id)}
        />
      ))}
      {threadMode ? null : (
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

function UserProfilePane({ column }: { column: ColumnSummary }) {
  const confirmationSettings = useAppStore(
    (state) => state.snapshot?.settings.confirmation,
  );
  const requestConfirmation = useAppStore((state) => state.requestConfirmation);
  const openUserBookmarksPane = useAppStore(
    (state) => state.openUserBookmarksPane,
  );
  const target = column.profile!;
  const [profile, setProfile] = React.useState<AccountProfileSummary | null>(
    null,
  );
  const [posts, setPosts] = React.useState<TimelineStatus[]>([]);
  const [pinnedPosts, setPinnedPosts] = React.useState<TimelineStatus[]>([]);
  const [mediaPosts, setMediaPosts] = React.useState<TimelineStatus[]>([]);
  const [tab, setTab] = React.useState<"posts" | "media">("posts");
  const [loading, setLoading] = React.useState(true);
  const [menuPosition, setMenuPosition] = React.useState<{
    top: number;
    right: number;
  } | null>(null);

  React.useEffect(() => {
    let disposed = false;
    setLoading(true);
    const paneStartedAt = performance.now();
    const paneContext = `column=${column.id} account=${target.accountId} server=${target.serverDomain}`;
    console.info(`[awayuki][ui-profile-pane] start ${paneContext}`);
    const profileRequest = {
      accountId: target.accountId,
      serverDomain: target.serverDomain,
    };
    const timelineRequest = (
      pinned: boolean,
      onlyMedia: boolean,
      limit: number,
    ) => ({
      accountId: target.accountId,
      serverDomain: target.serverDomain,
      pinned,
      onlyMedia,
      limit,
      offset: 0,
    });

    const loadTimelineSlice = async (
      slice: "pinned" | "posts" | "media",
      pinned: boolean,
      onlyMedia: boolean,
      limit: number,
    ) => {
      const startedAt = performance.now();
      console.info(
        `[awayuki][ui-profile-pane] account_timeline_start ${paneContext} slice=${slice} pinned=${pinned} only_media=${onlyMedia} limit=${limit}`,
      );
      try {
        const statuses = await invokeCommand<TimelineStatus[]>(
          "account_timeline",
          {
            request: timelineRequest(pinned, onlyMedia, limit),
          },
        );
        console.info(
          `[awayuki][ui-profile-pane] account_timeline_success ${paneContext} slice=${slice} count=${statuses.length} duration_ms=${elapsedUiMs(startedAt)} total_ms=${elapsedUiMs(paneStartedAt)}`,
        );
        return statuses;
      } catch (error) {
        console.info(
          `[awayuki][ui-profile-pane] account_timeline_error ${paneContext} slice=${slice} duration_ms=${elapsedUiMs(startedAt)} total_ms=${elapsedUiMs(paneStartedAt)} error=${String(error)}`,
        );
        throw error;
      }
    };

    void (async () => {
      try {
        const profileStartedAt = performance.now();
        console.info(
          `[awayuki][ui-profile-pane] account_profile_start ${paneContext}`,
        );
        const profile = await invokeCommand<AccountProfileSummary>(
          "account_profile",
          { request: profileRequest },
        );
        if (disposed) return;
        setProfile(profile);
        console.info(
          `[awayuki][ui-profile-pane] account_profile_success ${paneContext} duration_ms=${elapsedUiMs(profileStartedAt)} total_ms=${elapsedUiMs(paneStartedAt)}`,
        );
      } catch (error) {
        if (disposed) return;
        console.info(
          `[awayuki][ui-profile-pane] account_profile_error ${paneContext} duration_ms=${elapsedUiMs(paneStartedAt)} error=${String(error)}`,
        );
        setLoading(false);
        useAppStore.setState({ error: String(error) });
        return;
      }

      const [pinned, posts, media] = await Promise.allSettled([
        loadTimelineSlice("pinned", true, false, 40),
        loadTimelineSlice("posts", false, false, 80),
        loadTimelineSlice("media", false, true, 80),
      ]);
      if (disposed) return;

      if (pinned.status === "fulfilled") setPinnedPosts(pinned.value);
      if (posts.status === "fulfilled") setPosts(posts.value);
      if (media.status === "fulfilled") setMediaPosts(media.value);

      const timelineError = [pinned, posts, media].find(
        (result) => result.status === "rejected",
      );
      if (timelineError?.status === "rejected") {
        useAppStore.setState({ error: String(timelineError.reason) });
      }

      setLoading(false);
      console.info(
        `[awayuki][ui-profile-pane] complete ${paneContext} pinned=${pinned.status === "fulfilled" ? pinned.value.length : "error"} posts=${posts.status === "fulfilled" ? posts.value.length : "error"} media=${media.status === "fulfilled" ? media.value.length : "error"} duration_ms=${elapsedUiMs(paneStartedAt)}`,
      );
    })();
    return () => {
      disposed = true;
      console.info(
        `[awayuki][ui-profile-pane] dispose ${paneContext} duration_ms=${elapsedUiMs(paneStartedAt)}`,
      );
    };
  }, [target.accountId, target.serverDomain]);

  const headerSources = React.useMemo(
    () => uniqueMediaSources([profile?.header]),
    [profile?.header],
  );
  const headerImage = useRetriedMediaSource(headerSources);

  const followAction = async () => {
    if (!profile || profile.isSelf) return;
    const action = profile.relationship?.following ? "unfollow" : "follow";
    try {
      const confirmed = await confirmFollowAction(
        confirmationSettings,
        requestConfirmation,
        profile,
        action,
      );
      if (!confirmed) return;
      const relationship = await invokeCommand<AccountRelationshipSummary>(
        "account_follow_action",
        {
          request: {
            accountId: profile.id,
            serverDomain: profile.serverDomain,
            action,
          },
        },
      );
      setProfile({ ...profile, relationship });
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    }
  };
  const setDesktopNotificationMuted = async (muted: boolean) => {
    if (!profile) return;
    try {
      const notificationMuted = await invokeCommand<boolean>(
        "set_account_notification_mute",
        {
          request: {
            accountId: profile.id,
            serverDomain: profile.serverDomain,
            muted,
          },
        },
      );
      setProfile({ ...profile, notificationMuted });
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    }
  };
  const accountModerationAction = async (
    action: "mute" | "unmute" | "block" | "unblock",
  ) => {
    if (!profile || profile.isSelf) return;
    try {
      const relationship = await invokeCommand<AccountRelationshipSummary>(
        "account_follow_action",
        {
          request: {
            accountId: profile.id,
            serverDomain: profile.serverDomain,
            action,
          },
        },
      );
      setProfile({ ...profile, relationship });
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    }
  };
  const openProfileUrl = () => {
    if (!profile?.url) return;
    void openExternalUrl(profile.url).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const toggleMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenuPosition((current) =>
      current
        ? null
        : {
            top: Math.max(
              8,
              Math.min(rect.bottom + 4, window.innerHeight - 196),
            ),
            right: Math.max(8, window.innerWidth - rect.right),
          },
    );
  };

  if (loading && !profile) {
    return (
      <div className="grid h-full place-items-center text-sm text-subtext0">
        <Loader2 className="h-4 w-4 animate-spin" />
      </div>
    );
  }

  const displayName = profile?.displayName || target.displayName || target.acct;
  const acct = profile?.acct || target.acct;
  const accountEmojis = profile?.accountEmojis ?? [];
  const visiblePosts = tab === "media" ? mediaPosts : posts;
  const acctLabel = `@${acct.replace(/^@/, "")}`;

  return (
    <div className="min-h-full bg-base text-sm">
      <div className="relative z-0 h-32 overflow-hidden border-b border-surface0 bg-base-200">
        {headerImage.src && !headerImage.failed ? (
          <img
            key={headerImage.key}
            src={headerImage.src}
            alt=""
            className={`h-full w-full object-cover ${headerImage.loaded ? "" : "opacity-0"}`}
            onLoad={headerImage.onLoad}
            onError={headerImage.onError}
          />
        ) : null}
        {headerImage.retrying ? (
          <div className="absolute inset-0 grid place-items-center bg-base-200/70 text-overlay0">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : null}
      </div>
      <div className="relative z-10 border-b border-surface0 bg-base px-3 py-3">
        <div className="relative z-20 mb-1 flex min-w-0 items-center justify-end gap-2 pl-20">
          <div className="rounded absolute left-0">
            <Avatar
              sources={[profile?.avatar, target.avatar]}
              label={displayName}
              size="xxl"
            />
          </div>
          {profile?.relationship?.followedBy ||
          (profile && !profile.isSelf) ? (
            <div className="flex min-w-0 shrink-0 flex-col items-end gap-1">
              {profile?.relationship?.followedBy ? (
                <span className="badge badge-sm border-blue bg-blue text-black">
                  {t("Follows you")}
                </span>
              ) : null}
              {profile && !profile.isSelf ? (
                <span
                  className={`badge badge-sm text-black ${profile.notificationMuted ? "border-yellow bg-yellow" : "border-transparent bg-blue"}`}
                  title={
                    profile.notificationMuted
                      ? t("Desktop notifications disabled")
                      : t("Desktop notifications enabled")
                  }
                >
                  {profile.notificationMuted ? t("Notify off") : t("Notify on")}
                </span>
              ) : null}
            </div>
          ) : null}
          <button
            className={`btn btn-sm shrink-0 ${profile?.relationship?.following ? "border-red bg-red text-white hover:border-red hover:bg-red hover:text-white" : "btn-secondary"}`}
            disabled={profile?.isSelf}
            onClick={() => void followAction()}
          >
            {profile?.isSelf
              ? t("This is you")
              : profile?.relationship?.following
                ? t("Unfollow")
                : profile?.relationship?.requested
                  ? t("Requested")
                  : t("Follow")}
          </button>
          <button
            className="btn btn-ghost btn-sm shrink-0 px-2"
            onClick={toggleMenu}
            title={t("More")}
            disabled={!profile}
            data-post-menu-trigger
          >
            <MoreHorizontal className="h-4 w-4" />
          </button>
          {menuPosition && profile ? (
            <PostMenuPopover
              position={menuPosition}
              onClose={() => setMenuPosition(null)}
              widthClassName="w-60"
              items={[
                {
                  label: t("Open in browser"),
                  action: openProfileUrl,
                  disabled: !profile.url,
                },
                {
                  label: t("Search this user's bookmarks"),
                  action: () =>
                    openUserBookmarksPane({
                      accountId: profile.id,
                      serverDomain: profile.serverDomain,
                      acct: profile.acct,
                      displayName: profile.displayName,
                      avatar: profile.avatar,
                    }),
                },
                {
                  label: profile.notificationMuted
                    ? t("Enable desktop notifications")
                    : t("Disable desktop notifications"),
                  action: () =>
                    void setDesktopNotificationMuted(
                      !profile.notificationMuted,
                    ),
                  disabled: profile.isSelf,
                },
                {
                  label: profile.relationship?.muting
                    ? t("Unmute {acct}", { acct: acctLabel })
                    : t("Mute {acct}", { acct: acctLabel }),
                  action: () =>
                    void accountModerationAction(
                      profile.relationship?.muting ? "unmute" : "mute",
                    ),
                  disabled: profile.isSelf,
                },
                {
                  label: profile.relationship?.blocking
                    ? t("Unblock {acct}", { acct: acctLabel })
                    : t("Block {acct}", { acct: acctLabel }),
                  action: () =>
                    void accountModerationAction(
                      profile.relationship?.blocking ? "unblock" : "block",
                    ),
                  disabled: profile.isSelf,
                },
              ]}
            />
          ) : null}
        </div>
        <div className="user-name mt-3 font-semibold text-text text-xl">
          <CustomEmojiText text={displayName} emojis={accountEmojis} />
        </div>
        <div className="user-acct text-sm text-subtext0">
          @{acct.replace(/^@/, "")}
        </div>
        {profile?.note ? (
          <StatusHtmlWithCustomEmojis
            className="status-content mt-3 text-sm leading-6"
            html={profile.note}
            emojis={accountEmojis}
          />
        ) : null}
        {profile?.fields.length ? (
          <div className="mt-3 overflow-hidden rounded border border-surface0">
            {profile.fields.map((field) => (
              <div
                key={field.name}
                className="border-b border-surface0 px-3 py-2 last:border-b-0"
              >
                <div className="text-xs text-subtext0">{field.name}</div>
                <StatusHtmlWithCustomEmojis
                  className="status-content text-sm"
                  html={field.value}
                  emojis={accountEmojis}
                />
              </div>
            ))}
          </div>
        ) : null}
        {profile ? (
          <div className="mt-3 flex gap-5 text-sm text-subtext0">
            <span>
              <b className="text-text">
                {formatCompactNumber(profile.statusesCount)}
              </b>{" "}
              {t("Posts")}
            </span>
            <span>
              <b className="text-text">
                {formatCompactNumber(profile.followingCount)}
              </b>{" "}
              {t("Following")}
            </span>
            <span>
              <b className="text-text">
                {formatCompactNumber(profile.followersCount)}
              </b>{" "}
              {t("Followers")}
            </span>
          </div>
        ) : null}
      </div>
      <div className="grid grid-cols-2 border-b border-surface0">
        <button
          className={`h-10 border-b-2 text-sm ${tab === "posts" ? "border-blue text-text" : "border-transparent text-subtext0"}`}
          onClick={() => setTab("posts")}
        >
          {t("Posts")}
        </button>
        <button
          className={`h-10 border-b-2 text-sm ${tab === "media" ? "border-blue text-text" : "border-transparent text-subtext0"}`}
          onClick={() => setTab("media")}
        >
          {t("Media")}
        </button>
      </div>
      {tab === "posts" && pinnedPosts.length ? (
        <>
          <div className="border-b border-surface0 px-3 py-2 text-xs text-subtext0">
            {t("Pinned posts")}
          </div>
          {pinnedPosts.map((status) => (
            <StatusItem
              key={`pinned-${status.id}`}
              column={column}
              status={status}
            />
          ))}
          <div className="border-b border-surface0 px-3 py-2 text-xs text-subtext0">
            {t("Posts")}
          </div>
        </>
      ) : tab === "posts" ? (
        <div className="border-b border-surface0 px-3 py-2 text-xs text-subtext0">
          {t("Posts")}
        </div>
      ) : null}
      {visiblePosts.length ? (
        visiblePosts.map((status) => (
          <StatusItem
            key={`${tab}-${status.id}`}
            column={column}
            status={status}
          />
        ))
      ) : (
        <div className="p-4 text-xs text-subtext0">
          {t("No statuses loaded.")}
        </div>
      )}
    </div>
  );
}

function StatusItem({
  column,
  status,
  threadDepth = 0,
}: {
  column: ColumnSummary;
  status: TimelineStatus;
  threadDepth?: number;
}) {
  const action = useAppStore((state) => state.action);
  const votePoll = useAppStore((state) => state.votePoll);
  const deleteStatus = useAppStore((state) => state.deleteStatus);
  const replyStatus = useAppStore((state) => state.replyStatus);
  const quoteStatus = useAppStore((state) => state.quoteStatus);
  const beginEditStatus = useAppStore((state) => state.beginEditStatus);
  const openThreadPane = useAppStore((state) => state.openThreadPane);
  const openAirContextPane = useAppStore(
    (state) => state.openAirContextPane,
  );
  const openUserPane = useAppStore((state) => state.openUserPane);
  const openMediaPreview = useAppStore((state) => state.openMediaPreview);
  const requestConfirmation = useAppStore((state) => state.requestConfirmation);
  const activeAccount = useAppStore((state) => {
    const accounts = state.snapshot?.accounts ?? [];
    return (
      accounts.find((account) => account.acct === state.snapshot?.activeAcct) ??
      accounts.find((account) => account.isActive)
    );
  });
  const nsfwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.nsfw_behavior ?? "Hide",
  );
  const fontSize = useAppStore(
    (state) => state.snapshot?.settings.appearance.font_size ?? "Medium",
  );
  const cwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.cw_behavior ?? "Hide",
  );
  const displayMode = useAppStore(
    (state) => state.snapshot?.settings.appearance.display_mode ?? "StarryEyes",
  );
  const showStatusApplication = useAppStore(
    (state) =>
      state.snapshot?.settings.confirmation.show_status_application ?? false,
  );
  const sourceBorderColor = useAppStore((state) =>
    accountSourceColorHex(
      status.sourceAcct
        ? state.snapshot?.settings.accountSourceColors[status.sourceAcct]
        : undefined,
    ),
  );
  const mediaSourcePreference = useAppStore(
    (state) => state.snapshot?.settings.confirmation.media_source ?? "Local",
  );
  const [mystiqueExpanded, setMystiqueExpanded] = React.useState(false);
  const [mediaVisibility, setMediaVisibility] = React.useState<
    Record<string, boolean>
  >({});
  const [menuPosition, setMenuPosition] = React.useState<{
    top: number;
    right: number;
  } | null>(null);
  const url = statusUrl(status);
  const fontSizeClass = statusFontSizeClass(fontSize);
  const statusApplicationLabel = showStatusApplication
    ? status.applicationName?.trim()
    : undefined;
  const isMystique = displayMode === "Mystique";
  const isCompact = isMystique && !mystiqueExpanded;
  const threadIndent = threadDepth > 0 ? 12 + threadDepth * 18 : undefined;
  const threadLineLeft = threadDepth > 0 ? 5 + threadDepth * 18 : undefined;
  const statusStyle = statusItemStyle(threadIndent, sourceBorderColor);
  const openThread = (event: React.MouseEvent<HTMLElement>) => {
    event.stopPropagation();
    openThreadPane(status);
  };
  React.useEffect(() => {
    setMystiqueExpanded(false);
  }, [displayMode, status.id, status.notificationId]);
  const canManageStatus =
    activeAccount?.accountId === status.accountId &&
    activeAccount.serverDomain === status.serverDomain;
  const copyText = () =>
    void copyToClipboard(statusPlainText(status)).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  const copyUrl = () => {
    if (!url) return;
    void copyToClipboard(url).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const openUrl = () => {
    if (!url) return;
    void openExternalUrl(url).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const editPost = () => {
    beginEditStatus(status);
  };
  const deletePost = async () => {
    const confirmed = await requestConfirmation({
      title: t("Delete post"),
      message: t("Delete this post? This cannot be undone."),
      confirmLabel: t("Delete"),
      danger: true,
    });
    if (!confirmed) return;
    await deleteStatus(status);
  };
  const toggleMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenuPosition((current) =>
      current
        ? null
        : {
            top: Math.min(rect.bottom + 4, window.innerHeight - 132),
            right: Math.max(8, window.innerWidth - rect.right),
          },
    );
  };
  const toggleMystiqueExpanded = (event: React.MouseEvent<HTMLElement>) => {
    if (!isMystique) return;
    const target = event.target as HTMLElement;
    if (
      target.closest(
        "a, button, details, input, label, menu, select, summary, textarea",
      )
    ) {
      return;
    }
    setMystiqueExpanded((current) => !current);
  };

  if (isCompact) {
    return (
      <article
        className={`${fontSizeClass} relative grid min-h-6 max-w-full cursor-pointer grid-cols-[28px_minmax(0,1fr)_auto] items-center gap-2 overflow-hidden border-b border-l-2 border-surface0 border-l-transparent px-1.5 py-0.5 hover:bg-surface0/40`}
        style={statusStyle}
        onClick={() => setMystiqueExpanded(true)}
        title={t("Expand post")}
      >
        {threadLineLeft ? (
          <span
            className="pointer-events-none absolute bottom-0 top-0 w-px bg-surface1"
            style={{ left: threadLineLeft }}
          />
        ) : null}
        <Avatar src={status.avatar} label={status.displayName} size="md" />
        <div className="flex min-w-0 items-center gap-2">
          <span className="shrink-0 truncate font-semibold">
            <CustomEmojiText
              text={status.displayName || status.acct}
              emojis={status.accountEmojis}
            />
          </span>
          <span className="min-w-0 truncate font-extralight text-subtext0">
            {statusPlainText(status)}
          </span>
        </div>
        <button
          type="button"
          className="inline-flex min-w-0 shrink-0 items-center gap-1 text-xs text-overlay0 hover:text-blue"
          onClick={openThread}
          title={t("Open thread")}
        >
          {formatTime(statusDisplayCreatedAt(status))}
          {statusApplicationLabel ? (
            <span className="hidden max-w-28 truncate sm:inline">
              from {statusApplicationLabel}
            </span>
          ) : null}
        </button>
      </article>
    );
  }

  return (
    <article
      className={`${fontSizeClass} relative max-w-full overflow-x-hidden border-b border-l-2 border-surface0 border-l-transparent px-3 py-3 hover:bg-surface0/40 ${isMystique ? "cursor-pointer" : ""}`}
      style={statusStyle}
      onClick={toggleMystiqueExpanded}
      title={isMystique ? t("Collapse post") : undefined}
    >
      {threadLineLeft ? (
        <span
          className="pointer-events-none absolute bottom-0 top-0 w-px bg-surface1"
          style={{ left: threadLineLeft }}
        />
      ) : null}
      <NotificationMeta status={status} onOpenUser={openUserPane} />
      <div className="flex max-w-full gap-3">
        <Avatar src={status.avatar} label={status.displayName} size="post" />
        <div className="min-w-0 max-w-full flex-1 overflow-x-hidden">
          <div className="flex min-w-0 items-baseline gap-1">
            <button
              className="truncate text-left font-semibold hover:text-blue"
              onClick={() => openUserPane(status)}
              title={t("Open profile")}
            >
              <CustomEmojiText
                text={status.displayName || status.acct}
                emojis={status.accountEmojis}
              />
            </button>
            <button
              className="truncate text-left text-xs text-subtext0 hover:text-blue"
              onClick={() => openUserPane(status)}
              title={t("Open profile")}
            >
              {status.acct}
            </button>
            <button
              type="button"
              className="ml-auto inline-flex shrink-0 items-center gap-1 text-xs text-overlay0 hover:text-blue"
              onClick={openThread}
              title={t("Open thread")}
            >
              <VisibilityIcon visibility={status.visibility} />
              {formatTime(statusDisplayCreatedAt(status))}
              {statusApplicationLabel ? (
                <span className="max-w-36 truncate text-overlay0">
                  from {statusApplicationLabel}
                </span>
              ) : null}
            </button>
          </div>
          <StatusContentBlock
            status={status}
            cwBehavior={cwBehavior}
            className="mt-1 max-w-full font-extralight"
          />
          {status.quote ? (
            <QuotePreview
              status={status.quote}
              onOpenUser={openUserPane}
              onOpenStatus={(quote) =>
                void openExternalUrl(statusUrl(quote)).catch((error) =>
                  useAppStore.setState({ error: String(error) }),
                )
              }
            />
          ) : status.quoteOriginalUrl ? (
            <QuoteLinkPreview url={status.quoteOriginalUrl} />
          ) : null}
          {status.media.length ? (
            <div className="mt-2 grid grid-cols-2 gap-1">
              {status.media.slice(0, 4).map((media) => {
                const sources = thumbnailMediaSources(
                  media,
                  mediaSourcePreference,
                );
                return sources.length ? (
                  <MediaThumbnail
                    key={media.id}
                    media={media}
                    sources={sources}
                    sensitive={status.sensitive}
                    visible={
                      mediaVisibility[media.id] ?? nsfwBehavior === "AlwaysShow"
                    }
                    onToggle={() =>
                      setMediaVisibility((current) => ({
                        ...current,
                        [media.id]: !(
                          current[media.id] ?? nsfwBehavior === "AlwaysShow"
                        ),
                      }))
                    }
                    onOpen={() => openMediaPreview(status, media)}
                  />
                ) : null;
              })}
            </div>
          ) : null}
          {status.poll ? (
            <StatusPoll
              status={status}
              onVote={(choices) => votePoll(status, choices)}
            />
          ) : null}
          <div className="mt-2.5 flex items-center gap-4 text-xs text-overlay0">
            <button
              className="inline-flex h-5 items-center gap-1 text-overlay0 hover:text-subtext0"
              title={t("Reply")}
              onClick={() => replyStatus(status)}
            >
              <MessageCircle className="h-3.5 w-3.5" />
              {status.repliesCount || ""}
            </button>
            <button
              className="inline-flex h-5 items-center gap-1 text-overlay0 hover:text-subtext0"
              title={t("Quote")}
              onClick={() => quoteStatus(status)}
            >
              <Quote className="h-3.5 w-3.5" />
            </button>
            <button
              className={`inline-flex h-5 items-center gap-1 ${status.reblogged ? "text-green hover:text-green" : "text-overlay0 hover:text-subtext0"}`}
              title={t("Boost")}
              onClick={() =>
                void action(
                  column,
                  status,
                  status.reblogged ? "unreblog" : "reblog",
                )
              }
            >
              <Repeat2 className="h-3.5 w-3.5" />
              {status.reblogsCount || ""}
            </button>
            <button
              className={`inline-flex h-5 items-center gap-1 ${status.favourited ? "text-yellow hover:text-yellow" : "text-overlay0 hover:text-subtext0"}`}
              title={t("Favorite")}
              onClick={() =>
                void action(
                  column,
                  status,
                  status.favourited ? "unfavourite" : "favourite",
                )
              }
            >
              <Star className="h-3.5 w-3.5" />
              {status.favouritesCount || ""}
            </button>
            <button
              className={`inline-flex h-5 items-center gap-1 ${status.bookmarked ? "text-blue hover:text-blue" : "text-overlay0 hover:text-subtext0"}`}
              title={t("Bookmark")}
              onClick={() =>
                void action(
                  column,
                  status,
                  status.bookmarked ? "unbookmark" : "bookmark",
                )
              }
            >
              <Bookmark className="h-3.5 w-3.5" />
            </button>
            <div className="ml-auto flex items-center gap-2">
              <button
                className="btn btn-ghost btn-xs h-5 min-h-5 px-1 text-subtext0 hover:bg-surface1 hover:text-text"
                title={t("More")}
                onClick={toggleMenu}
                data-post-menu-trigger
              >
                <MoreHorizontal className="h-3.5 w-3.5" />
              </button>
              {menuPosition ? (
                <PostMenuPopover
                  position={menuPosition}
                  onClose={() => setMenuPosition(null)}
                  items={[
                    ...(canManageStatus
                      ? [
                          { label: t("Edit post"), action: editPost },
                          {
                            label: t("Delete post"),
                            action: () => void deletePost(),
                            danger: true,
                          },
                        ]
                      : []),
                    ...(status.notificationId
                      ? [
                          {
                            label: t("Find AIR context"),
                            action: () => openAirContextPane(status),
                            disabled: !status.notificationAccountId,
                          },
                        ]
                      : []),
                    { label: t("Copy text"), action: copyText },
                    { label: t("Copy URL"), action: copyUrl, disabled: !url },
                    {
                      label: t("Open in browser"),
                      action: openUrl,
                      disabled: !url,
                    },
                  ]}
                />
              ) : null}
            </div>
          </div>
        </div>
      </div>
    </article>
  );
}

function QuotePreview({
  status,
  onOpenUser,
  onOpenStatus,
}: {
  status: TimelineStatus;
  onOpenUser: (status: TimelineStatus) => void;
  onOpenStatus: (status: TimelineStatus) => void;
}) {
  const cwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.cw_behavior ?? "Hide",
  );

  return (
    <div className="mt-2 max-w-full overflow-hidden rounded border border-surface1 bg-base-300/50 p-2">
      <div className="flex min-w-0 items-center gap-2">
        <button
          className="shrink-0"
          onClick={() => onOpenUser(status)}
          title={t("Open profile")}
        >
          <Avatar src={status.avatar} label={status.displayName} size="md" />
        </button>
        <button
          className="min-w-0 flex-1 truncate text-left text-xs font-semibold hover:text-blue"
          onClick={() => onOpenUser(status)}
          title={t("Open profile")}
        >
          <CustomEmojiText
            text={status.displayName || status.acct}
            emojis={status.accountEmojis}
          />
        </button>
        <button
          className="shrink-0 text-xs text-overlay0 hover:text-blue"
          onClick={() => onOpenStatus(status)}
          title={t("Open quoted post")}
        >
          {formatTime(statusDisplayCreatedAt(status))}
        </button>
      </div>
      <div className="mt-1 truncate text-xs text-subtext0">{status.acct}</div>
      <StatusContentBlock
        status={status}
        cwBehavior={cwBehavior}
        className="mt-1 max-w-full font-extralight"
      />
    </div>
  );
}

function StatusContentBlock({
  status,
  cwBehavior,
  className,
}: {
  status: TimelineStatus;
  cwBehavior: AppearanceSettings["cw_behavior"];
  className?: string;
}) {
  const behavior = useAppStore((state) => state.snapshot?.settings.confirmation);
  const translationEnabled = behavior?.translate_enabled ?? false;
  const autoTranslationEnabled = behavior?.auto_translate_enabled ?? false;
  const translationEngine = behavior?.translation_engine ?? "TranslationFramework";
  const jumbomojiEnabled = behavior?.jumbomoji_enabled ?? false;
  const translationSupported = getClientPlatform() === "macos";
  const targetLanguage = targetTranslationLanguage();
  const plainText = React.useMemo(
    () => htmlToPlainText(status.content),
    [status.content],
  );
  const cacheKey = translationCacheKey(status, targetLanguage, translationEngine);
  const [translation, setTranslation] = React.useState<TranslationState>(() => {
    const cached = translationCache.get(cacheKey);
    return cached
      ? {
          kind: "translated",
          text: cached.text,
          sourceLanguage: cached.sourceLanguage,
        }
      : { kind: "idle" };
  });
  const [showTranslated, setShowTranslated] = React.useState(() =>
    translationCache.has(cacheKey),
  );
  const spoilerText = status.spoilerText.trim();
  const canTranslate =
    translationEnabled && shouldOfferTranslation(status, plainText);
  const translated =
    canTranslate && translation.kind === "translated" && showTranslated
      ? translation
      : undefined;

  React.useEffect(() => {
    const cached = translationCache.get(cacheKey);
    if (cached) {
      setTranslation({
        kind: "translated",
        text: cached.text,
        sourceLanguage: cached.sourceLanguage,
      });
    } else {
      setTranslation({ kind: "idle" });
      setShowTranslated(false);
    }
  }, [cacheKey]);

  const translate = React.useCallback(async () => {
    if (!translationSupported || !plainText.trim()) return;
    const cached = translationCache.get(cacheKey);
    if (cached) {
      setTranslation({
        kind: "translated",
        text: cached.text,
        sourceLanguage: cached.sourceLanguage,
      });
      setShowTranslated(true);
      return;
    }

    setTranslation({ kind: "loading" });
    try {
      const response = await invokeCommand<TranslateStatusResponse>(
        "translate_status_text",
        {
          request: {
            text: plainText,
            sourceLanguage: status.language ?? null,
            targetLanguage,
            translationEngine,
          },
        },
      );
      const next = {
        text: response.text.trim(),
        sourceLanguage: response.sourceLanguage ?? status.language ?? null,
      };
      translationCache.set(cacheKey, next);
      setTranslation({
        kind: "translated",
        text: next.text,
        sourceLanguage: next.sourceLanguage,
      });
      setShowTranslated(true);
    } catch (error) {
      setTranslation({
        kind: "error",
        message: error instanceof Error ? error.message : String(error),
      });
      setShowTranslated(false);
    }
  }, [
    cacheKey,
    plainText,
    status.language,
    targetLanguage,
    translationEngine,
    translationSupported,
  ]);

  React.useEffect(() => {
    if (
      !canTranslate ||
      !translationSupported ||
      !autoTranslationEnabled ||
      translation.kind !== "idle"
    ) {
      return;
    }
    void translate();
  }, [
    autoTranslationEnabled,
    canTranslate,
    translate,
    translation.kind,
    translationSupported,
  ]);

  const translationMeta = canTranslate ? (
    <div className="mb-1 flex min-w-0 flex-wrap items-center gap-1.5 text-xs text-subtext0">
      <Languages className="h-3.5 w-3.5 shrink-0" />
      {!translationSupported ? (
        <span>{t("Translation is not supported on this OS.")}</span>
      ) : translated ? (
        <>
          <span>
            {t("Translated from {language}", {
              language: languageDisplayName(
                translated.sourceLanguage ?? status.language,
              ),
            })}
          </span>
          <button
            type="button"
            className="font-semibold text-blue hover:underline"
            onClick={() => setShowTranslated(false)}
          >
            {t("Show original")}
          </button>
        </>
      ) : (
        <>
          <button
            type="button"
            className="inline-flex items-center gap-1 font-semibold text-blue hover:underline disabled:cursor-wait disabled:text-subtext0"
            disabled={translation.kind === "loading"}
            onClick={() => void translate()}
          >
            {translation.kind === "loading"
              ? t("Translating...")
              : t("Show translation")}
          </button>
          {translation.kind === "error" ? (
            <span className="text-red">
              {t("Translation failed")}: {translation.message}
            </span>
          ) : null}
        </>
      )}
    </div>
  ) : null;
  const contentHtml = translated
    ? translatedTextToHtml(translated.text)
    : status.content;
  const contentEmojis = translated ? [] : status.emojis;
  const content = (
    <>
      {translationMeta}
      <StatusHtmlWithCustomEmojis
        className="status-content"
        html={contentHtml}
        emojis={contentEmojis}
        jumbomojiEnabled={jumbomojiEnabled}
      />
    </>
  );

  if (!spoilerText) {
    return <div className={className}>{content}</div>;
  }

  if (cwBehavior === "AlwaysExpand") {
    return (
      <div
        className={`status-cw-collapse collapse collapse-open border border-surface0 bg-base-300/50 ${className ?? ""}`}
      >
        <div className="collapse-title min-h-0 px-3 py-2 text-sm font-semibold text-warning">
          {spoilerText}
        </div>
        <div className="collapse-content px-3 pb-3">{content}</div>
      </div>
    );
  }

  return (
    <details
      className={`status-cw-collapse collapse collapse-arrow border border-surface0 bg-base-300/50 ${className ?? ""}`}
    >
      <summary className="collapse-title min-h-0 px-3 py-2 text-sm font-semibold text-warning">
        {spoilerText}
      </summary>
      <div className="collapse-content px-3 pb-3">{content}</div>
    </details>
  );
}

function statusFontSizeClass(fontSize: AppearanceSettings["font_size"]) {
  if (fontSize === "Small") return "status-size-small";
  if (fontSize === "Large") return "status-size-large";
  return "status-size-medium";
}

function statusItemStyle(
  paddingLeft: number | undefined,
  borderLeftColor: string | undefined,
) {
  if (paddingLeft === undefined && !borderLeftColor) return undefined;
  const style: React.CSSProperties = {};
  if (paddingLeft !== undefined) style.paddingLeft = paddingLeft;
  if (borderLeftColor) style.borderLeftColor = borderLeftColor;
  return style;
}

function QuoteLinkPreview({ url }: { url: string }) {
  return (
    <button
      className="mt-2 block max-w-full overflow-hidden rounded border border-surface1 bg-base-300/50 p-2 text-left text-xs text-blue hover:border-blue/60"
      onClick={() =>
        void openExternalUrl(url).catch((error) =>
          useAppStore.setState({ error: String(error) }),
        )
      }
      title={t("Open quoted post")}
    >
      {url}
    </button>
  );
}

function StatusPoll({
  status,
  onVote,
}: {
  status: TimelineStatus;
  onVote: (choices: number[]) => Promise<PollSummary | null>;
}) {
  const poll = status.poll;
  const [selected, setSelected] = React.useState<Set<number>>(
    () => new Set(poll?.ownVotes ?? []),
  );
  const [showResults, setShowResults] = React.useState(() =>
    Boolean(poll?.voted || poll?.expired),
  );
  const [pending, setPending] = React.useState(false);

  React.useEffect(() => {
    setSelected(new Set(poll?.ownVotes ?? []));
    setShowResults(Boolean(poll?.voted || poll?.expired));
  }, [poll?.id, poll?.voted, poll?.expired, poll?.ownVotes]);

  if (!poll || poll.options.length === 0) return null;

  const canVote = poll.voted !== true && !poll.expired;
  const totalVotes = Math.max(0, poll.votesCount);
  const selectedCount = selected.size;

  const toggleOption = (index: number) => {
    if (!canVote || pending) return;
    setSelected((current) => {
      if (!poll.multiple) return new Set([index]);
      const next = new Set(current);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  };

  const submitVote = async () => {
    if (!canVote || selectedCount === 0 || pending) return;
    setPending(true);
    const updated = await onVote([...selected].sort((a, b) => a - b));
    setPending(false);
    if (updated) setShowResults(true);
  };

  return (
    <div className="mt-3 space-y-2 text-sm">
      <div className="space-y-1.5">
        {poll.options.map((option, index) => (
          <PollOptionRow
            key={`${poll.id}-${index}`}
            option={option}
            index={index}
            poll={poll}
            checked={selected.has(index)}
            disabled={!canVote || pending}
            showResults={showResults || poll.voted === true || poll.expired}
            totalVotes={totalVotes}
            onToggle={() => toggleOption(index)}
          />
        ))}
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs text-overlay0">
        {canVote ? (
          <button
            type="button"
            className="btn btn-outline btn-xs min-h-7 border-surface1 px-3"
            disabled={selectedCount === 0 || pending}
            onClick={() => void submitVote()}
          >
            {pending ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
            {t("Vote")}
          </button>
        ) : null}
        {!showResults && !poll.voted && (
          <button
            type="button"
            className="text-subtext0 hover:text-blue"
            onClick={() => setShowResults(true)}
          >
            {t("Show results")}
          </button>
        )}
        <span>{formatPollCount(poll)}</span>
        <span>{formatPollExpiry(poll)}</span>
      </div>
    </div>
  );
}

function PollOptionRow({
  option,
  index,
  poll,
  checked,
  disabled,
  showResults,
  totalVotes,
  onToggle,
}: {
  option: PollSummary["options"][number];
  index: number;
  poll: PollSummary;
  checked: boolean;
  disabled: boolean;
  showResults: boolean;
  totalVotes: number;
  onToggle: () => void;
}) {
  const votes = option.votesCount ?? 0;
  const percentage =
    totalVotes > 0 ? Math.round((votes / totalVotes) * 100) : 0;
  const inputType = poll.multiple ? "checkbox" : "radio";

  return (
    <label
      className={`block rounded border border-surface0 bg-base-200/50 px-2.5 py-2 ${disabled ? "" : "cursor-pointer hover:border-blue/70"}`}
    >
      <span className="flex min-w-0 items-center gap-2">
        <input
          type={inputType}
          name={`poll-${poll.id}`}
          className={`${poll.multiple ? "checkbox checkbox-xs" : "radio radio-xs"} border-overlay0 bg-base`}
          checked={checked}
          disabled={disabled}
          onChange={onToggle}
        />
        <span className="min-w-0 flex-1 text-text">
          <CustomEmojiText text={option.title} emojis={poll.emojis} />
        </span>
        {showResults ? (
          <span className="shrink-0 tabular-nums text-xs text-subtext0">
            {percentage}%
          </span>
        ) : null}
      </span>
      {showResults ? (
        <span className="mt-1.5 block h-1.5 overflow-hidden rounded bg-surface1">
          <span
            className="block h-full rounded bg-blue"
            style={{ width: `${percentage}%` }}
          />
        </span>
      ) : null}
    </label>
  );
}

function formatPollCount(poll: PollSummary) {
  const count = poll.votersCount ?? poll.votesCount;
  return t("{count} voters", { count: count.toLocaleString() });
}

function formatPollExpiry(poll: PollSummary) {
  if (poll.expired) return t("Closed");
  if (!poll.expiresAt) return t("No deadline");

  const remainingMs = Date.parse(poll.expiresAt) - Date.now();
  if (!Number.isFinite(remainingMs) || remainingMs <= 0)
    return t("Closing soon");
  const minutes = Math.ceil(remainingMs / 60_000);
  if (minutes < 60) return t("{count}m left", { count: minutes });
  const hours = Math.ceil(minutes / 60);
  if (hours < 48) return t("{count}h left", { count: hours });
  return t("{count}d left", { count: Math.ceil(hours / 24) });
}

function MediaThumbnail({
  media,
  sources,
  sensitive,
  visible,
  onToggle,
  onOpen,
}: {
  media: TimelineStatus["media"][number];
  sources: string[];
  sensitive: boolean;
  visible: boolean;
  onToggle: () => void;
  onOpen: () => void;
}) {
  const image = useRetriedMediaSource(sources);
  const blurhashUrl = React.useMemo(
    () => (media.blurhash ? blurHashToDataUrl(media.blurhash) : null),
    [media.blurhash],
  );
  const shouldHide = sensitive && !visible;
  const placeholderStyle = blurhashUrl
    ? { backgroundImage: `url(${blurhashUrl})` }
    : undefined;

  return (
    <div className="relative h-28 w-full overflow-hidden rounded border border-transparent bg-base-200 hover:border-blue">
      <button
        type="button"
        className="h-full w-full"
        onClick={shouldHide ? onToggle : onOpen}
        title={shouldHide ? t("Reveal media") : t("Open media preview")}
      >
        {shouldHide ? (
          <div
            className="h-full w-full bg-surface0 bg-cover bg-center"
            style={placeholderStyle}
          />
        ) : (
          <>
            {!image.loaded ? (
              <div
                className="absolute inset-0 grid place-items-center bg-surface0 bg-cover bg-center text-xs text-overlay0"
                style={placeholderStyle}
              >
                {image.retrying ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : image.failed ? (
                  <span>{t("Media unavailable")}</span>
                ) : null}
              </div>
            ) : null}
            {image.src ? (
              <img
                key={image.key}
                src={image.src}
                alt={media.description ?? ""}
                className={`h-full w-full object-cover ${image.loaded ? "" : "opacity-0"}`}
                onLoad={image.onLoad}
                onError={image.onError}
              />
            ) : null}
          </>
        )}
      </button>
      {sensitive ? (
        <button
          type="button"
          className="absolute left-1 top-1 grid h-7 w-7 place-items-center rounded bg-base/80 text-text shadow hover:bg-base"
          onClick={onToggle}
          title={shouldHide ? t("Reveal media") : t("Hide media")}
        >
          {shouldHide ? (
            <Eye className="h-3.5 w-3.5" />
          ) : (
            <EyeOff className="h-3.5 w-3.5" />
          )}
        </button>
      ) : null}
    </div>
  );
}

function NotificationMeta({
  status,
  onOpenUser,
}: {
  status: TimelineStatus;
  onOpenUser: (status: TimelineStatus) => void;
}) {
  if (!status.notificationLabel) return null;

  const Icon = notificationMetaIcon(status.notificationLabel);
  const boostCreatedAt =
    status.originalCreatedAt && notificationMetaIsBoost(status.notificationLabel)
      ? status.createdAt
      : null;
  const notificationUserStatus = status.notificationAccountId
    ? {
        ...status,
        accountId: status.notificationAccountId,
        acct: status.notificationAcct ?? status.notificationAccountId,
        displayName:
          status.notificationDisplayName ??
          status.notificationAcct ??
          status.notificationAccountId,
        avatar: status.notificationAvatar ?? "",
        accountEmojis: status.notificationAccountEmojis ?? [],
      }
    : null;
  const className =
    "mb-1 flex min-w-0 max-w-full items-center gap-1.5 text-xs font-semibold text-overlay0";
  const content = (
    <>
      <Icon className="h-3.5 w-3.5 shrink-0" />
      {status.notificationAvatar ? (
        <Avatar
          src={status.notificationAvatar}
          label={status.notificationLabel}
          size="xs"
        />
      ) : null}
      <span className="min-w-0 truncate">
        <CustomEmojiText
          text={status.notificationLabel}
          emojis={status.notificationAccountEmojis ?? []}
        />
      </span>
      {boostCreatedAt ? (
        <span className="shrink-0 font-normal text-overlay0">
          {formatTime(boostCreatedAt)}
        </span>
      ) : null}
    </>
  );

  if (!notificationUserStatus) {
    return <div className={className}>{content}</div>;
  }

  return (
    <button
      type="button"
      className={`${className} hover:text-blue`}
      onClick={(event) => {
        event.stopPropagation();
        onOpenUser(notificationUserStatus);
      }}
      title={t("Open profile")}
    >
      {content}
    </button>
  );
}

function notificationMetaIcon(label: string) {
  if (notificationMetaIsBoost(label)) {
    return Repeat2;
  }
  const normalized = label.toLowerCase();
  if (normalized.includes("favourite") || normalized.includes("favorite")) {
    return Star;
  }
  return MessageCircle;
}

function notificationMetaIsBoost(label: string) {
  const normalized = label.toLowerCase();
  return normalized.includes("boost") || normalized.includes("reblog");
}

function statusDisplayCreatedAt(status: TimelineStatus) {
  return status.originalCreatedAt ?? status.createdAt;
}
