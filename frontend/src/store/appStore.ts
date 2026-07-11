import { create } from "zustand";
import type {
  AccountSummary,
  AppStartupProgressEvent,
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
  SettingsSnapshot,
  StatusBarSnapshot,
  TimelinePageResponse,
  TimelineRequest,
  TimelineStatus,
  TimelineStreamEvent,
  UserProfileTarget,
  VotePollRequest,
} from "../types/app";
import { invokeCommand, invokeReadCommand } from "../api/tauri";
import { isResponseLossError } from "../api/ipcErrors";
import {
  recordFrontendStreamGap,
  recordFrontendStreamResync,
  setPendingStreamEvents,
} from "../api/observability";
import {
  canonicalStatusKey,
  clampTimelineLimit,
  createTimelineEntityState,
  reduceTimelineEntities,
  statusForCanonical,
  statusKey,
  type StatusKey,
  type TimelineEntityOperation,
} from "../domain/timelineEntities";
import { ConfirmationQueue } from "../domain/confirmationQueue";
import {
  MutationLifecycle,
  type MutationRunOptions,
  type MutationState,
} from "../domain/mutationLifecycle";
import {
  SettingsMutationCoordinator,
  type SettingSaveState,
} from "../domain/settingsMutations";
import { confirmStatusAction } from "../utils/confirmation";
import { clearBlurHashCache } from "../utils/blurhash";
import {
  createColumn,
  normalizeColumns,
  normalizeDisplayFilter,
  reconcileActiveTabs,
  timelineDisplayFilterApplies,
} from "../utils/columns";
import { htmlToPlainText } from "../utils/format";
import { previewMediaSources } from "../utils/media";
import { clearMediaRetryCache } from "../utils/mediaRetryCoordinator";
import {
  frontendRequestScheduler,
  RequestCancelledError,
  type RequestLaneMetrics,
} from "../utils/requestScheduler";
import { hasTopLevelSqlLimit } from "../utils/sql";
import { matchPresetVisibility } from "../utils/visibility";
import { t } from "../i18n";
import { timelineDescriptor } from "../domain/timelineDescriptors";
import {
  reduceOpenOrFocusDynamicPane,
  type DynamicPaneDescriptor,
} from "./slices/panes";
import {
  reduceResourceStates,
  type ResourcePhase,
  type ResourceState,
} from "./slices/resources";
import {
  applyPersistedSetting,
  reduceSettingDraft,
  settingKeys,
  settingValue,
} from "./slices/settingsDraft";
import {
  initialComposeSlice,
  reduceComposeSlice,
  type ComposeTarget,
  type ComposeVisibility,
} from "./slices/compose";
import { reduceOverlaySlice } from "./slices/overlays";
import {
  clearUnreadResource,
  incrementUnreadResources,
} from "./slices/notifications";
import {
  initialBootState,
  type BootState,
} from "./slices/session";
import { createSessionActions } from "./actions/sessionActions";

type LoadTimelineOptions = {
  delta?: boolean;
};

type PendingTimelineRefresh = {
  column: ColumnSummary;
  options: LoadTimelineOptions;
};

type EditStatusOptions = {
  visibility?: string | null;
  spoilerText?: string | null;
  sensitive?: boolean | null;
};

export type StatusMutationState = {
  operationId: string;
  phase: "pending" | "confirmed" | "uncertain" | "failed";
  beforeImage: TimelineStatus;
  error?: string;
};

export type StreamPerformanceSnapshot = {
  batches: number;
  lastBatchSize: number;
  lastDurationMs: number;
  p95DurationMs: number;
};

export type AsyncResourcePhase = ResourcePhase;
export type AsyncResourceState = ResourceState;

const composeVisibilityValues = new Set([
  "public",
  "unlisted",
  "private",
  "direct",
]);

function normalizeComposeVisibility(
  value: string | null | undefined,
): AppStore["visibility"] | null {
  if (!value) return null;
  const normalized = value.toLowerCase();
  return composeVisibilityValues.has(normalized)
    ? (normalized as AppStore["visibility"])
    : null;
}

export type AppStore = {
  boot: BootState;
  snapshot?: AppSnapshot;
  entities: Map<StatusKey, TimelineStatus>;
  timelineKeys: Record<string, StatusKey[]>;
  canonicalIndex: Map<StatusKey, Set<StatusKey>>;
  timelines: Record<string, TimelineStatus[]>;
  timelineUnread: Record<string, number>;
  statusMutations: Record<StatusKey, StatusMutationState>;
  resourceStates: Record<string, AsyncResourceState>;
  settingMutations: Record<string, SettingSaveState>;
  mutationStates: Record<string, MutationState>;
  requestMetrics: Record<"timeline" | "profile" | "autocomplete", RequestLaneMetrics>;
  streamPerformance: StreamPerformanceSnapshot;
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
  composeTarget?: ComposeTarget | null;
  visibility: ComposeVisibility;
  mediaPreview?: MediaPreviewState | null;
  confirmationDialog?: ConfirmationDialogState;
  loadSnapshot: () => Promise<void>;
  applyStartupProgress: (progress: AppStartupProgressEvent) => void;
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
  replaceTimelineSlice: (
    sliceId: string,
    statuses: TimelineStatus[],
    limit: number,
  ) => void;
  removeTimelineSlices: (sliceIds: string[]) => void;
  setActiveTab: (paneIndex: number, column: ColumnSummary) => void;
  addBookmarksPane: () => void;
  addFavouritesPane: () => void;
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
  beginEditStatus: (status: TimelineStatus) => void;
  clearComposeTarget: () => void;
  action: (
    column: ColumnSummary,
    status: TimelineStatus,
    action: string,
  ) => Promise<void>;
  actionStatus: (
    status: TimelineStatus,
    action: string,
    confirm?: boolean,
  ) => Promise<void>;
  votePoll: (status: TimelineStatus, choices: number[]) => Promise<PollSummary | null>;
  editStatus: (
    status: TimelineStatus,
    content: string,
    options?: EditStatusOptions,
  ) => Promise<TimelineStatus | null>;
  deleteStatus: (status: TimelineStatus) => Promise<boolean>;
  switchAccount: (acct: string) => Promise<void>;
  logoutAccount: (acct: string) => Promise<void>;
  saveSetting: (key: string, value: unknown) => Promise<void>;
  flushSettingSaves: () => Promise<void>;
  saveColumns: (columns: ColumnSummary[]) => Promise<void>;
  applyStreamEvent: (event: TimelineStreamEvent) => void;
  requestConfirmation: (
    request: ConfirmationDialogRequest,
  ) => Promise<boolean>;
  resolveConfirmation: (id: string, confirmed: boolean) => void;
  cancelConfirmation: (id: string) => void;
  runMutation: <T>(
    key: string,
    options: MutationRunOptions<T>,
  ) => Promise<T | undefined>;
};

export type { BootState } from "./slices/session";

export type SettingsSection =
  | "Account"
  | "Appearance"
  | "Behavior"
  | "Performance"
  | "Notification"
  | "Timeline"
  | "Sidecar"
  | "Database"
  | "Debug"
  | "About";

const inFlightTimelineLoads = new Map<string, Promise<void>>();
const pendingTimelineRefreshes = new Map<string, PendingTimelineRefresh>();
const timelineLoadSignatures = new Map<string, string>();

function timelineLogContext(column: ColumnSummary) {
  const accountScope = isUnifiedTimelineColumn(column)
    ? "unified"
    : isGlobalSQLiteTimelineColumn(column)
      ? "sqlite"
      : (column.accountAcct ?? "all");
  return `column=${column.id} type=${column.columnType} account=${accountScope} dynamic=${Boolean(column.dynamic)}`;
}

function timelineColumnSignature(column: ColumnSummary) {
  return JSON.stringify([
    column.columnType,
    column.columnParam ?? null,
    timelineRequestAccountAcct(column) ?? null,
    column.displayFilter ?? null,
    column.maxStatuses,
  ]);
}

function isVisibleTimelineColumn(
  state: Pick<AppStore, "activeTabs">,
  column: ColumnSummary,
) {
  return (state.activeTabs[column.paneIndex] ?? column.id) === column.id;
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

function seedSettingsCoordinator(
  coordinator: SettingsMutationCoordinator<SettingsSnapshot>,
  settings: SettingsSnapshot,
) {
  for (const key of settingKeys) {
    coordinator.seed(key, settingValue(settings, key));
  }
}

function cancelAccountScopedFrontendWork() {
  frontendRequestScheduler.cancelAll();
  pendingTimelineRefreshes.clear();
  timelineLoadSignatures.clear();
}

function clearAccountScopedCaches() {
  clearBlurHashCache();
  clearMediaRetryCache();
}

function requiredActingAccount(state: AppStore) {
  const acct = state.snapshot?.activeAcct?.trim();
  if (!acct) throw new Error(t("No active account is signed in"));
  const account = state.snapshot?.accounts.find((candidate) => candidate.acct === acct);
  if (!account) throw new Error(t("No active account is signed in"));
  return account;
}

function statusActionCapability(
  account: AccountSummary,
  action: string,
) {
  if (action.includes("favourite")) return account.capabilities.status.favourite;
  if (action.includes("reblog")) return account.capabilities.status.reblog;
  if (action.includes("bookmark")) return account.capabilities.status.bookmark;
  return false;
}

function appStoreTimelineInitialState() {
  const state = createTimelineEntityState();
  return {
    entities: state.entities,
    timelineKeys: state.columnKeys,
    canonicalIndex: state.canonicalIndex,
    timelines: state.timelines,
  };
}

function timelineEntityPatch(
  state: Pick<
    AppStore,
    "entities" | "timelineKeys" | "canonicalIndex" | "timelines"
  >,
  operations: TimelineEntityOperation[],
) {
  const next = reduceTimelineEntities(
    {
      entities: state.entities,
      columnKeys: state.timelineKeys,
      canonicalIndex: state.canonicalIndex,
      timelines: state.timelines,
    },
    operations,
  );
  return {
    entities: next.entities,
    timelineKeys: next.columnKeys,
    canonicalIndex: next.canonicalIndex,
    timelines: next.timelines,
  };
}

export const useAppStore = create<AppStore>((set, get) => {
  const confirmationQueue = new ConfirmationQueue((confirmationDialog) => {
    set((state) =>
      reduceOverlaySlice(state, {
        type: "showConfirmation",
        dialog: confirmationDialog,
      }),
    );
  });
  const settingsCoordinator = new SettingsMutationCoordinator<SettingsSnapshot>({
    debounceMs: 400,
    persist: (key, value) =>
      invokeCommand<SettingsSnapshot>("save_settings", {
        request: { key, value },
      }),
    onState: (mutation) => {
      set((state) => ({
        settingMutations: {
          ...state.settingMutations,
          [mutation.key]: mutation,
        },
        ...(mutation.phase === "failed" ? { error: mutation.error } : {}),
        ...(mutation.phase === "saved"
          ? { statusMessage: t("Settings saved") }
          : {}),
      }));
    },
    onPersisted: (key, persisted) => {
      set((state) =>
        state.snapshot
          ? {
              snapshot: {
                ...state.snapshot,
                settings: applyPersistedSetting(
                  state.snapshot.settings,
                  persisted,
                  key,
                ),
              },
            }
          : {},
      );
    },
  });
  const mutationLifecycle = new MutationLifecycle((mutation) => {
    set((state) => ({
      mutationStates: {
        ...state.mutationStates,
        [mutation.key]: mutation,
      },
      ...(mutation.phase === "failed" || mutation.phase === "uncertain"
        ? { error: mutation.error }
        : {}),
      ...(mutation.phase === "pending"
        ? { statusMessage: t("Working") }
        : mutation.phase === "succeeded"
          ? { statusMessage: t("Completed") }
          : {}),
    }));
  });
  const openOrFocusDynamicPane = (
    descriptor: DynamicPaneDescriptor,
    { load = true }: { load?: boolean } = {},
  ) => {
    const current = get();
    const result = reduceOpenOrFocusDynamicPane(
      {
        persistedColumns: current.snapshot?.columns ?? [],
        dynamicColumns: current.dynamicColumns,
        activeTabs: current.activeTabs,
        pendingScrollPaneIndex: current.pendingScrollPaneIndex,
      },
      descriptor,
    );
    set(result.state);
    if (load && !get().timelines[result.column.id]) {
      void get().loadTimeline(result.column);
    }
    return result.column;
  };

  return {
  boot: initialBootState(),
  ...appStoreTimelineInitialState(),
  timelineUnread: {},
  statusMutations: {},
  resourceStates: {},
  settingMutations: {},
  mutationStates: {},
  requestMetrics: frontendRequestScheduler.metrics(),
  streamPerformance: {
    batches: 0,
    lastBatchSize: 0,
    lastDurationMs: 0,
    p95DurationMs: 0,
  },
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
  ...initialComposeSlice(),
  mediaPreview: null,
  confirmationDialog: undefined,
  ...createSessionActions({
    set,
    get,
    settingsCoordinator,
    mutationLifecycle,
    confirmationQueue,
    seedSettingsCoordinator,
    cancelAccountScopedFrontendWork,
    clearAccountScopedCaches,
    appStoreTimelineInitialState,
    isUncertainMutationError,
  }),
  loadTimeline: async (column, refresh = false, options = {}) => {
    if (!timelineDescriptor(column.columnType)) {
      const resourceKey = `timeline:${column.id}`;
      set((state) => ({
        loading: { ...state.loading, [column.id]: false },
        resourceStates: reduceResourceStates(state.resourceStates, {
          type: "fail",
          key: resourceKey,
          generation: (state.resourceStates[resourceKey]?.generation ?? 0) + 1,
          error: t("timeline.unsupported", { type: column.columnType }),
        }),
      }));
      return;
    }
    const signature = timelineColumnSignature(column);
    const inFlight = inFlightTimelineLoads.get(column.id);
    if (inFlight) {
      const resourceChanged = timelineLoadSignatures.get(column.id) !== signature;
      if (refresh || resourceChanged) {
        queuePendingTimelineRefresh(column, options);
        if (resourceChanged) {
          frontendRequestScheduler.cancel(`timeline:${column.id}`);
        }
        console.debug(
          `[awayuki][ui-timeline] queued ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(options.delta)} reason=${resourceChanged ? "resource_changed" : "in_flight"}`,
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
      const requestAccountAcct = timelineRequestAccountAcct(column);
      const request: TimelineRequest = {
        columnType: column.columnType,
        columnParam: column.columnParam,
        limit,
        ...(requestAccountAcct ? { accountAcct: requestAccountAcct } : {}),
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
      const resourceKey = `timeline:${column.id}`;
      let resourceGeneration = 0;
      set((state) => ({ loading: { ...state.loading, [column.id]: true } }));
      try {
        const statuses = await frontendRequestScheduler.schedule(
          {
            key: resourceKey,
            lane: "timeline",
            priority: isVisibleTimelineColumn(get(), column) ? 100 : 0,
            replace: false,
          },
          async (context) => {
            resourceGeneration = context.generation;
            set((state) => ({
              resourceStates: reduceResourceStates(state.resourceStates, {
                type: "begin",
                key: resourceKey,
                generation: context.generation,
                refreshing: refresh,
              }),
            }));
            const loadStrategy = timelineDescriptor(
              column.columnType,
            )?.loadStrategy;
            const result =
              loadStrategy === "thread"
                ? await invokeReadCommand<TimelineStatus[]>("status_thread", {
                    request: {
                      ...parseThreadColumnParam(column.columnParam),
                      limit,
                    },
                  })
                : loadStrategy === "airContext"
                  ? await invokeReadCommand<TimelineStatus[]>("air_context", {
                      request: {
                        ...parseAirContextColumnParam(column.columnParam),
                        limit,
                      },
                    })
                  : await invokeReadCommand<TimelineStatus[]>(
                      refresh ? "refresh_timeline" : "load_timeline",
                      { request },
                    );
            if (!context.isCurrent()) throw new RequestCancelledError(resourceKey);
            return result;
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
          if (
            state.resourceStates[resourceKey]?.generation !== resourceGeneration
          ) {
            return {};
          }
          const displayLimit = timelineDisplayLimit(column);
          const current = state.timelines[column.id] ?? [];
          const operation: TimelineEntityOperation = sinceStatus
            ? {
                type: "mergeDelta",
                columnId: column.id,
                statuses: displayStatuses,
                limit: displayLimit,
              }
            : {
                type: "replaceColumn",
                columnId: column.id,
                statuses: mergeTimelineLoadPage(
                  column,
                  displayStatuses,
                  current,
                  displayLimit,
                ),
                limit: displayLimit,
              };
          return {
            ...timelineEntityPatch(state, [operation]),
            loading: { ...state.loading, [column.id]: false },
            resourceStates: reduceResourceStates(state.resourceStates, {
              type: "succeed",
              key: resourceKey,
              generation:
                state.resourceStates[resourceKey]?.generation ??
                resourceGeneration,
            }),
            timelineUnread: clearUnreadResource(
              state.timelineUnread,
              column.id,
            ),
            timelineHasMore: {
              ...state.timelineHasMore,
              [column.id]: sinceStatus
                ? (state.timelineHasMore[column.id] ?? true)
                : timelineDescriptor(column.columnType)?.pagination === "none"
                  ? false
                : columnHasSqlLimit(column)
                  ? false
                  : columnCanLoadMoreFromApi(column) &&
                      timelineDisplayFilterApplies(column)
                    ? true
                  : timelinePageHasMore(statuses.length, limit, refresh),
            },
          };
        });
      } catch (error) {
        const cancelled = error instanceof RequestCancelledError;
        console.info(
          `[awayuki][ui-timeline] error ${timelineLogContext(column)} refresh=${refresh} delta=${Boolean(sinceStatus)} duration_ms=${uiElapsedMs(startedAt)} error=${String(error)}`,
        );
        set((state) =>
          state.resourceStates[resourceKey]?.generation === resourceGeneration
            ? {
                loading: { ...state.loading, [column.id]: false },
                resourceStates: reduceResourceStates(
                  state.resourceStates,
                  cancelled
                    ? {
                        type: "cancel",
                        key: resourceKey,
                        generation: resourceGeneration,
                      }
                    : {
                        type: "fail",
                        key: resourceKey,
                        generation: resourceGeneration,
                        error: String(error),
                      },
                ),
              }
            : {},
        );
      } finally {
        inFlightTimelineLoads.delete(column.id);
        if (timelineLoadSignatures.get(column.id) === signature) {
          timelineLoadSignatures.delete(column.id);
        }
        set({ requestMetrics: frontendRequestScheduler.metrics() });
        const pending = pendingTimelineRefreshes.get(column.id);
        if (pending) {
          pendingTimelineRefreshes.delete(column.id);
          await get().loadTimeline(pending.column, true, pending.options);
        }
      }
    };
    const promise = Promise.resolve().then(run);
    inFlightTimelineLoads.set(column.id, promise);
    timelineLoadSignatures.set(column.id, signature);
    await promise;
  },
  loadMoreTimeline: async (column) => {
    if (
      timelineDescriptor(column.columnType)?.pagination === "none"
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
    const requestAccountAcct = timelineRequestAccountAcct(column);
    const request: TimelineRequest = {
      columnType: column.columnType,
      columnParam: column.columnParam,
      limit,
      offset: current.length,
      maxStatusId: maxStatus?.id,
      maxServerDomain: maxStatus?.serverDomain,
      ...(requestAccountAcct ? { accountAcct: requestAccountAcct } : {}),
      displayFilter: timelineDisplayFilterApplies(column)
        ? normalizeDisplayFilter(column.displayFilter)
        : undefined,
    };
    const startedAt = performance.now();
    const resourceKey = `timeline:${column.id}:more`;
    let resourceGeneration = 0;
    console.info(
      `[awayuki][ui-timeline] load_more_start ${timelineLogContext(column)} offset=${request.offset} limit=${limit}`,
    );
    set((state) => ({
      loadingMore: { ...state.loadingMore, [column.id]: true },
    }));
    try {
      const response = await frontendRequestScheduler.schedule(
        {
          key: resourceKey,
          lane: "timeline",
          priority: isVisibleTimelineColumn(get(), column) ? 90 : -10,
          replace: false,
        },
        async (context) => {
          resourceGeneration = context.generation;
          set((state) => ({
            resourceStates: reduceResourceStates(state.resourceStates, {
              type: "begin",
              key: resourceKey,
              generation: context.generation,
            }),
          }));
          const page = columnCanLoadMoreFromApi(column)
            ? await invokeReadCommand<TimelinePageResponse>(
                "load_more_timeline",
                { request },
              )
            : {
                statuses: await invokeReadCommand<TimelineStatus[]>(
                  "load_timeline",
                  { request },
                ),
                hasMore: undefined,
              };
          if (!context.isCurrent()) throw new RequestCancelledError(resourceKey);
          return page;
        },
      );
      const nextPage = response.statuses;
      const displayNextPage = filterTimelineStatusesForColumn(nextPage, column);
      console.info(
        `[awayuki][ui-timeline] load_more_success ${timelineLogContext(column)} offset=${request.offset} count=${nextPage.length} display_count=${displayNextPage.length} duration_ms=${uiElapsedMs(startedAt)}`,
      );
      set((state) =>
        state.resourceStates[resourceKey]?.generation === resourceGeneration
          ? {
              ...timelineEntityPatch(state, [
                {
                  type: "appendPage",
                  columnId: column.id,
                  statuses: displayNextPage,
                  limit: timelineDisplayLimit(column),
                },
              ]),
              loadingMore: { ...state.loadingMore, [column.id]: false },
              resourceStates: reduceResourceStates(state.resourceStates, {
                type: "succeed",
                key: resourceKey,
                generation: resourceGeneration,
              }),
              timelineHasMore: {
                ...state.timelineHasMore,
                [column.id]:
                  typeof response.hasMore === "boolean"
                    ? response.hasMore
                    : timelinePageHasMore(nextPage.length, limit),
              },
            }
          : {},
      );
    } catch (error) {
      const cancelled = error instanceof RequestCancelledError;
      console.info(
        `[awayuki][ui-timeline] load_more_error ${timelineLogContext(column)} offset=${request.offset} duration_ms=${uiElapsedMs(startedAt)} error=${String(error)}`,
      );
      set((state) =>
        state.resourceStates[resourceKey]?.generation === resourceGeneration
          ? {
              loadingMore: { ...state.loadingMore, [column.id]: false },
              resourceStates: reduceResourceStates(
                state.resourceStates,
                cancelled
                  ? {
                      type: "cancel",
                      key: resourceKey,
                      generation: resourceGeneration,
                    }
                  : {
                      type: "fail",
                      key: resourceKey,
                      generation: resourceGeneration,
                      error: String(error),
                    },
              ),
            }
          : {},
      );
    } finally {
      set({ requestMetrics: frontendRequestScheduler.metrics() });
    }
  },
  setTimelineNearTop: (column, nearTop) => {
    const currentNearTop = get().timelineNearTop[column.id] ?? true;
    if (currentNearTop === nearTop) return;
    const shouldRefresh =
      nearTop &&
      !currentNearTop &&
      (get().timelineUnread[column.id] ?? 0) > 0;
    set((state) => {
      return {
        timelineNearTop: {
          ...state.timelineNearTop,
          [column.id]: nearTop,
        },
      };
    });
    if (shouldRefresh) void get().loadTimeline(column, true, { delta: true });
  },
  trimTimelineToMaxStatuses: (column) => {
    set((state) => {
      const current = state.timelines[column.id] ?? EMPTY_TIMELINE_STATUSES;
      const limit = timelineDisplayLimit(column);
      return {
        ...(current.length > limit
          ? timelineEntityPatch(state, [
              {
                type: "replaceColumn",
                columnId: column.id,
                statuses: current,
                limit,
              },
            ])
          : {}),
        timelineNearTop: {
          ...state.timelineNearTop,
          [column.id]: true,
        },
        timelineUnread: clearUnreadResource(state.timelineUnread, column.id),
      };
    });
  },
  replaceTimelineSlice: (sliceId, statuses, limit) => {
    set((state) => ({
      ...timelineEntityPatch(state, [
        {
          type: "replaceColumn",
          columnId: sliceId,
          statuses,
          limit,
        },
      ]),
    }));
  },
  removeTimelineSlices: (sliceIds) => {
    set((state) => ({
      ...timelineEntityPatch(
        state,
        sliceIds.map((columnId) => ({
          type: "removeColumn" as const,
          columnId,
        })),
      ),
    }));
  },
  setActiveTab: (paneIndex, column) => {
    set((state) => ({
      activeTabs: { ...state.activeTabs, [paneIndex]: column.id },
    }));
    if (!get().timelines[column.id]) void get().loadTimeline(column);
  },
  addBookmarksPane: () => {
    openOrFocusDynamicPane({
      resourceKey: "bookmarks:",
      column: createColumn(0, 0, "bookmarks"),
    });
  },
  addFavouritesPane: () => {
    openOrFocusDynamicPane({
      resourceKey: "favourites:",
      column: createColumn(0, 0, "favourites"),
    });
  },
  openUserBookmarksPane: (target) => {
    if (!target.accountId || !target.serverDomain) return;
    const columnParam = userBookmarksColumnParam(target);
    const acct = target.acct || target.accountId;
    openOrFocusDynamicPane({
      resourceKey: `user_bookmarks:${columnParam}`,
      column: {
        ...createColumn(0, 0, "user_bookmarks"),
        columnParam,
        name: t("Bookmarks by {acct}", {
          acct: `@${acct.replace(/^@/, "")}`,
        }),
        maxStatuses: 100,
      },
    });
  },
  openSearchPane: (rawQuery) => {
    const query = rawQuery.trim();
    if (!query) return;

    const yqMode = query.startsWith("?");
    const columnType = yqMode ? "yq" : "search";
    const columnParam = yqMode ? query.slice(1).trim() : query;
    if (!columnParam) return;

    const namePrefix = yqMode ? "YQ" : t("Search");
    const shortQuery =
      columnParam.length > 40 ? `${columnParam.slice(0, 39)}...` : columnParam;
    openOrFocusDynamicPane({
      resourceKey: `${columnType}:${columnParam}`,
      column: {
        ...createColumn(0, 0, columnType),
        columnParam,
        name: `${namePrefix}: ${shortQuery}`,
        maxStatuses: 100,
      },
    });
  },
  openThreadPane: (status) => {
    const statusId = status.originalStatusId || status.id;
    if (!statusId || !status.serverDomain) return;

    const columnParam = threadColumnParam(status);
    openOrFocusDynamicPane({
      resourceKey: `thread:${columnParam}`,
      column: {
        ...createColumn(0, 0, "thread"),
        columnParam,
        name: t("Thread"),
        maxStatuses: 240,
      },
    });
  },
  openAirContextPane: (status) => {
    const statusId = status.originalStatusId || status.id;
    const accountId = status.notificationAccountId;
    if (!statusId || !status.serverDomain || !accountId) return;

    const columnParam = airContextColumnParam(status);
    openOrFocusDynamicPane({
      resourceKey: `airContext:${columnParam}`,
      column: {
        ...createColumn(0, 0, "airContext"),
        columnParam,
        name: t("AIR context"),
        maxStatuses: 2,
      },
    });
  },
  openUserPane: (status) => {
    const target: UserProfileTarget = {
      accountId: status.accountId,
      serverDomain: status.serverDomain,
      acct: status.acct,
      displayName: status.displayName,
      avatar: status.avatar,
    };
    openOrFocusDynamicPane(
      {
        resourceKey: `profile:${target.serverDomain}:${target.accountId}`,
        column: {
          ...createColumn(0, 0, "profile"),
          name: target.acct,
          maxStatuses: 80,
          profile: target,
        },
        updateExisting: (current) => ({
          name: target.acct || current.name,
          profile: {
            ...target,
            acct: target.acct || current.profile?.acct || "",
            displayName:
              target.displayName || current.profile?.displayName || "",
            avatar: target.avatar || current.profile?.avatar || "",
          },
        }),
      },
      { load: false },
    );
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
    const entity =
      get().entities.get(statusKey(status)) ??
      statusForCanonical(get(), status) ??
      status;
    set((state) =>
      reduceOverlaySlice(state, {
        type: "openMedia",
        preview: { status: entity, media, src },
      }),
    );
  },
  closeMediaPreview: () =>
    set((state) => reduceOverlaySlice(state, { type: "closeMedia" })),
  closeDynamicPane: (paneIndex) => {
    const removedColumnIds = get()
      .dynamicColumns.filter((column) => column.paneIndex === paneIndex)
      .map((column) => column.id);
    for (const columnId of removedColumnIds) {
      frontendRequestScheduler.cancelPrefix(`timeline:${columnId}`);
      pendingTimelineRefreshes.delete(columnId);
      timelineLoadSignatures.delete(columnId);
    }
    set((state) => {
      const activeTabs = { ...state.activeTabs };
      const timelineHasMore = { ...state.timelineHasMore };
      const timelineNearTop = { ...state.timelineNearTop };
      const timelineUnread = { ...state.timelineUnread };
      const loading = { ...state.loading };
      const loadingMore = { ...state.loadingMore };
      delete activeTabs[paneIndex];
      const removedColumns = state.dynamicColumns.filter(
        (item) => item.paneIndex === paneIndex,
      );
      for (const column of removedColumns) {
        delete timelineHasMore[column.id];
        delete timelineNearTop[column.id];
        delete timelineUnread[column.id];
        delete loading[column.id];
        delete loadingMore[column.id];
      }
      return {
        ...timelineEntityPatch(
          state,
          removedColumns.map((column) => ({
            type: "removeColumn" as const,
            columnId: column.id,
          })),
        ),
        activeTabs,
        timelineHasMore,
        timelineNearTop,
        timelineUnread,
        loading,
        loadingMore,
        dynamicColumns: state.dynamicColumns.filter(
          (column) => column.paneIndex !== paneIndex,
        ),
      };
    });
  },
  post: async (options = {}) => {
    const { composeText, composeTarget, visibility, snapshot } = get();
    let actingAccount: AccountSummary;
    try {
      actingAccount = requiredActingAccount(get());
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
    const hasMedia = Boolean(options.mediaIds?.length);
    const hasPoll = Boolean(options.poll?.options.length);
    const editing = composeTarget?.kind === "edit";
    if (editing && !composeText.trim()) return false;
    if (!editing && !composeText.trim() && !hasMedia && !hasPoll) return false;
    const resolvedVisibility =
      matchPresetVisibility(snapshot?.settings.presetVisibility, composeText) ??
      visibility;
    try {
      if (editing && composeTarget) {
        const updated = await get().editStatus(composeTarget.status, composeText, {
          visibility: resolvedVisibility,
          spoilerText: Object.prototype.hasOwnProperty.call(
            options,
            "spoilerText",
          )
            ? (options.spoilerText ?? null)
            : composeTarget.status.spoilerText || null,
          sensitive: options.sensitive ?? composeTarget.status.sensitive,
        });
        if (!updated) return false;
        set((state) => reduceComposeSlice(state, { type: "clearDraft" }));
        return true;
      }
      const posted = await mutationLifecycle.run("compose:submit", {
        execute: () =>
          invokeCommand<TimelineStatus>("post_status", {
            request: {
              actingAccountAcct: actingAccount.acct,
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
          }),
        isUncertain: isUncertainMutationError,
      });
      if (!posted) return false;
      set((state) => {
        const columns = allStoreColumns(state).filter(
          (column) =>
            column.columnType === "home" &&
            statusMatchesDisplayFilter(posted, column),
        );
        const preserveAnchorColumns = new Set(
          columns
            .filter((column) => !(state.timelineNearTop[column.id] ?? true))
            .map((column) => column.id),
        );
        return {
          ...reduceComposeSlice(state, { type: "clearDraft" }),
          ...timelineEntityPatch(state, [
            {
              type: "upsertInColumns",
              columnIds: columns.map((column) => column.id),
              status: posted,
              limits: Object.fromEntries(
                columns.map((column) => [
                  column.id,
                  timelineDisplayLimit(column),
                ]),
              ),
              preserveAnchorColumns,
            },
          ]),
          timelineUnread: incrementUnreadResources(
            state.timelineUnread,
            preserveAnchorColumns,
          ),
        };
      });
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
      return reduceComposeSlice(state, {
        type: "setTarget",
        target: { kind: "reply", status },
        text: current ? `${current}\n${mention}` : mention,
      });
    });
    requestAnimationFrame(() =>
      document.getElementById("compose-textarea")?.focus(),
    );
  },
  quoteStatus: (status) => {
    set((state) =>
      reduceComposeSlice(state, {
        type: "setTarget",
        target: { kind: "quote", status },
      }),
    );
    requestAnimationFrame(() =>
      document.getElementById("compose-textarea")?.focus(),
    );
  },
  beginEditStatus: (status) => {
    set((state) => ({
      ...reduceComposeSlice(state, {
        type: "setTarget",
        target: { kind: "edit", status },
        text: htmlToPlainText(status.content),
      }),
      visibility: normalizeComposeVisibility(status.visibility) ?? "public",
    }));
    requestAnimationFrame(() =>
      document.getElementById("compose-textarea")?.focus(),
    );
  },
  clearComposeTarget: () =>
    set((state) => reduceComposeSlice(state, { type: "clearTarget" })),
  action: async (_column, status, action) => {
    await get().actionStatus(status, action, true);
  },
  actionStatus: async (status, action, confirm = true) => {
    const canonical = canonicalStatusKey(status);
    if (get().statusMutations[canonical]?.phase === "pending") return;
    const current = resolvedEntityFor(get(), status) ?? status;
    let actingAccount: AccountSummary;
    try {
      actingAccount = requiredActingAccount(get());
      if (!statusActionCapability(actingAccount, action)) {
        throw new Error(t("This action is not supported by the selected account"));
      }
    } catch (error) {
      set({ error: String(error) });
      return;
    }
    const operationId = crypto.randomUUID();
    set((state) => ({
      statusMutations: {
        ...state.statusMutations,
        [canonical]: {
          operationId,
          phase: "pending",
          beforeImage: current,
        },
      },
    }));
    try {
      if (confirm) {
        const confirmed = await confirmStatusAction(
          get().snapshot?.settings.confirmation,
          get().requestConfirmation,
          current,
          action,
        );
        if (!confirmed) {
          set((state) => {
            if (state.statusMutations[canonical]?.operationId !== operationId) {
              return {};
            }
            const statusMutations = { ...state.statusMutations };
            delete statusMutations[canonical];
            return { statusMutations };
          });
          return;
        }
      }
      const optimisticPatch = optimisticStatusActionPatch(current, action);
      set((state) => {
        const entityPatch = timelineEntityPatch(state, [
          {
            type: "patchCanonical",
            target: current,
            patch: optimisticPatch,
          },
        ]);
        const optimistic =
          resolvedEntityFor(entityPatch, current) ?? {
            ...current,
            ...optimisticPatch,
          };
        return {
          ...entityPatch,
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: "pending",
              beforeImage: current,
            },
          },
          ...syncResolvedStatusConsumers(state, current, optimistic),
        };
      });
      const updated = await invokeCommand<TimelineStatus>("status_action", {
        request: {
          identity: current.statusIdentity,
          actingAccountAcct: actingAccount.acct,
          action,
        },
      });
      set((state) => {
        if (state.statusMutations[canonical]?.operationId !== operationId) {
          return {};
        }
        const entityPatch = timelineEntityPatch(state, [
          { type: "replaceCanonical", target: current, status: updated },
        ]);
        const resolved = resolvedEntityFor(entityPatch, current) ?? updated;
        return {
          ...entityPatch,
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: "confirmed",
              beforeImage: current,
            },
          },
          ...syncResolvedStatusConsumers(state, current, resolved),
          error: undefined,
        };
      });
    } catch (error) {
      const uncertain = isUncertainMutationError(error);
      set((state) => {
        if (state.statusMutations[canonical]?.operationId !== operationId) {
          return { error: String(error) };
        }
        const entityPatch = uncertain
          ? {}
          : timelineEntityPatch(state, [
              { type: "replaceCanonical", target: current, status: current },
            ]);
        return {
          ...entityPatch,
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: uncertain ? "uncertain" : "failed",
              beforeImage: current,
              error: String(error),
            },
          },
          ...(!uncertain
            ? syncResolvedStatusConsumers(state, current, current)
            : {}),
          error: uncertain
            ? `${t("The status action result is uncertain")}: ${String(error)}`
            : String(error),
        };
      });
    }
  },
  votePoll: async (status, choices) => {
    if (!status.poll) return null;
    const current = resolvedEntityFor(get(), status) ?? status;
    let actingAccount: AccountSummary;
    try {
      actingAccount = requiredActingAccount(get());
      if (!actingAccount.capabilities.status.vote) {
        throw new Error(t("This action is not supported by the selected account"));
      }
    } catch (error) {
      set({ error: String(error) });
      return null;
    }
    const canonical = canonicalStatusKey(current);
    if (get().statusMutations[canonical]?.phase === "pending") return null;
    const operationId = crypto.randomUUID();
    set((state) => ({
      statusMutations: {
        ...state.statusMutations,
        [canonical]: {
          operationId,
          phase: "pending",
          beforeImage: current,
        },
      },
    }));
    try {
      const request: VotePollRequest = {
        identity: current.statusIdentity,
        actingAccountAcct: actingAccount.acct,
        pollId: current.poll?.id ?? status.poll.id,
        choices,
      };
      const poll = await invokeCommand<PollSummary>("vote_poll", { request });
      set((state) => {
        if (state.statusMutations[canonical]?.operationId !== operationId) {
          return {};
        }
        const entityPatch = timelineEntityPatch(state, [
          { type: "patchCanonical", target: current, patch: { poll } },
        ]);
        const resolved = resolvedEntityFor(entityPatch, current) ?? {
          ...current,
          poll,
        };
        return {
          ...entityPatch,
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: "confirmed",
              beforeImage: current,
            },
          },
          ...syncResolvedStatusConsumers(state, current, resolved),
          error: undefined,
        };
      });
      return poll;
    } catch (error) {
      set((state) => ({
        statusMutations: {
          ...state.statusMutations,
          [canonical]: {
            operationId,
            phase: isUncertainMutationError(error) ? "uncertain" : "failed",
            beforeImage: current,
            error: String(error),
          },
        },
        error: String(error),
      }));
      return null;
    }
  },
  editStatus: async (status, content, options = {}) => {
    const current = resolvedEntityFor(get(), status) ?? status;
    let actingAccount: AccountSummary;
    try {
      actingAccount = requiredActingAccount(get());
      if (!actingAccount.capabilities.status.edit) {
        throw new Error(t("This action is not supported by the selected account"));
      }
    } catch (error) {
      set({ error: String(error) });
      return null;
    }
    const canonical = canonicalStatusKey(current);
    if (get().statusMutations[canonical]?.phase === "pending") return null;
    const operationId = crypto.randomUUID();
    set((state) => ({
      statusMutations: {
        ...state.statusMutations,
        [canonical]: {
          operationId,
          phase: "pending",
          beforeImage: current,
        },
      },
    }));
    try {
      const request: EditStatusRequest = {
        identity: current.statusIdentity,
        actingAccountAcct: actingAccount.acct,
        accountId: current.accountId,
        status: content,
        visibility: options.visibility ?? current.visibility,
        spoilerText: options.spoilerText ?? (current.spoilerText || null),
        sensitive: options.sensitive ?? current.sensitive,
      };
      const updated = await invokeCommand<TimelineStatus>("edit_own_status", {
        request,
      });
      set((state) => {
        if (state.statusMutations[canonical]?.operationId !== operationId) {
          return {};
        }
        const entityPatch = timelineEntityPatch(state, [
          { type: "replaceCanonical", target: current, status: updated },
        ]);
        const resolved = resolvedEntityFor(entityPatch, current) ?? updated;
        return {
          ...entityPatch,
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: "confirmed",
              beforeImage: current,
            },
          },
          ...syncResolvedStatusConsumers(state, current, resolved),
          error: undefined,
        };
      });
      return updated;
    } catch (error) {
      set((state) => ({
        statusMutations: {
          ...state.statusMutations,
          [canonical]: {
            operationId,
            phase: isUncertainMutationError(error) ? "uncertain" : "failed",
            beforeImage: current,
            error: String(error),
          },
        },
        error: String(error),
      }));
      return null;
    }
  },
  deleteStatus: async (status) => {
    const current = resolvedEntityFor(get(), status) ?? status;
    let actingAccount: AccountSummary;
    try {
      actingAccount = requiredActingAccount(get());
      if (!actingAccount.capabilities.status.delete) {
        throw new Error(t("This action is not supported by the selected account"));
      }
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
    const canonical = canonicalStatusKey(current);
    if (get().statusMutations[canonical]?.phase === "pending") return false;
    const operationId = crypto.randomUUID();
    set((state) => ({
      statusMutations: {
        ...state.statusMutations,
        [canonical]: {
          operationId,
          phase: "pending",
          beforeImage: current,
        },
      },
    }));
    try {
      const request: DeleteStatusRequest = {
        identity: current.statusIdentity,
        actingAccountAcct: actingAccount.acct,
        accountId: current.accountId,
      };
      await invokeCommand("delete_own_status", { request });
      set((state) => ({
        ...timelineEntityPatch(state, [
          { type: "removeCanonical", target: current },
        ]),
        statusMutations: {
          ...state.statusMutations,
          [canonical]: {
            operationId,
            phase: "confirmed",
            beforeImage: current,
          },
        },
        mediaPreview:
          state.mediaPreview && sameCanonical(state.mediaPreview.status, current)
            ? null
            : state.mediaPreview,
        composeTarget:
          state.composeTarget && sameCanonical(state.composeTarget.status, current)
            ? null
            : state.composeTarget,
        error: undefined,
      }));
      return true;
    } catch (error) {
      set((state) => ({
        statusMutations: {
          ...state.statusMutations,
          [canonical]: {
            operationId,
            phase: isUncertainMutationError(error) ? "uncertain" : "failed",
            beforeImage: current,
            error: String(error),
          },
        },
        error: String(error),
      }));
      return false;
    }
  },
  saveSetting: async (key, value) => {
    set((state) =>
      state.snapshot
        ? {
            snapshot: {
              ...state.snapshot,
              settings: reduceSettingDraft(state.snapshot.settings, key, value),
            },
          }
        : {},
    );
    await settingsCoordinator.enqueue(key, value);
  },
  flushSettingSaves: () => settingsCoordinator.flush(),
  saveColumns: async (columns) => {
    try {
      const snapshot = await invokeCommand<AppSnapshot>("save_columns", {
        request: { columns: normalizeColumns(columns) },
      });
      set((state) => {
        const retained = new Set(
          [...snapshot.columns, ...state.dynamicColumns].map(
            (column) => column.id,
          ),
        );
        const removed = Object.keys(state.timelineKeys).filter(
          (columnId) => !retained.has(columnId),
        );
        return {
          ...timelineEntityPatch(
            state,
            removed.map((columnId) => ({
              type: "removeColumn" as const,
              columnId,
            })),
          ),
          snapshot,
          activeTabs: reconcileActiveTabs(snapshot.columns, state.activeTabs),
          error: undefined,
        };
      });
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  applyStreamEvent: (event) => {
    queueTimelineStreamEvent(event);
  },
  requestConfirmation: (request) => confirmationQueue.request(request),
  resolveConfirmation: (id, confirmed) => {
    confirmationQueue.resolve(id, confirmed);
  },
  cancelConfirmation: (id) => {
    confirmationQueue.cancel(id);
  },
  runMutation: (key, options) => mutationLifecycle.run(key, options),
  };
});

const pendingTimelineStreamEvents = new Map<string, TimelineStreamEvent>();
const timelineStreamPositions = new Map<
  string,
  { generation: number; sequence: number }
>();
const timelineResyncs = new Set<string>();
const streamBatchDurations: number[] = [];
let timelineStreamFrame: number | undefined;
let timelineStreamTimer: number | undefined;

function queueTimelineStreamEvent(event: TimelineStreamEvent) {
  const continuity = recordTimelineStreamPosition(event);
  if (continuity === "gap") recordFrontendStreamGap();
  if (event.kind === "resync") recordFrontendStreamResync();
  if (event.kind === "resync" || continuity === "gap") {
    scheduleTimelineResync(event);
    if (event.kind === "resync") return;
  }
  const normalized = event.status
    ? {
        ...event,
        status: {
          ...event.status,
          sourceAcct: event.status.sourceAcct ?? event.sourceAcct,
        },
      }
    : event;
  const key = timelineStreamEventKey(normalized);
  const existing = pendingTimelineStreamEvents.get(key);
  pendingTimelineStreamEvents.set(
    key,
    existing ? coalesceTimelineStreamEvent(existing, normalized) : normalized,
  );
  setPendingStreamEvents(pendingTimelineStreamEvents.size);
  if (timelineStreamFrame !== undefined || timelineStreamTimer !== undefined) {
    return;
  }
  timelineStreamFrame = window.requestAnimationFrame(() => {
    flushTimelineStreamEvents();
  });
  timelineStreamTimer = window.setTimeout(() => {
    flushTimelineStreamEvents();
  }, 40);
}

function recordTimelineStreamPosition(event: TimelineStreamEvent) {
  if (event.generation === undefined || event.sequence === undefined) {
    return "untracked" as const;
  }
  const key = `${event.serverDomain.toLowerCase()}:${accountIdentityKey(event.sourceAcct)}`;
  const previous = timelineStreamPositions.get(key);
  timelineStreamPositions.set(key, {
    generation: event.generation,
    sequence: event.sequence,
  });
  if (!previous || event.kind === "resync") return "continuous" as const;
  return previous.generation === event.generation &&
    previous.sequence + 1 === event.sequence
    ? ("continuous" as const)
    : ("gap" as const);
}

function scheduleTimelineResync(event: TimelineStreamEvent) {
  const key = `${event.serverDomain.toLowerCase()}:${accountIdentityKey(event.sourceAcct)}`;
  if (timelineResyncs.has(key)) return;
  const { snapshot, loadTimeline } = useAppStore.getState();
  if (!snapshot) return;
  const columns = snapshot.columns.filter(
    (column) => columnMatchesEventAccount(column, event.sourceAcct),
  );
  timelineResyncs.add(key);
  void Promise.all(columns.map((column) => loadTimeline(column, true))).finally(
    () => {
      timelineResyncs.delete(key);
    },
  );
}

export function flushTimelineStreamEventsForTest() {
  flushTimelineStreamEvents();
}

function flushTimelineStreamEvents() {
  if (timelineStreamFrame !== undefined) {
    window.cancelAnimationFrame(timelineStreamFrame);
    timelineStreamFrame = undefined;
  }
  if (timelineStreamTimer !== undefined) {
    window.clearTimeout(timelineStreamTimer);
    timelineStreamTimer = undefined;
  }
  const events = [...pendingTimelineStreamEvents.values()];
  pendingTimelineStreamEvents.clear();
  setPendingStreamEvents(0);
  if (events.length === 0) return;

  const startedAt = performance.now();
  const refreshColumns = new Map<string, ColumnSummary>();
  useAppStore.setState((state) => {
    const columns = allStoreColumns(state);
    if (columns.length === 0) return {};
    const membership = buildColumnCanonicalMembership(state);
    const operations: TimelineEntityOperation[] = [];
    const preserveAnchorColumns = new Set<string>();
    const unreadColumns = new Map<string, number>();

    for (const event of events) {
      collectSqlInvalidations(
        state,
        columns,
        membership,
        event,
        refreshColumns,
        unreadColumns,
      );
      if (event.kind === "deleteStatus" && event.statusId) {
        operations.push({
          type: "removeCanonicalId",
          serverDomain: event.serverDomain,
          statusId: event.statusId,
        });
        continue;
      }

      const eventStatus = event.status;
      if (!eventStatus) continue;
      const canonical = canonicalStatusKey(eventStatus);
      const matchingColumns: ColumnSummary[] = [];
      const removeFromColumns: string[] = [];

      for (const column of columns) {
        if (!columnMatchesEventAccount(column, event.sourceAcct)) continue;
        const contains = membership.get(column.id)?.has(canonical) ?? false;
        if (column.columnType === "search") {
          const matches =
            event.kind !== "newNotification" &&
            timelineStatusMatchesSearchQuery(
              eventStatus,
              column.columnParam ?? "",
            ) &&
            statusMatchesDisplayFilter(eventStatus, column);
          if (!matches) {
            if (contains) removeFromColumns.push(column.id);
            continue;
          }
          matchingColumns.push(column);
          if (
            !contains &&
            event.kind !== "statusUpdate" &&
            !(state.timelineNearTop[column.id] ?? true)
          ) {
            preserveAnchorColumns.add(column.id);
            unreadColumns.set(
              column.id,
              (unreadColumns.get(column.id) ?? 0) + 1,
            );
          }
          continue;
        }
        const receives =
          event.kind === "newNotification"
            ? timelineDescriptor(column.columnType)?.streamPolicy ===
              "notification"
            : columnReceivesStreamStatus(column, event.streamType) ||
              contains;
        if (!receives) continue;
        if (!statusMatchesDisplayFilter(eventStatus, column)) {
          if (contains) removeFromColumns.push(column.id);
          continue;
        }
        matchingColumns.push(column);
        if (
          !contains &&
          event.kind !== "statusUpdate" &&
          !(state.timelineNearTop[column.id] ?? true)
        ) {
          preserveAnchorColumns.add(column.id);
          unreadColumns.set(column.id, (unreadColumns.get(column.id) ?? 0) + 1);
        }
      }

      if (removeFromColumns.length > 0) {
        operations.push({
          type: "removeFromColumns",
          target: eventStatus,
          columnIds: removeFromColumns,
        });
      }
      operations.push({
        type: "upsertInColumns",
        columnIds: matchingColumns.map((column) => column.id),
        status: eventStatus,
        limits: Object.fromEntries(
          matchingColumns.map((column) => [
            column.id,
            timelineDisplayLimit(column),
          ]),
        ),
        updateOnly: event.kind === "statusUpdate",
        preserveAnchorColumns,
      });
    }

    const entityPatch = timelineEntityPatch(state, operations);
    const duration = performance.now() - startedAt;
    const performanceSnapshot = recordStreamBatchPerformance(
      state.streamPerformance,
      events.length,
      duration,
    );
    const consumers = syncAllStatusConsumers(state, entityPatch);
    const mediaDeleted = Boolean(
      state.mediaPreview &&
        events.some((event) =>
          streamEventDeletesStatus(event, state.mediaPreview!.status),
        ),
    );
    const composeDeleted = Boolean(
      state.composeTarget &&
        events.some((event) =>
          streamEventDeletesStatus(event, state.composeTarget!.status),
        ),
    );
    return {
      ...entityPatch,
      timelineUnread: incrementUnreadResources(
        state.timelineUnread,
        unreadColumns,
      ),
      ...consumers,
      ...(mediaDeleted ? { mediaPreview: null } : {}),
      ...(composeDeleted ? { composeTarget: null } : {}),
      streamPerformance: performanceSnapshot,
    };
  });

  for (const column of refreshColumns.values()) {
    void useAppStore.getState().loadTimeline(column, true, {
      delta: column.columnType === "yq",
    });
  }
}

function timelineStreamEventKey(event: TimelineStreamEvent) {
  if (event.kind === "newNotification" && event.status?.notificationId) {
    return `notification:${event.serverDomain.toLowerCase()}:${event.status.notificationId}:${accountIdentityKey(event.sourceAcct)}`;
  }
  const statusId =
    event.statusId ?? event.status?.originalStatusId ?? event.status?.id;
  return statusId
    ? `status:${event.serverDomain.toLowerCase()}:${statusId}:${event.streamType}:${accountIdentityKey(event.sourceAcct)}`
    : `${event.kind}:${event.streamType}:${event.sourceAcct}`;
}

function streamEventDeletesStatus(
  event: TimelineStreamEvent,
  status: TimelineStatus,
) {
  return (
    event.kind === "deleteStatus" &&
    event.serverDomain.toLowerCase() === status.serverDomain.toLowerCase() &&
    Boolean(
      event.statusId &&
        (event.statusId === status.id ||
          event.statusId === status.originalStatusId),
    )
  );
}

function coalesceTimelineStreamEvent(
  current: TimelineStreamEvent,
  next: TimelineStreamEvent,
) {
  if (next.kind === "deleteStatus") return next;
  if (current.kind === "deleteStatus") return current;
  if (current.kind === "newStatus" && next.kind === "statusUpdate") {
    return { ...next, kind: "newStatus" as const };
  }
  return next;
}

function buildColumnCanonicalMembership(
  state: Pick<AppStore, "timelineKeys" | "entities">,
) {
  return new Map(
    Object.entries(state.timelineKeys).map(([columnId, keys]) => [
      columnId,
      new Set(
        keys.flatMap((key) => {
          const status = state.entities.get(key);
          return status ? [canonicalStatusKey(status)] : [];
        }),
      ),
    ]),
  );
}

function collectSqlInvalidations(
  state: Pick<AppStore, "timelines" | "timelineNearTop">,
  columns: ColumnSummary[],
  membership: Map<string, Set<StatusKey>>,
  event: TimelineStreamEvent,
  refreshColumns: Map<string, ColumnSummary>,
  unreadColumns: Map<string, number>,
) {
  const invalidate = (column: ColumnSummary) => {
    if (state.timelineNearTop[column.id] ?? true) {
      refreshColumns.set(column.id, column);
    } else {
      unreadColumns.set(column.id, (unreadColumns.get(column.id) ?? 0) + 1);
    }
  };
  for (const column of columns) {
    if (!columnMatchesEventAccount(column, event.sourceAcct)) continue;
    if (!["custom", "yq", "thread"].includes(column.columnType)) continue;
    if (event.kind === "newNotification") continue;
    if (event.kind === "newStatus") {
      if (column.columnType !== "thread") invalidate(column);
      continue;
    }
    const eventStatus = event.status;
    const contains = eventStatus
      ? membership.get(column.id)?.has(canonicalStatusKey(eventStatus)) ?? false
      : Boolean(
          event.statusId &&
            state.timelines[column.id]?.some(
              (status) =>
                status.serverDomain === event.serverDomain &&
                (status.id === event.statusId ||
                  status.originalStatusId === event.statusId),
            ),
        );
    if (contains) invalidate(column);
  }
}

function recordStreamBatchPerformance(
  current: StreamPerformanceSnapshot,
  batchSize: number,
  durationMs: number,
) {
  streamBatchDurations.push(durationMs);
  if (streamBatchDurations.length > 120) streamBatchDurations.shift();
  const sorted = [...streamBatchDurations].sort((left, right) => left - right);
  const p95Index = Math.max(0, Math.ceil(sorted.length * 0.95) - 1);
  return {
    batches: current.batches + 1,
    lastBatchSize: batchSize,
    lastDurationMs: durationMs,
    p95DurationMs: sorted[p95Index] ?? durationMs,
  };
}

function allStoreColumns(
  state: Pick<AppStore, "snapshot" | "dynamicColumns">,
) {
  return [...(state.snapshot?.columns ?? []), ...state.dynamicColumns];
}

function sameCanonical(left: TimelineStatus, right: TimelineStatus) {
  return canonicalStatusKey(left) === canonicalStatusKey(right);
}

function resolvedEntityFor(
  state: Pick<AppStore, "entities" | "canonicalIndex">,
  status: TimelineStatus,
) {
  return (
    state.entities.get(statusKey(status)) ?? statusForCanonical(state, status)
  );
}

function syncResolvedStatusConsumers(
  state: Pick<AppStore, "mediaPreview" | "composeTarget">,
  target: TimelineStatus,
  resolved: TimelineStatus,
) {
  return {
    mediaPreview:
      state.mediaPreview && sameCanonical(state.mediaPreview.status, target)
        ? { ...state.mediaPreview, status: resolved }
        : state.mediaPreview,
    composeTarget:
      state.composeTarget && sameCanonical(state.composeTarget.status, target)
        ? { ...state.composeTarget, status: resolved }
        : state.composeTarget,
  };
}

function syncAllStatusConsumers(
  state: Pick<AppStore, "mediaPreview" | "composeTarget">,
  entityState: Pick<
    AppStore,
    "entities" | "canonicalIndex"
  >,
) {
  const resolve = (status: TimelineStatus) =>
    resolvedEntityFor(entityState, status);
  const mediaStatus = state.mediaPreview
    ? resolve(state.mediaPreview.status)
    : undefined;
  const composeStatus = state.composeTarget
    ? resolve(state.composeTarget.status)
    : undefined;
  return {
    mediaPreview:
      state.mediaPreview && mediaStatus
        ? { ...state.mediaPreview, status: mediaStatus }
        : state.mediaPreview,
    composeTarget:
      state.composeTarget && composeStatus
        ? { ...state.composeTarget, status: composeStatus }
        : state.composeTarget,
  };
}

function optimisticStatusActionPatch(
  status: TimelineStatus,
  action: string,
): Partial<TimelineStatus> {
  switch (action) {
    case "favourite":
      return {
        favourited: true,
        favouritesCount: status.favourited
          ? status.favouritesCount
          : status.favouritesCount + 1,
      };
    case "unfavourite":
      return {
        favourited: false,
        favouritesCount: status.favourited
          ? Math.max(0, status.favouritesCount - 1)
          : status.favouritesCount,
      };
    case "reblog":
      return {
        reblogged: true,
        reblogsCount: status.reblogged
          ? status.reblogsCount
          : status.reblogsCount + 1,
      };
    case "unreblog":
      return {
        reblogged: false,
        reblogsCount: status.reblogged
          ? Math.max(0, status.reblogsCount - 1)
          : status.reblogsCount,
      };
    case "bookmark":
      return { bookmarked: true };
    case "unbookmark":
      return { bookmarked: false };
    default:
      return {};
  }
}

function isUncertainMutationError(error: unknown) {
  return isResponseLossError(error);
}

const EMPTY_TIMELINE_STATUSES: TimelineStatus[] = [];

function timelineDisplayLimit(column: ColumnSummary) {
  return clampTimelineLimit(column.maxStatuses);
}

function columnReceivesStreamStatus(column: ColumnSummary, streamType: string) {
  const policy = timelineDescriptor(column.columnType)?.streamPolicy;
  if (policy === "home") return streamType === "user";
  if (policy === "public") return streamType === "public";
  if (policy === "local") return streamType === "public:local";
  if (policy === "hashtag")
    return streamType === `hashtag:${column.columnParam}`;
  if (policy === "list") return streamType === `list:${column.columnParam}`;
  return false;
}

function columnMatchesEventAccount(column: ColumnSummary, sourceAcct: string) {
  if (
    isUnifiedTimelineColumn(column) ||
    isGlobalSQLiteTimelineColumn(column)
  ) {
    return true;
  }
  return (
    !column.accountAcct ||
    accountIdentityKey(column.accountAcct) === accountIdentityKey(sourceAcct)
  );
}

function timelineRequestAccountAcct(column: ColumnSummary) {
  return isUnifiedTimelineColumn(column) || isGlobalSQLiteTimelineColumn(column)
    ? undefined
    : (column.accountAcct ?? undefined);
}

function isUnifiedTimelineColumn(column: ColumnSummary) {
  return ["home", "public", "notification"].includes(column.columnType);
}

/** These timelines query the shared SQLite corpus rather than one session. */
function isGlobalSQLiteTimelineColumn(column: ColumnSummary) {
  return ["custom", "yq", "search", "thread"].includes(column.columnType);
}

function accountIdentityKey(acct: string) {
  return acct.trim().replace(/^@+/, "").toLocaleLowerCase("en-US");
}

function statusMatchesDisplayFilter(
  status: TimelineStatus,
  column: ColumnSummary,
) {
  if (!timelineDisplayFilterApplies(column)) return true;
  const filter = normalizeDisplayFilter(column.displayFilter);
  const isBoost =
    status.id !== status.originalStatusId ||
    status.notificationKind === "reblog";
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

function timelineStatusMatchesSearchQuery(
  status: TimelineStatus,
  query: string,
) {
  const terms = normalizeSearchTerms(query);
  if (terms.length === 0) return false;
  const haystack = [
    status.content,
    status.spoilerText,
    status.uri,
    status.url ?? "",
    status.acct,
    status.displayName,
  ]
    .join("\n")
    .toLowerCase();
  return terms.every((term) => haystack.includes(term));
}

function normalizeSearchTerms(query: string) {
  return query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
}

export function statusIdentity(status: TimelineStatus) {
  return statusKey(status);
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
  const loadStrategy = timelineDescriptor(column.columnType)?.loadStrategy;
  const maxLimit =
    loadStrategy === "thread"
      ? 300
      : loadStrategy === "airContext"
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
  return timelineDescriptor(column.columnType)?.pagination === "api";
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

function mergeTimelineLoadPage(
  column: ColumnSummary,
  loaded: TimelineStatus[],
  current: TimelineStatus[],
  limit: number,
) {
  const filteredCurrent = filterTimelineStatusesForColumn(current, column);
  if (!columnReceivesRealtimeStatuses(column) || current.length === 0) {
    return loaded.slice(0, limit);
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
    return loaded.slice(0, limit);
  }
  return mergeSortedStatuses(loaded, streamed, limit);
}

function columnReceivesRealtimeStatuses(column: ColumnSummary) {
  const policy = timelineDescriptor(column.columnType)?.streamPolicy;
  return Boolean(policy && policy !== "none" && policy !== "notification");
}

function mergeSortedStatuses(
  left: TimelineStatus[],
  right: TimelineStatus[],
  limit: number,
) {
  const result: TimelineStatus[] = [];
  const seen = new Set<StatusKey>();
  let leftIndex = 0;
  let rightIndex = 0;
  while (
    result.length < limit &&
    (leftIndex < left.length || rightIndex < right.length)
  ) {
    const leftStatus = left[leftIndex];
    const rightStatus = right[rightIndex];
    let next: TimelineStatus;
    if (!leftStatus) {
      next = rightStatus;
      rightIndex += 1;
    } else if (!rightStatus) {
      next = leftStatus;
      leftIndex += 1;
    } else if (Date.parse(leftStatus.createdAt) >= Date.parse(rightStatus.createdAt)) {
      next = leftStatus;
      leftIndex += 1;
    } else {
      next = rightStatus;
      rightIndex += 1;
    }
    const key = statusIdentity(next);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(next);
  }
  return result;
}
