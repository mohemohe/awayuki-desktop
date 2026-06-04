import { create } from "zustand";
import type {
  AccountSummary,
  AppSnapshot,
  ColumnSummary,
  ConfirmationDialogRequest,
  ConfirmationDialogState,
  DeleteStatusRequest,
  EditStatusRequest,
  MediaAttachment,
  MediaPreviewState,
  PollSummary,
  PostSubmitOptions,
  SaveColumnsRequest,
  SettingsSnapshot,
  StatusBarSnapshot,
  TimelinePageResponse,
  TimelineRequest,
  TimelineStatus,
  TimelineStreamEvent,
  UserProfileTarget,
  VotePollRequest,
} from "../types/app";
import { invokeCommand } from "../api/tauri";
import { confirmStatusAction } from "../utils/confirmation";
import {
  createColumn,
  defaultTimelineName,
  groupColumnsByPane,
  normalizeColumns,
  normalizeDisplayFilter,
  reconcileActiveTabs,
  timelineDisplayFilterApplies,
} from "../utils/columns";
import { previewMediaSources } from "../utils/media";
import { hasTopLevelSqlLimit } from "../utils/sql";
import { matchPresetVisibility } from "../utils/visibility";
import { t } from "../i18n";

type LoadTimelineOptions = {
  delta?: boolean;
};

type PendingTimelineRefresh = {
  column: ColumnSummary;
  options: LoadTimelineOptions;
};

export type AppStore = {
  snapshot?: AppSnapshot;
  timelines: Record<string, TimelineStatus[]>;
  dynamicColumns: ColumnSummary[];
  loading: Record<string, boolean>;
  loadingMore: Record<string, boolean>;
  timelineHasMore: Record<string, boolean>;
  timelineNearTop: Record<string, boolean>;
  activeTabs: Record<number, string>;
  pendingScrollPaneIndex?: number;
  error?: string;
  statusMessage: string;
  statusBar?: StatusBarSnapshot;
  settingsOpen: boolean;
  selectedSettings: SettingsSection;
  loginOpen: boolean;
  composeText: string;
  composeTarget?: { kind: "reply" | "quote"; status: TimelineStatus } | null;
  visibility: "public" | "unlisted" | "private" | "direct";
  mediaPreview?: MediaPreviewState | null;
  confirmationDialog?: ConfirmationDialogState;
  loadSnapshot: () => Promise<void>;
  refreshAccounts: () => Promise<void>;
  loginWithInstanceDomain: (domain: string) => Promise<boolean>;
  loginWithBluesky: (identifier: string, password: string) => Promise<boolean>;
  loadStatusBar: () => Promise<void>;
  loadTimeline: (
    column: ColumnSummary,
    refresh?: boolean,
    options?: LoadTimelineOptions,
  ) => Promise<void>;
  loadMoreTimeline: (column: ColumnSummary) => Promise<void>;
  setTimelineNearTop: (column: ColumnSummary, nearTop: boolean) => void;
  trimTimelineToMaxStatuses: (column: ColumnSummary) => void;
  setActiveTab: (paneIndex: number, column: ColumnSummary) => void;
  addBookmarksPane: () => void;
  openUserBookmarksPane: (target: UserProfileTarget) => void;
  openSearchPane: (query: string) => void;
  openThreadPane: (status: TimelineStatus) => void;
  openAirContextPane: (status: TimelineStatus) => void;
  openUserPane: (status: TimelineStatus) => void;
  clearPendingPaneScroll: (paneIndex: number) => void;
  openMediaPreview: (status: TimelineStatus, media: MediaAttachment) => void;
  closeMediaPreview: () => void;
  closeDynamicPane: (paneIndex: number) => void;
  post: (options?: PostSubmitOptions) => Promise<boolean>;
  replyStatus: (status: TimelineStatus) => void;
  quoteStatus: (status: TimelineStatus) => void;
  clearComposeTarget: () => void;
  action: (
    column: ColumnSummary,
    status: TimelineStatus,
    action: string,
  ) => Promise<void>;
  votePoll: (status: TimelineStatus, choices: number[]) => Promise<PollSummary | null>;
  editStatus: (
    status: TimelineStatus,
    content: string,
  ) => Promise<TimelineStatus | null>;
  deleteStatus: (status: TimelineStatus) => Promise<boolean>;
  switchAccount: (acct: string) => Promise<void>;
  logoutAccount: (acct: string) => Promise<void>;
  saveSetting: (key: string, value: unknown) => Promise<void>;
  saveColumns: (columns: ColumnSummary[]) => Promise<void>;
  applyStreamEvent: (event: TimelineStreamEvent) => void;
  requestConfirmation: (
    request: ConfirmationDialogRequest,
  ) => Promise<boolean>;
  resolveConfirmation: (confirmed: boolean) => void;
};

export type SettingsSection =
  | "Account"
  | "Appearance"
  | "Behavior"
  | "Performance"
  | "Notification"
  | "Timeline"
  | "Database"
  | "Debug"
  | "About";

let pendingConfirmationResolve:
  | ((confirmed: boolean) => void)
  | undefined;

const inFlightTimelineLoads = new Map<string, Promise<void>>();
const pendingTimelineRefreshes = new Map<string, PendingTimelineRefresh>();

function timelineLogContext(column: ColumnSummary) {
  return `column=${column.id} type=${column.columnType} account=${column.accountAcct ?? "active"} dynamic=${Boolean(column.dynamic)}`;
}

function uiElapsedMs(startedAt: number) {
  return (performance.now() - startedAt).toFixed(1);
}

function queuePendingTimelineRefresh(
  column: ColumnSummary,
  options: LoadTimelineOptions,
) {
  const existing = pendingTimelineRefreshes.get(column.id);
  pendingTimelineRefreshes.set(column.id, {
    column,
    options: mergeTimelineLoadOptions(existing?.options, options),
  });
}

function mergeTimelineLoadOptions(
  current: LoadTimelineOptions | undefined,
  next: LoadTimelineOptions,
): LoadTimelineOptions {
  if (!current) {
    return next.delta ? { delta: true } : {};
  }
  return current.delta && next.delta ? { delta: true } : {};
}

export const useAppStore = create<AppStore>((set, get) => ({
  timelines: {},
  dynamicColumns: [],
  loading: {},
  loadingMore: {},
  timelineHasMore: {},
  timelineNearTop: {},
  activeTabs: {},
  pendingScrollPaneIndex: undefined,
  statusMessage: t("Ready"),
  settingsOpen: false,
  selectedSettings: "Account",
  loginOpen: false,
  composeText: "",
  composeTarget: null,
  visibility: "public",
  mediaPreview: null,
  confirmationDialog: undefined,
  loadSnapshot: async () => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>("app_snapshot");
      set((state) => ({
        snapshot,
        activeTabs: reconcileActiveTabs(
          [...snapshot.columns, ...state.dynamicColumns],
          state.activeTabs,
        ),
        error: undefined,
      }));
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column)),
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  refreshAccounts: async () => {
    try {
      const accounts =
        await invokeCommand<AccountSummary[]>("account_summaries");
      set((state) =>
        state.snapshot
          ? {
              snapshot: {
                ...state.snapshot,
                accounts,
              },
              error: undefined,
            }
          : {},
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  loginWithInstanceDomain: async (domain) => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>(
        "login_with_instance_domain",
        { request: { domain } },
      );
      set((state) => ({
        snapshot,
        loginOpen: false,
        settingsOpen: false,
        activeTabs: reconcileActiveTabs(
          [...snapshot.columns, ...state.dynamicColumns],
          state.activeTabs,
        ),
        error: undefined,
      }));
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
      return true;
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
  },
  loginWithBluesky: async (identifier, password) => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>(
        "login_with_bluesky_app_password",
        { request: { identifier, password } },
      );
      set((state) => ({
        snapshot,
        loginOpen: false,
        settingsOpen: false,
        activeTabs: reconcileActiveTabs(
          [...snapshot.columns, ...state.dynamicColumns],
          state.activeTabs,
        ),
        error: undefined,
      }));
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
      return true;
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
  },
  loadStatusBar: async () => {
    try {
      const snapshot = await invokeCommand<
        Omit<StatusBarSnapshot, "fetchedAt">
      >("status_bar_snapshot");
      set({ statusBar: { ...snapshot, fetchedAt: Date.now() } });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  loadTimeline: async (column, refresh = false, options = {}) => {
    const inFlight = inFlightTimelineLoads.get(column.id);
    if (inFlight) {
      if (refresh) {
        queuePendingTimelineRefresh(column, options);
        console.debug(
          `[awayuki][ui-timeline] queued ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(options.delta)} reason=in_flight`,
        );
      } else {
        console.info(
          `[awayuki][ui-timeline] coalesced ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(options.delta)} reason=in_flight`,
        );
      }
      await inFlight;
      return;
    }

    const run = async () => {
      const startedAt = performance.now();
      const limit = timelinePageLimit(column);
      const currentTimeline = get().timelines[column.id] ?? [];
      const sinceStatus =
        refresh && options.delta && column.columnType === "yq"
          ? latestTimelineStatus(currentTimeline)
          : undefined;
      const request: TimelineRequest = {
        columnType: column.columnType,
        columnParam: column.columnParam,
        limit,
        accountAcct: column.accountAcct,
        displayFilter: timelineDisplayFilterApplies(column)
          ? normalizeDisplayFilter(column.displayFilter)
          : undefined,
        ...(sinceStatus
          ? {
              sinceStatusId: sinceStatus.id,
              sinceServerDomain: sinceStatus.serverDomain,
            }
          : {}),
      };
      console.info(
        `[awayuki][ui-timeline] start ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(sinceStatus)} limit=${limit}${sinceStatus ? ` since=${sinceStatus.serverDomain}:${sinceStatus.id}` : ""}`,
      );
      set((state) => ({ loading: { ...state.loading, [column.id]: true } }));
      try {
        const statuses =
          column.columnType === "thread"
            ? await invokeCommand<TimelineStatus[]>("status_thread", {
                request: {
                  ...parseThreadColumnParam(column.columnParam),
                  limit,
                },
              })
            : column.columnType === "airContext"
              ? await invokeCommand<TimelineStatus[]>("air_context", {
                  request: {
                    ...parseAirContextColumnParam(column.columnParam),
                    limit,
                  },
                })
            : await invokeCommand<TimelineStatus[]>(
                refresh ? "refresh_timeline" : "load_timeline",
                {
                  request,
                },
              );
        const displayStatuses = filterTimelineStatusesForColumn(
          statuses,
          column,
        );
        console.info(
          `[awayuki][ui-timeline] success ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(sinceStatus)} count=${statuses.length} display_count=${displayStatuses.length} duration_ms=${uiElapsedMs(startedAt)}`,
        );
        set((state) => {
          const shouldLimitDisplay = shouldLimitTimelineDisplay(state, column);
          const displayLimit = shouldLimitDisplay
            ? timelineDisplayLimit(column)
            : undefined;
          return {
            timelines: {
              ...state.timelines,
              [column.id]: sinceStatus
                ? mergeTimelineDelta(
                    column,
                    state.timelines[column.id] ?? [],
                    displayStatuses,
                    displayLimit,
                  )
                : mergeTimelineLoadPage(
                    column,
                    displayStatuses,
                    state.timelines[column.id] ?? [],
                    displayLimit,
                  ),
            },
            loading: { ...state.loading, [column.id]: false },
            timelineHasMore: {
              ...state.timelineHasMore,
              [column.id]: sinceStatus
                ? (state.timelineHasMore[column.id] ?? true)
                : column.columnType === "thread" ||
                    column.columnType === "airContext"
                  ? false
                : columnHasSqlLimit(column)
                  ? false
                  : columnCanLoadMoreFromApi(column) &&
                      timelineDisplayFilterApplies(column)
                    ? true
                  : timelinePageHasMore(statuses.length, limit, refresh),
            },
            error: undefined,
          };
        });
      } catch (error) {
        console.info(
          `[awayuki][ui-timeline] error ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(sinceStatus)} duration_ms=${uiElapsedMs(startedAt)} error=${String(error)}`,
        );
        set((state) => ({
          loading: { ...state.loading, [column.id]: false },
          error: String(error),
        }));
      } finally {
        inFlightTimelineLoads.delete(column.id);
        const pending = pendingTimelineRefreshes.get(column.id);
        if (pending) {
          pendingTimelineRefreshes.delete(column.id);
          await get().loadTimeline(pending.column, true, pending.options);
        }
      }
    };
    const promise = Promise.resolve().then(run);
    inFlightTimelineLoads.set(column.id, promise);
    await promise;
  },
  loadMoreTimeline: async (column) => {
    if (
      column.columnType === "thread" ||
      column.columnType === "profile" ||
      column.columnType === "airContext"
    )
      return;
    if (columnHasSqlLimit(column)) return;
    const { loading, loadingMore, timelineHasMore, timelines } = get();
    if (loading[column.id] || loadingMore[column.id]) return;
    if (timelineHasMore[column.id] === false) return;

    const current = timelines[column.id] ?? [];
    if (current.length === 0) {
      await get().loadTimeline(column);
      return;
    }

    const limit = timelinePageLimit(column);
    const maxStatus = oldestTimelineStatus(current);
    const request: TimelineRequest = {
      columnType: column.columnType,
      columnParam: column.columnParam,
      limit,
      offset: current.length,
      maxStatusId: maxStatus?.id,
      accountAcct: column.accountAcct,
      displayFilter: timelineDisplayFilterApplies(column)
        ? normalizeDisplayFilter(column.displayFilter)
        : undefined,
    };
    const startedAt = performance.now();
    console.info(
      `[awayuki][ui-timeline] load_more_start ${timelineLogContext(column)} offset=${request.offset} limit=${limit}`,
    );
    set((state) => ({
      loadingMore: { ...state.loadingMore, [column.id]: true },
    }));
    try {
      const response = columnCanLoadMoreFromApi(column)
        ? await invokeCommand<TimelinePageResponse>("load_more_timeline", {
            request,
          })
        : {
            statuses: await invokeCommand<TimelineStatus[]>("load_timeline", {
              request,
            }),
            hasMore: undefined,
          };
      const nextPage = response.statuses;
      const displayNextPage = filterTimelineStatusesForColumn(nextPage, column);
      console.info(
        `[awayuki][ui-timeline] load_more_success ${timelineLogContext(column)} offset=${request.offset} count=${nextPage.length} display_count=${displayNextPage.length} duration_ms=${uiElapsedMs(startedAt)}`,
      );
      set((state) => ({
        timelines: {
          ...state.timelines,
          [column.id]: mergeTimelinePage(
            column,
            state.timelines[column.id] ?? [],
            displayNextPage,
          ),
        },
        loadingMore: { ...state.loadingMore, [column.id]: false },
        timelineHasMore: {
          ...state.timelineHasMore,
          [column.id]:
            typeof response.hasMore === "boolean"
              ? response.hasMore
              : timelinePageHasMore(nextPage.length, limit),
        },
        error: undefined,
      }));
    } catch (error) {
      console.info(
        `[awayuki][ui-timeline] load_more_error ${timelineLogContext(column)} offset=${request.offset} duration_ms=${uiElapsedMs(startedAt)} error=${String(error)}`,
      );
      set((state) => ({
        loadingMore: { ...state.loadingMore, [column.id]: false },
        error: String(error),
      }));
    }
  },
  setTimelineNearTop: (column, nearTop) => {
    set((state) => {
      const currentNearTop = state.timelineNearTop[column.id] ?? true;
      const current = state.timelines[column.id] ?? EMPTY_TIMELINE_STATUSES;
      const limit = timelineDisplayLimit(column);
      const shouldTrim = nearTop && current.length > limit;
      if (currentNearTop === nearTop && !shouldTrim) return {};
      return {
        timelineNearTop: {
          ...state.timelineNearTop,
          [column.id]: nearTop,
        },
        ...(shouldTrim
          ? {
              timelines: {
                ...state.timelines,
                [column.id]: current.slice(0, limit),
              },
            }
          : {}),
      };
    });
  },
  trimTimelineToMaxStatuses: (column) => {
    set((state) => {
      const current = state.timelines[column.id] ?? EMPTY_TIMELINE_STATUSES;
      const limit = timelineDisplayLimit(column);
      const timelines =
        current.length > limit
          ? {
              ...state.timelines,
              [column.id]: current.slice(0, limit),
            }
          : state.timelines;
      return {
        timelines,
        timelineNearTop: {
          ...state.timelineNearTop,
          [column.id]: true,
        },
      };
    });
  },
  setActiveTab: (paneIndex, column) => {
    set((state) => ({
      activeTabs: { ...state.activeTabs, [paneIndex]: column.id },
    }));
    if (!get().timelines[column.id]) void get().loadTimeline(column);
  },
  addBookmarksPane: () => {
    const { snapshot, dynamicColumns, timelines } = get();
    const existing = dynamicColumns.find(
      (column) => column.columnType === "bookmarks",
    );
    if (existing) {
      set((state) => ({
        activeTabs: { ...state.activeTabs, [existing.paneIndex]: existing.id },
      }));
      if (!timelines[existing.id]) void get().loadTimeline(existing);
      set({ pendingScrollPaneIndex: existing.paneIndex });
      return;
    }

    const allColumns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    const nextPaneIndex =
      allColumns.reduce(
        (maxPane, column) => Math.max(maxPane, column.paneIndex),
        -1,
      ) + 1;
    const column = {
      ...createColumn(nextPaneIndex, 0, "bookmarks"),
      dynamic: true,
    };
    set((state) => ({
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: { ...state.activeTabs, [nextPaneIndex]: column.id },
    }));
    void get().loadTimeline(column);
    set({ pendingScrollPaneIndex: nextPaneIndex });
  },
  openUserBookmarksPane: (target) => {
    if (!target.accountId || !target.serverDomain) return;
    const { snapshot, dynamicColumns, timelines } = get();
    const columnParam = userBookmarksColumnParam(target);
    const existing = dynamicColumns.find(
      (column) =>
        column.columnType === "user_bookmarks" &&
        column.columnParam === columnParam,
    );
    if (existing) {
      set((state) => ({
        activeTabs: { ...state.activeTabs, [existing.paneIndex]: existing.id },
      }));
      if (!timelines[existing.id]) void get().loadTimeline(existing);
      set({ pendingScrollPaneIndex: existing.paneIndex });
      return;
    }

    const allColumns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    const nextPaneIndex =
      allColumns.reduce(
        (maxPane, column) => Math.max(maxPane, column.paneIndex),
        -1,
      ) + 1;
    const acct = target.acct || target.accountId;
    const column: ColumnSummary = {
      ...createColumn(nextPaneIndex, 0, "user_bookmarks"),
      columnParam,
      name: t("Bookmarks by {acct}", { acct: `@${acct.replace(/^@/, "")}` }),
      maxStatuses: 100,
      dynamic: true,
    };
    set((state) => ({
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: { ...state.activeTabs, [nextPaneIndex]: column.id },
    }));
    void get().loadTimeline(column);
    set({ pendingScrollPaneIndex: nextPaneIndex });
  },
  openSearchPane: (rawQuery) => {
    const query = rawQuery.trim();
    if (!query) return;

    const yqMode = query.startsWith("?");
    const columnType = yqMode ? "yq" : "search";
    const columnParam = yqMode ? query.slice(1).trim() : query;
    if (!columnParam) return;

    const { snapshot, dynamicColumns, timelines } = get();
    const existing = dynamicColumns.find(
      (column) =>
        column.columnType === columnType && column.columnParam === columnParam,
    );
    if (existing) {
      set((state) => ({
        activeTabs: { ...state.activeTabs, [existing.paneIndex]: existing.id },
      }));
      if (!timelines[existing.id]) void get().loadTimeline(existing);
      set({ pendingScrollPaneIndex: existing.paneIndex });
      return;
    }

    const allColumns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    const nextPaneIndex =
      allColumns.reduce(
        (maxPane, column) => Math.max(maxPane, column.paneIndex),
        -1,
      ) + 1;
    const namePrefix = yqMode ? "YQ" : t("Search");
    const shortQuery =
      columnParam.length > 40 ? `${columnParam.slice(0, 39)}...` : columnParam;
    const column: ColumnSummary = {
      ...createColumn(nextPaneIndex, 0, columnType),
      columnParam,
      name: `${namePrefix}: ${shortQuery}`,
      maxStatuses: 100,
      dynamic: true,
    };
    set((state) => ({
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: { ...state.activeTabs, [nextPaneIndex]: column.id },
    }));
    void get().loadTimeline(column);
    set({ pendingScrollPaneIndex: nextPaneIndex });
  },
  openThreadPane: (status) => {
    const { snapshot, dynamicColumns, timelines } = get();
    const statusId = status.originalStatusId || status.id;
    if (!statusId || !status.serverDomain) return;

    const columnParam = threadColumnParam(status);
    const existing = dynamicColumns.find(
      (column) =>
        column.columnType === "thread" && column.columnParam === columnParam,
    );
    if (existing) {
      set((state) => ({
        activeTabs: { ...state.activeTabs, [existing.paneIndex]: existing.id },
      }));
      if (!timelines[existing.id]) void get().loadTimeline(existing);
      set({ pendingScrollPaneIndex: existing.paneIndex });
      return;
    }

    const allColumns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    const nextPaneIndex =
      allColumns.reduce(
        (maxPane, column) => Math.max(maxPane, column.paneIndex),
        -1,
      ) + 1;
    const column: ColumnSummary = {
      ...createColumn(nextPaneIndex, 0, "thread"),
      columnParam,
      name: t("Thread"),
      maxStatuses: 240,
      dynamic: true,
    };
    set((state) => ({
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: { ...state.activeTabs, [nextPaneIndex]: column.id },
    }));
    void get().loadTimeline(column);
    set({ pendingScrollPaneIndex: nextPaneIndex });
  },
  openAirContextPane: (status) => {
    const { snapshot, dynamicColumns, timelines } = get();
    const statusId = status.originalStatusId || status.id;
    const accountId = status.notificationAccountId;
    if (!statusId || !status.serverDomain || !accountId) return;

    const columnParam = airContextColumnParam(status);
    const existing = dynamicColumns.find(
      (column) =>
        column.columnType === "airContext" &&
        column.columnParam === columnParam,
    );
    if (existing) {
      set((state) => ({
        activeTabs: { ...state.activeTabs, [existing.paneIndex]: existing.id },
      }));
      if (!timelines[existing.id]) void get().loadTimeline(existing);
      set({ pendingScrollPaneIndex: existing.paneIndex });
      return;
    }

    const allColumns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    const nextPaneIndex =
      allColumns.reduce(
        (maxPane, column) => Math.max(maxPane, column.paneIndex),
        -1,
      ) + 1;
    const column: ColumnSummary = {
      ...createColumn(nextPaneIndex, 0, "airContext"),
      columnParam,
      name: t("AIR context"),
      maxStatuses: 2,
      dynamic: true,
    };
    set((state) => ({
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: { ...state.activeTabs, [nextPaneIndex]: column.id },
    }));
    void get().loadTimeline(column);
    set({ pendingScrollPaneIndex: nextPaneIndex });
  },
  openUserPane: (status) => {
    const { snapshot, dynamicColumns } = get();
    const target: UserProfileTarget = {
      accountId: status.accountId,
      serverDomain: status.serverDomain,
      acct: status.acct,
      displayName: status.displayName,
      avatar: status.avatar,
    };
    const existing = dynamicColumns.find(
      (column) =>
        column.columnType === "profile" &&
        column.profile?.accountId === target.accountId &&
        column.profile?.serverDomain === target.serverDomain,
    );
    if (existing) {
      set((state) => ({
        dynamicColumns: state.dynamicColumns.map((column) =>
          column.id === existing.id
            ? {
                ...column,
                name: target.acct || column.name,
                profile: {
                  ...target,
                  acct: target.acct || column.profile?.acct || "",
                  displayName:
                    target.displayName || column.profile?.displayName || "",
                  avatar: target.avatar || column.profile?.avatar || "",
                },
              }
            : column,
        ),
        activeTabs: { ...state.activeTabs, [existing.paneIndex]: existing.id },
        pendingScrollPaneIndex: existing.paneIndex,
      }));
      return;
    }

    const allColumns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    const nextPaneIndex =
      allColumns.reduce(
        (maxPane, column) => Math.max(maxPane, column.paneIndex),
        -1,
      ) + 1;
    const column: ColumnSummary = {
      ...createColumn(nextPaneIndex, 0, "profile"),
      name: target.acct,
      maxStatuses: 80,
      dynamic: true,
      profile: target,
    };
    set((state) => ({
      dynamicColumns: [...state.dynamicColumns, column],
      activeTabs: { ...state.activeTabs, [nextPaneIndex]: column.id },
      pendingScrollPaneIndex: nextPaneIndex,
    }));
  },
  clearPendingPaneScroll: (paneIndex) => {
    set((state) =>
      state.pendingScrollPaneIndex === paneIndex
        ? { pendingScrollPaneIndex: undefined }
        : {},
    );
  },
  openMediaPreview: (status, media) => {
    const isVideo =
      media.media_type?.startsWith("video") || media.type?.startsWith("video");
    const mediaSource =
      get().snapshot?.settings.confirmation.media_source ?? "Local";
    const src = previewMediaSources(media, isVideo, mediaSource)[0];
    if (!src) return;
    set({ mediaPreview: { status, media, src } });
  },
  closeMediaPreview: () => set({ mediaPreview: null }),
  closeDynamicPane: (paneIndex) => {
    set((state) => {
      const activeTabs = { ...state.activeTabs };
      const timelines = { ...state.timelines };
      const timelineHasMore = { ...state.timelineHasMore };
      const timelineNearTop = { ...state.timelineNearTop };
      const loadingMore = { ...state.loadingMore };
      delete activeTabs[paneIndex];
      for (const column of state.dynamicColumns.filter(
        (item) => item.paneIndex === paneIndex,
      )) {
        delete timelines[column.id];
        delete timelineHasMore[column.id];
        delete timelineNearTop[column.id];
        delete loadingMore[column.id];
      }
      return {
        activeTabs,
        timelines,
        timelineHasMore,
        timelineNearTop,
        loadingMore,
        dynamicColumns: state.dynamicColumns.filter(
          (column) => column.paneIndex !== paneIndex,
        ),
      };
    });
  },
  post: async (options = {}) => {
    const { composeText, composeTarget, visibility, snapshot } = get();
    const hasMedia = Boolean(options.mediaIds?.length);
    const hasPoll = Boolean(options.poll?.options.length);
    if (!composeText.trim() && !hasMedia && !hasPoll) return false;
    const resolvedVisibility =
      matchPresetVisibility(snapshot?.settings.presetVisibility, composeText) ??
      visibility;
    try {
      await invokeCommand<TimelineStatus>("post_status", {
        request: {
          status: composeText,
          visibility: resolvedVisibility,
          mediaIds: options.mediaIds,
          sensitive: options.sensitive ?? false,
          spoilerText: options.spoilerText,
          poll: options.poll,
          inReplyToId:
            options.inReplyToId ??
            (composeTarget?.kind === "reply"
              ? composeTarget.status.originalStatusId
              : undefined),
          quoteId:
            options.quoteId ??
            (composeTarget?.kind === "quote"
              ? composeTarget.status.originalStatusId
              : undefined),
        },
      });
      set({ composeText: "", composeTarget: null });
      const home = get().snapshot?.columns.find(
        (column) => column.columnType === "home",
      );
      if (home) void get().loadTimeline(home, true);
      return true;
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
  },
  replyStatus: (status) => {
    const mention = `${status.acct.trim()} `;
    set((state) => {
      const current = state.composeText.trimEnd();
      return {
        composeTarget: { kind: "reply", status },
        composeText: current ? `${current}\n${mention}` : mention,
      };
    });
    requestAnimationFrame(() =>
      document.getElementById("compose-textarea")?.focus(),
    );
  },
  quoteStatus: (status) => {
    set({ composeTarget: { kind: "quote", status } });
    requestAnimationFrame(() =>
      document.getElementById("compose-textarea")?.focus(),
    );
  },
  clearComposeTarget: () => set({ composeTarget: null }),
  action: async (column, status, action) => {
    try {
      const confirmed = await confirmStatusAction(
        get().snapshot?.settings.confirmation,
        get().requestConfirmation,
        status,
        action,
      );
      if (!confirmed) return;
      const updated = await invokeCommand<TimelineStatus>("status_action", {
        request: { statusId: status.originalStatusId, action },
      });
      set((state) => {
        const limit = shouldLimitTimelineDisplay(state, column)
          ? timelineDisplayLimit(column)
          : Number.MAX_SAFE_INTEGER;
        return {
          timelines: {
            ...state.timelines,
            [column.id]: mergeActionStatusIntoTimeline(
              state.timelines[column.id] ?? [],
              status,
              updated,
              action,
              limit,
            ),
          },
        };
      });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  votePoll: async (status, choices) => {
    if (!status.poll) return null;
    try {
      const request: VotePollRequest = {
        statusId: status.originalStatusId,
        serverDomain: status.serverDomain,
        pollId: status.poll.id,
        choices,
      };
      const poll = await invokeCommand<PollSummary>("vote_poll", { request });
      set((state) => ({
        timelines: updatePollAcrossTimelines(state.timelines, status, poll),
        mediaPreview:
          state.mediaPreview &&
          isSameOriginalStatus(state.mediaPreview.status, status)
            ? {
                ...state.mediaPreview,
                status: { ...state.mediaPreview.status, poll },
              }
            : state.mediaPreview,
        error: undefined,
      }));
      return poll;
    } catch (error) {
      set({ error: String(error) });
      return null;
    }
  },
  editStatus: async (status, content) => {
    try {
      const request: EditStatusRequest = {
        statusId: status.originalStatusId,
        serverDomain: status.serverDomain,
        accountId: status.accountId,
        status: content,
        visibility: status.visibility,
        spoilerText: status.spoilerText || null,
        sensitive: status.sensitive,
      };
      const updated = await invokeCommand<TimelineStatus>("edit_own_status", {
        request,
      });
      set((state) => ({
        timelines: updateStatusAcrossTimelines(state.timelines, status, updated),
        mediaPreview:
          state.mediaPreview &&
          isSameOriginalStatus(state.mediaPreview.status, status)
            ? { ...state.mediaPreview, status: updated }
            : state.mediaPreview,
        error: undefined,
      }));
      return updated;
    } catch (error) {
      set({ error: String(error) });
      return null;
    }
  },
  deleteStatus: async (status) => {
    try {
      const request: DeleteStatusRequest = {
        statusId: status.originalStatusId,
        serverDomain: status.serverDomain,
        accountId: status.accountId,
      };
      await invokeCommand("delete_own_status", { request });
      set((state) => ({
        timelines: removeStatusAcrossTimelines(state.timelines, status),
        mediaPreview:
          state.mediaPreview &&
          isSameOriginalStatus(state.mediaPreview.status, status)
            ? null
            : state.mediaPreview,
        error: undefined,
      }));
      return true;
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
  },
  switchAccount: async (acct) => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>(
        "switch_active_account",
        { acct },
      );
      set((state) => ({
        snapshot,
        activeTabs: reconcileActiveTabs(snapshot.columns, state.activeTabs),
        error: undefined,
      }));
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  logoutAccount: async (acct) => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>("logout_account", {
        acct,
      });
      set((state) => ({
        snapshot,
        activeTabs: reconcileActiveTabs(snapshot.columns, state.activeTabs),
        error: undefined,
      }));
      if (snapshot.accounts.length > 0) {
        await Promise.all(
          snapshot.columns.map((column) => get().loadTimeline(column, true)),
        );
      }
    } catch (error) {
      set({ error: String(error) });
    }
  },
  saveSetting: async (key, value) => {
    try {
      const settings = await invokeCommand<SettingsSnapshot>("save_settings", {
        request: { key, value },
      });
      set((state) =>
        state.snapshot ? { snapshot: { ...state.snapshot, settings } } : {},
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  saveColumns: async (columns) => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>("save_columns", {
        request: { columns: normalizeColumns(columns) },
      });
      set((state) => ({
        snapshot,
        activeTabs: reconcileActiveTabs(snapshot.columns, state.activeTabs),
        error: undefined,
      }));
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  applyStreamEvent: (event) => {
    const { dynamicColumns, snapshot } = get();
    const columns = [...(snapshot?.columns ?? []), ...dynamicColumns];
    if (columns.length === 0) return;
    const eventStatus = event.status
      ? {
          ...event.status,
          sourceAcct: event.status.sourceAcct ?? event.sourceAcct,
        }
      : null;

    if (event.kind === "newNotification" && eventStatus) {
      set((state) => {
        const timelines = { ...state.timelines };
        for (const column of columns.filter(
          (column) => column.columnType === "notification",
        )) {
          const limit = shouldLimitTimelineDisplay(state, column)
            ? timelineDisplayLimit(column)
            : Number.MAX_SAFE_INTEGER;
          timelines[column.id] = [
            eventStatus,
            ...(timelines[column.id] ?? []),
          ].slice(0, limit);
        }
        return { timelines };
      });
      return;
    }

    if (event.kind === "deleteStatus" && event.statusId) {
      set((state) => {
        const timelines = { ...state.timelines };
        for (const column of columns) {
          const current = timelines[column.id];
          if (!current) continue;
          timelines[column.id] = current.filter(
            (status) =>
              !(
                status.serverDomain === event.serverDomain &&
                (status.originalStatusId === event.statusId ||
                  status.id === event.statusId)
              ),
          );
        }
        return { timelines };
      });
      refreshSqlBackedColumns(columns, event);
      return;
    }

    if (!eventStatus) return;

    const directColumns = columns.filter(
      (column) =>
        columnMatchesEventAccount(column, event.sourceAcct) &&
        (columnReceivesStreamStatus(column, event.streamType) ||
          columnContainsStatus(column, eventStatus)),
    );
    if (directColumns.length > 0) {
      set((state) => {
        const timelines = { ...state.timelines };
        for (const column of directColumns) {
          const limit = shouldLimitTimelineDisplay(state, column)
            ? timelineDisplayLimit(column)
            : Number.MAX_SAFE_INTEGER;
          if (!statusMatchesDisplayFilter(eventStatus, column)) {
            timelines[column.id] = (timelines[column.id] ?? []).filter(
              (status) => statusIdentity(status) !== statusIdentity(eventStatus),
            );
            continue;
          }
          timelines[column.id] = mergeStreamStatus(
            timelines[column.id] ?? [],
            eventStatus,
            limit,
            event.kind === "statusUpdate",
          );
        }
        return { timelines };
      });
    }

    refreshSqlBackedColumns(columns, event);
  },
  requestConfirmation: (request) => {
    pendingConfirmationResolve?.(false);
    return new Promise((resolve) => {
      pendingConfirmationResolve = resolve;
      set({
        confirmationDialog: {
          ...request,
          id: crypto.randomUUID(),
        },
      });
    });
  },
  resolveConfirmation: (confirmed) => {
    const resolve = pendingConfirmationResolve;
    pendingConfirmationResolve = undefined;
    set({ confirmationDialog: undefined });
    resolve?.(confirmed);
  },
}));

const EMPTY_TIMELINE_STATUSES: TimelineStatus[] = [];

function timelineDisplayLimit(column: ColumnSummary) {
  return Math.max(1, Number(column.maxStatuses) || 100);
}

function shouldLimitTimelineDisplay(
  state: Pick<AppStore, "timelineNearTop">,
  column: ColumnSummary,
) {
  return state.timelineNearTop[column.id] ?? true;
}

function columnReceivesStreamStatus(column: ColumnSummary, streamType: string) {
  if (column.columnType === "home") return streamType === "user";
  if (column.columnType === "public") return streamType === "public";
  if (column.columnType === "local") return streamType === "public:local";
  if (column.columnType === "hashtag")
    return streamType === `hashtag:${column.columnParam}`;
  return false;
}

function columnMatchesEventAccount(column: ColumnSummary, sourceAcct: string) {
  if (!["local", "list", "hashtag"].includes(column.columnType)) return true;
  return !column.accountAcct || column.accountAcct === sourceAcct;
}

function columnContainsStatus(column: ColumnSummary, status: TimelineStatus) {
  return (useAppStore.getState().timelines[column.id] ?? []).some(
    (item) => statusIdentity(item) === statusIdentity(status),
  );
}

function statusMatchesDisplayFilter(
  status: TimelineStatus,
  column: ColumnSummary,
) {
  if (!timelineDisplayFilterApplies(column)) return true;
  const filter = normalizeDisplayFilter(column.displayFilter);
  const isBoost =
    status.id !== status.originalStatusId ||
    Boolean(status.notificationLabel?.includes("boosted"));
  const hasMedia = status.media.length > 0;
  if (filter.excludeBoosts && isBoost) return false;
  if (filter.excludeMedia && hasMedia) return false;
  if (filter.includeMedia && !hasMedia) return false;
  return true;
}

function filterTimelineStatusesForColumn(
  statuses: TimelineStatus[],
  column: ColumnSummary,
) {
  if (!timelineDisplayFilterApplies(column)) return statuses;
  return statuses.filter((status) => statusMatchesDisplayFilter(status, column));
}

function refreshSqlBackedColumns(
  columns: ColumnSummary[],
  event: TimelineStreamEvent,
) {
  const { loadTimeline } = useAppStore.getState();
  for (const column of columns) {
    if (!columnMatchesEventAccount(column, event.sourceAcct)) continue;
    if (!columnShouldRefetchFromSql(column, event)) continue;
    void loadTimeline(column, true, { delta: event.kind === "newStatus" });
  }
}

function columnShouldRefetchFromSql(
  column: ColumnSummary,
  event: TimelineStreamEvent,
) {
  if (
    column.columnType === "yq" ||
    column.columnType === "custom" ||
    column.columnType === "search" ||
    column.columnType === "thread"
  ) {
    return (
      event.kind === "deleteStatus" ||
      event.kind === "newStatus" ||
      event.kind === "statusUpdate"
    );
  }
  if (column.columnType === "list") {
    return event.streamType === `list:${column.columnParam}`;
  }
  return false;
}

function mergeStreamStatus(
  current: TimelineStatus[],
  status: TimelineStatus,
  limit: number,
  updateOnly: boolean,
) {
  const key = statusIdentity(status);
  const exists = current.some((item) => statusIdentity(item) === key);
  if (updateOnly && !exists) return current;

  const merged = exists
    ? current.map((item) => (statusIdentity(item) === key ? status : item))
    : [status, ...current];

  return merged
    .filter(
      (item, index, items) =>
        items.findIndex(
          (candidate) => statusIdentity(candidate) === statusIdentity(item),
        ) === index,
    )
    .sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt))
    .slice(0, limit);
}

export function statusIdentity(status: TimelineStatus) {
  if (status.notificationId) {
    return `${status.serverDomain}:notification:${status.notificationId}`;
  }
  const uri = status.uri?.trim();
  if (uri) return `uri:${uri}`;
  return `${status.serverDomain}:status:${status.id}`;
}

function latestTimelineStatus(statuses: TimelineStatus[]) {
  if (statuses.length === 0) return undefined;
  return statuses.reduce((latest, status) =>
    Date.parse(status.createdAt) > Date.parse(latest.createdAt)
      ? status
      : latest,
  );
}

function oldestTimelineStatus(statuses: TimelineStatus[]) {
  if (statuses.length === 0) return undefined;
  return statuses.reduce((oldest, status) =>
    Date.parse(status.createdAt) < Date.parse(oldest.createdAt)
      ? status
      : oldest,
  );
}

function timelinePageLimit(column: ColumnSummary) {
  const maxLimit =
    column.columnType === "thread"
      ? 300
      : column.columnType === "airContext"
        ? 2
        : 120;
  return Math.min(maxLimit, timelineDisplayLimit(column));
}

function columnHasSqlLimit(column: ColumnSummary) {
  return (
    column.columnType === "custom" &&
    hasTopLevelSqlLimit(column.columnParam ?? "")
  );
}

function columnCanLoadMoreFromApi(column: ColumnSummary) {
  return ["local", "list", "hashtag"].includes(column.columnType);
}

function threadColumnParam(status: TimelineStatus) {
  return JSON.stringify({
    statusId: status.originalStatusId || status.id,
    serverDomain: status.serverDomain,
  });
}

function airContextColumnParam(status: TimelineStatus) {
  return JSON.stringify({
    statusId: status.originalStatusId || status.id,
    serverDomain: status.serverDomain,
    accountId: status.notificationAccountId,
    accountAcct: status.notificationAcct,
  });
}

function userBookmarksColumnParam(target: UserProfileTarget) {
  return JSON.stringify({
    accountId: target.accountId,
    serverDomain: target.serverDomain,
  });
}

function parseThreadColumnParam(columnParam?: string | null) {
  if (!columnParam) throw new Error(t("Thread target is missing"));
  const parsed = JSON.parse(columnParam) as {
    statusId?: unknown;
    serverDomain?: unknown;
  };
  if (
    typeof parsed.statusId !== "string" ||
    typeof parsed.serverDomain !== "string" ||
    !parsed.statusId ||
    !parsed.serverDomain
  ) {
    throw new Error(t("Thread target is invalid"));
  }
  return {
    statusId: parsed.statusId,
    serverDomain: parsed.serverDomain,
  };
}

function parseAirContextColumnParam(columnParam?: string | null) {
  if (!columnParam) throw new Error(t("AIR context target is missing"));
  const parsed = JSON.parse(columnParam) as {
    statusId?: unknown;
    serverDomain?: unknown;
    accountId?: unknown;
    accountAcct?: unknown;
  };
  if (
    typeof parsed.statusId !== "string" ||
    typeof parsed.serverDomain !== "string" ||
    typeof parsed.accountId !== "string" ||
    !parsed.statusId ||
    !parsed.serverDomain ||
    !parsed.accountId
  ) {
    throw new Error(t("AIR context target is invalid"));
  }
  return {
    statusId: parsed.statusId,
    serverDomain: parsed.serverDomain,
    accountId: parsed.accountId,
    accountAcct:
      typeof parsed.accountAcct === "string" ? parsed.accountAcct : undefined,
  };
}

function timelinePageHasMore(
  pageLength: number,
  requestedLimit: number,
  loadedViaRefresh = false,
) {
  if (pageLength === 0) return false;
  const responseLimit = loadedViaRefresh
    ? Math.min(requestedLimit, 80)
    : requestedLimit;
  return pageLength >= responseLimit;
}

function mergeTimelinePage(
  column: ColumnSummary,
  current: TimelineStatus[],
  nextPage: TimelineStatus[],
) {
  const filteredCurrent = filterTimelineStatusesForColumn(current, column);
  const seen = new Set(filteredCurrent.map(statusIdentity));
  const appended = nextPage.filter((status) => {
    const identity = statusIdentity(status);
    if (seen.has(identity)) return false;
    seen.add(identity);
    return true;
  });
  return [...filteredCurrent, ...appended];
}

function mergeTimelineDelta(
  column: ColumnSummary,
  current: TimelineStatus[],
  delta: TimelineStatus[],
  limit?: number,
) {
  const filteredCurrent = filterTimelineStatusesForColumn(current, column);
  if (delta.length === 0) return filteredCurrent;
  const merged = [...delta, ...filteredCurrent];
  const result = merged
    .filter(
      (item, index, items) =>
        items.findIndex(
          (candidate) => statusIdentity(candidate) === statusIdentity(item),
        ) === index,
    )
    .sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  return limit === undefined ? result : result.slice(0, limit);
}

function mergeTimelineLoadPage(
  column: ColumnSummary,
  loaded: TimelineStatus[],
  current: TimelineStatus[],
  limit?: number,
) {
  const filteredCurrent = filterTimelineStatusesForColumn(current, column);
  if (!columnReceivesRealtimeStatuses(column) || current.length === 0) {
    return limit === undefined ? loaded : loaded.slice(0, limit);
  }

  const seen = new Set(loaded.map(statusIdentity));
  const oldestLoadedTime =
    loaded.length > 0
      ? Math.min(...loaded.map((status) => Date.parse(status.createdAt)))
      : Number.NEGATIVE_INFINITY;
  const streamed = filteredCurrent.filter((status) => {
    if (seen.has(statusIdentity(status))) return false;
    return Date.parse(status.createdAt) >= oldestLoadedTime;
  });

  if (streamed.length === 0) {
    return limit === undefined ? loaded : loaded.slice(0, limit);
  }
  const result = [...loaded, ...streamed].sort(
    (a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt),
  );
  return limit === undefined ? result : result.slice(0, limit);
}

function mergeActionStatusIntoTimeline(
  current: TimelineStatus[],
  selected: TimelineStatus,
  updated: TimelineStatus,
  action: string,
  limit: number,
) {
  const updatedWithSource = {
    ...updated,
    sourceAcct: updated.sourceAcct ?? selected.sourceAcct,
  };
  const merged = current.map((item) =>
    item.id === selected.id
      ? mergeUpdatedStatusIntoTimelineItem(item, updatedWithSource)
      : item,
  );

  if (action === "reblog") {
    return mergeStreamStatus(merged, updatedWithSource, limit, false);
  }

  return merged;
}

function columnReceivesRealtimeStatuses(column: ColumnSummary) {
  return (
    column.columnType === "home" ||
    column.columnType === "public" ||
    column.columnType === "local" ||
    column.columnType === "list" ||
    column.columnType === "hashtag"
  );
}

function updateStatusAcrossTimelines(
  timelines: Record<string, TimelineStatus[]>,
  status: TimelineStatus,
  updated: TimelineStatus,
) {
  return Object.fromEntries(
    Object.entries(timelines).map(([columnId, statuses]) => [
      columnId,
      statuses.map((item) =>
        isSameOriginalStatus(item, status)
          ? mergeUpdatedStatusIntoTimelineItem(item, updated)
          : item,
      ),
    ]),
  );
}

function removeStatusAcrossTimelines(
  timelines: Record<string, TimelineStatus[]>,
  status: TimelineStatus,
) {
  return Object.fromEntries(
    Object.entries(timelines).map(([columnId, statuses]) => [
      columnId,
      statuses.filter((item) => !isSameOriginalStatus(item, status)),
    ]),
  );
}

function updatePollAcrossTimelines(
  timelines: Record<string, TimelineStatus[]>,
  status: TimelineStatus,
  poll: PollSummary,
) {
  return Object.fromEntries(
    Object.entries(timelines).map(([columnId, statuses]) => [
      columnId,
      statuses.map((item) =>
        isSameOriginalStatus(item, status) ? { ...item, poll } : item,
      ),
    ]),
  );
}

function isSameOriginalStatus(left: TimelineStatus, right: TimelineStatus) {
  return (
    left.serverDomain === right.serverDomain &&
    (left.originalStatusId || left.id) === (right.originalStatusId || right.id)
  );
}

function mergeUpdatedStatusIntoTimelineItem(
  current: TimelineStatus,
  updated: TimelineStatus,
) {
  const updatedWithSource = {
    ...updated,
    sourceAcct: updated.sourceAcct ?? current.sourceAcct,
  };
  const preservesExistingTimelineEvent =
    Boolean(current.notificationId) ||
    (current.originalStatusId === updated.originalStatusId &&
      (current.uri !== updated.uri || current.id !== updated.id));

  if (!preservesExistingTimelineEvent) return updatedWithSource;

  return {
    ...updatedWithSource,
    id: current.id,
    uri: current.uri,
    originalStatusId: current.originalStatusId,
    createdAt: current.createdAt,
    sourceAcct: current.sourceAcct ?? updated.sourceAcct,
    notificationId: current.notificationId,
    notificationLabel: current.notificationLabel,
    notificationAvatar: current.notificationAvatar,
    notificationAccountId: current.notificationAccountId,
    notificationAcct: current.notificationAcct,
    notificationDisplayName: current.notificationDisplayName,
    notificationAccountEmojis: current.notificationAccountEmojis,
  };
}
