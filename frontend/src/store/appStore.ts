import { create } from "zustand";
import type {
  AppStartupProgressEvent,
  AppSnapshot,
  ColumnSummary,
  ConfirmationDialogRequest,
  ConfirmationDialogState,
  MediaAttachment,
  MediaPreviewState,
  PollSummary,
  PostSubmitOptions,
  SettingsSnapshot,
  StatusBarSnapshot,
  StatusIdentity,
  StatusViewerStateSummary,
  TimelineStatus,
  TimelineStreamEvent,
  UserProfileTarget,
} from "../types/app";
import {
  invokeTypedCommand,
  invokeTypedReadCommand,
} from "../api/tauri";
import { isResponseLossError } from "../api/ipcErrors";
import {
  canonicalStatusKey,
  normalizeTimelineLimit,
  createTimelineEntityState,
  reduceTimelineEntities,
  statusForCanonical,
  statusKey,
  type CanonicalIndex,
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
import { clearBlurHashCache } from "../utils/blurhash";
import {
  normalizeDisplayFilter,
  timelineDisplayFilterApplies,
} from "../utils/columns";
import { isVideoMedia, previewMediaSources } from "../utils/media";
import { clearMediaRetryCache } from "../utils/mediaRetryCoordinator";
import { clearTranslationCache } from "../features/timeline/translation";
import {
  frontendRequestScheduler,
  type RequestLane,
  type RequestLaneMetrics,
} from "../utils/requestScheduler";
import { t } from "../i18n";
import {
  reduceOpenOrFocusDynamicPane,
  type DynamicPaneDescriptor,
} from "./slices/panes";
import type { ResourcePhase, ResourceState } from "./slices/resources";
import {
  applyPersistedSetting,
  settingKeys,
  settingValue,
} from "./slices/settingsDraft";
import {
  initialComposeSlice,
  type ComposeTarget,
  type ComposeVisibility,
} from "./slices/compose";
import { reduceOverlaySlice } from "./slices/overlays";
import { clearUnreadResource } from "./slices/notifications";
import {
  initialBootState,
  type BootState,
} from "./slices/session";
import { createSessionActions } from "./actions/sessionActions";
import { createPaneActions } from "./actions/paneActions";
import { createComposeTargetActions } from "./actions/composeTargetActions";
import { createSettingsActions } from "./actions/settingsActions";
import { createLifecycleActions } from "./actions/lifecycleActions";
import { createComposeSubmitActions } from "./actions/composeSubmitActions";
import { createStatusMutationActions } from "./actions/statusMutationActions";
import {
  activateAnalyticalTimelineRefresh,
  cancelAnalyticalTimelineRefresh,
  createTimelineStreamActions,
  resetAnalyticalTimelineRefreshes,
} from "./actions/timelineStreamActions";
import { notificationKindIsReblog } from "../domain/notification";
import {
  cancelQuoteConsumer,
  cancelTimelineQueryColumn,
  createTimelineQueryActions,
  resetTimelineQueryCoordinator,
} from "./actions/timelineQueryActions";

export {
  flushAnalyticalTimelineRefreshesForTest,
  flushTimelineStreamEventsForTest,
} from "./actions/timelineStreamActions";

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

export type AppStore = {
  boot: BootState;
  snapshot?: AppSnapshot;
  entities: Map<StatusKey, TimelineStatus>;
  timelineKeys: Record<string, StatusKey[]>;
  timelineDeferredKeys: Record<string, StatusKey[]>;
  canonicalIndex: CanonicalIndex;
  timelines: Record<string, TimelineStatus[]>;
  timelineUnread: Record<string, number>;
  statusMutations: Record<StatusKey, StatusMutationState>;
  resourceStates: Record<string, AsyncResourceState>;
  settingMutations: Record<string, SettingSaveState>;
  mutationStates: Record<string, MutationState>;
  requestMetrics: Record<RequestLane, RequestLaneMetrics>;
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
  loginWithInstanceDomain: (domain: string, operationId?: string) => Promise<boolean>;
  loginWithBluesky: (
    identifier: string,
    password: string,
    operationId?: string,
  ) => Promise<boolean>;
  loadStatusBar: () => Promise<void>;
  loadTimeline: (
    column: ColumnSummary,
    refresh?: boolean,
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
  applyTimelineCacheCommit: () => void;
  requestConfirmation: (
    request: ConfirmationDialogRequest,
  ) => Promise<boolean>;
  resolveConfirmation: (id: string, confirmed: boolean) => void;
  cancelConfirmation: (id: string) => void;
  cancelAllConfirmations: () => void;
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

function seedSettingsCoordinator(
  coordinator: SettingsMutationCoordinator<SettingsSnapshot>,
  settings: SettingsSnapshot,
) {
  for (const key of settingKeys) {
    coordinator.seed(key, settingValue(settings, key));
  }
}

function cancelBackendMutationOperation(targetOperationId: string) {
  void invokeTypedCommand("cancel_mutation_operation", {
    request: { targetOperationId },
  }).catch(() => undefined);
}

function cancelPendingStatusMutations(
  statusMutations: AppStore["statusMutations"],
) {
  for (const mutation of Object.values(statusMutations)) {
    if (mutation.phase === "pending") {
      cancelBackendMutationOperation(mutation.operationId);
    }
  }
}

function cancelAccountScopedFrontendWork(
  statusMutations: AppStore["statusMutations"],
) {
  frontendRequestScheduler.cancelAll();
  resetTimelineQueryCoordinator();
  resetAnalyticalTimelineRefreshes();
  cancelPendingStatusMutations(statusMutations);
}

function clearAccountScopedCaches() {
  clearBlurHashCache();
  clearMediaRetryCache();
  clearTranslationCache();
}

function requiredActingAccount(state: AppStore) {
  const acct = state.snapshot?.activeAcct?.trim();
  if (!acct) throw new Error(t("No active account is signed in"));
  const account = state.snapshot?.accounts.find((candidate) => candidate.acct === acct);
  if (!account) throw new Error(t("No active account is signed in"));
  return account;
}

function appStoreTimelineInitialState() {
  const state = createTimelineEntityState();
  return {
    entities: state.entities,
    timelineKeys: state.columnKeys,
    timelineDeferredKeys: state.deferredColumnKeys,
    canonicalIndex: state.canonicalIndex,
    timelines: state.timelines,
  };
}

function statusIdentityValueKey(identity: StatusIdentity) {
  return JSON.stringify([
    identity.protocol,
    identity.serverDomain.toLocaleLowerCase("en-US"),
    identity.canonicalUri,
    identity.remoteId,
  ]);
}

function statusIdentityKey(status: TimelineStatus) {
  return statusIdentityValueKey(status.statusIdentity);
}

function timelineEntityPatch(
  state: Pick<
    AppStore,
    | "entities"
    | "timelineKeys"
    | "timelineDeferredKeys"
    | "timelineUnread"
    | "canonicalIndex"
    | "timelines"
  >,
  operations: TimelineEntityOperation[],
) {
  const next = reduceTimelineEntities(
    {
      entities: state.entities,
      columnKeys: state.timelineKeys,
      deferredColumnKeys: state.timelineDeferredKeys,
      canonicalIndex: state.canonicalIndex,
      timelines: state.timelines,
    },
    operations,
  );
  const timelineUnread = { ...state.timelineUnread };
  const deferredColumns = new Set([
    ...Object.keys(state.timelineDeferredKeys),
    ...Object.keys(next.deferredColumnKeys),
  ]);
  for (const columnId of deferredColumns) {
    const before = state.timelineDeferredKeys[columnId]?.length ?? 0;
    const after = next.deferredColumnKeys[columnId]?.length ?? 0;
    if (before === 0 && after === 0) continue;
    const unread = deferredUnreadCount(next, columnId);
    if (unread > 0) {
      timelineUnread[columnId] = unread;
    } else {
      delete timelineUnread[columnId];
    }
  }
  return {
    entities: next.entities,
    timelineKeys: next.columnKeys,
    timelineDeferredKeys: next.deferredColumnKeys,
    timelineUnread,
    canonicalIndex: next.canonicalIndex,
    timelines: next.timelines,
  };
}

function deferredUnreadCount(
  state: ReturnType<typeof reduceTimelineEntities>,
  columnId: string,
) {
  const deferredKeys = state.deferredColumnKeys[columnId] ?? [];
  if (deferredKeys.length === 0) return 0;

  const visibleTopKey = state.columnKeys[columnId]?.[0];
  const visibleTop = visibleTopKey
    ? state.entities.get(visibleTopKey)
    : undefined;
  const visibleTopTimestamp = visibleTop
    ? Date.parse(visibleTop.createdAt)
    : Number.NaN;
  const unreadIdentities = new Set<StatusKey>();

  for (const key of deferredKeys) {
    const status = state.entities.get(key);
    if (!status) continue;
    const timestamp = Date.parse(status.createdAt);
    // Federated delivery can surface an old remote post after the currently
    // loaded head. Keep it deferred for chronological merging, but it is not
    // a new row above the user's current timeline head and must not inflate
    // the badge.
    if (
      Number.isFinite(visibleTopTimestamp) &&
      Number.isFinite(timestamp) &&
      timestamp <= visibleTopTimestamp
    ) {
      continue;
    }
    // Ordinary timeline rows are unique by their canonical URI. Notification
    // wrappers remain distinct because separate actors/events are real rows.
    unreadIdentities.add(status.notificationId ? key : canonicalStatusKey(status));
  }

  return unreadIdentities.size;
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
      invokeTypedCommand("save_settings", {
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
  const mutationLifecycle = new MutationLifecycle(
    (mutation) => {
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
    },
    cancelBackendMutationOperation,
  );
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
  const reconcileViewerStates = async (actingAccountAcct: string) => {
    const unique = new Map<string, TimelineStatus>();
    for (const status of get().entities.values()) {
      unique.set(statusIdentityKey(status), status);
    }
    if (unique.size === 0) return;

    let summaries: StatusViewerStateSummary[];
    let failure: unknown;
    try {
      summaries = await invokeTypedReadCommand(
        "status_viewer_states",
        {
          request: {
            actingAccountAcct,
            identities: [...unique.values()].map(
              (status) => status.statusIdentity,
            ),
          },
        },
      );
    } catch (error) {
      // Do not leave the previous actor's viewer flags visible after the
      // operation source changed. Missing state is conservatively false.
      summaries = [];
      failure = error;
    }
    const byIdentity = new Map(
      summaries.map((summary) => [
        statusIdentityValueKey(summary.identity),
        summary,
      ]),
    );
    set((state) => {
      const operations = [...unique.values()].map((status) => {
        const summary = byIdentity.get(statusIdentityKey(status));
        return {
          type: "patchCanonical" as const,
          target: status,
          patch: {
            favourited: summary?.favourited ?? false,
            reblogged: summary?.reblogged ?? false,
            bookmarked: summary?.bookmarked ?? false,
          },
        };
      });
      const entityPatch = timelineEntityPatch(state, operations);
      return {
        ...entityPatch,
        ...syncAllStatusConsumers(state, entityPatch),
      };
    });
    if (failure) throw failure;
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
    cancelAccountScopedFrontendWork: () =>
      cancelAccountScopedFrontendWork(get().statusMutations),
    cancelActingAccountMutations: () =>
      cancelPendingStatusMutations(get().statusMutations),
    clearAccountScopedCaches,
    appStoreTimelineInitialState,
    isUncertainMutationError,
    reconcileViewerStates,
  }),
  ...createPaneActions(openOrFocusDynamicPane),
  ...createComposeTargetActions({
    set,
    focusComposer: () => document.getElementById("compose-textarea")?.focus(),
  }),
  ...createSettingsActions({
    set,
    get,
    coordinator: settingsCoordinator,
    removeTimelineColumns: (state, columnIds) =>
      timelineEntityPatch(
        state,
        columnIds.map((columnId) => ({
          type: "removeColumn" as const,
          columnId,
        })),
      ),
  }),
  ...createLifecycleActions(confirmationQueue, mutationLifecycle),
  ...createComposeSubmitActions({
    set,
    get,
    mutations: mutationLifecycle,
    requiredActingAccount,
    allColumns: allStoreColumns,
    statusMatchesDisplayFilter,
    timelineDisplayLimit,
    entityPatch: timelineEntityPatch,
    isUncertain: isUncertainMutationError,
  }),
  ...createStatusMutationActions({
    set,
    get,
    requiredActingAccount,
    entityPatch: timelineEntityPatch,
    resolvedEntityFor,
    syncResolvedConsumers: syncResolvedStatusConsumers,
    sameCanonical,
    isUncertain: isUncertainMutationError,
  }),
  ...createTimelineStreamActions({
    set,
    get,
    allColumns: allStoreColumns,
    entityPatch: timelineEntityPatch,
    syncAllConsumers: syncAllStatusConsumers,
    columnMatchesEventAccount,
    statusMatchesDisplayFilter,
    timelineDisplayLimit,
    timelineStatusMatchesSearchQuery,
    accountIdentityKey,
  }),
  ...createTimelineQueryActions({
    set,
    get,
    entityPatch: timelineEntityPatch,
    statusMatchesDisplayFilter,
  }),
  setTimelineNearTop: (column, nearTop) => {
    const currentNearTop = get().timelineNearTop[column.id] ?? true;
    if (currentNearTop === nearTop) return;
    const returnedToTop = nearTop && !currentNearTop;
    const hasUnread = (get().timelineUnread[column.id] ?? 0) > 0;
    const hasDeferred =
      (get().timelineDeferredKeys[column.id]?.length ?? 0) > 0;
    set((state) => {
      const limit = timelineDisplayLimit(column);
      const current = state.timelines[column.id] ?? EMPTY_TIMELINE_STATUSES;
      const operations: TimelineEntityOperation[] = [];
      if (returnedToTop) {
        if ((state.timelineDeferredKeys[column.id]?.length ?? 0) > 0) {
          operations.push({
            type: "flushDeferredColumn",
            columnId: column.id,
            limit,
          });
        } else if (current.length > limit) {
          operations.push({
            type: "replaceColumn",
            columnId: column.id,
            statuses: current,
            limit,
          });
        }
      }
      return {
        ...timelineEntityPatch(state, operations),
        timelineNearTop: {
          ...state.timelineNearTop,
          [column.id]: nearTop,
        },
        ...(returnedToTop && hasDeferred
          ? {
              timelineUnread: clearUnreadResource(
                state.timelineUnread,
                column.id,
              ),
            }
          : {}),
      };
    });
    if (returnedToTop && ["custom", "yq"].includes(column.columnType)) {
      // The post-commit signal can mark a query dirty without a matching live
      // event (startup sync, resync, or a notification side effect). activate
      // is a no-op when clean, so unread is not the authority for SQL refresh.
      activateAnalyticalTimelineRefresh(column);
    } else if (returnedToTop && hasUnread && !hasDeferred) {
      void get().loadTimeline(column, true);
    }
  },
  trimTimelineToMaxStatuses: (column) => {
    const hadUnread = (get().timelineUnread[column.id] ?? 0) > 0;
    const hadDeferred =
      (get().timelineDeferredKeys[column.id]?.length ?? 0) > 0;
    set((state) => {
      const current = state.timelines[column.id] ?? EMPTY_TIMELINE_STATUSES;
      const limit = timelineDisplayLimit(column);
      const operations: TimelineEntityOperation[] = [];
      if (current.length > limit) {
        operations.push({
          type: "replaceColumn",
          columnId: column.id,
          statuses: current,
          limit,
        });
      }
      if (hadDeferred) {
        operations.push({
          type: "flushDeferredColumn",
          columnId: column.id,
          limit,
        });
      }
      return {
        ...timelineEntityPatch(state, operations),
        timelineNearTop: {
          ...state.timelineNearTop,
          [column.id]: true,
        },
        // Deferred rows are already held locally, so smooth scroll completion
        // can reveal them without racing network or SQLite cache persistence.
        timelineUnread: hadUnread && !hadDeferred
          ? state.timelineUnread
          : clearUnreadResource(state.timelineUnread, column.id),
      };
    });
    if (["custom", "yq"].includes(column.columnType)) {
      // Smooth scroll-to-top completes through this path instead of
      // setTimelineNearTop. A cache-only dirty version must not be stranded.
      activateAnalyticalTimelineRefresh(column);
    } else if (hadUnread && !hadDeferred) {
      // Non-row invalidations (for example a thread mutation) still need a
      // reload because there is no concrete deferred entity to reveal.
      void get().loadTimeline(column, true);
    }
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
    if (!get().timelines[column.id]) {
      void get().loadTimeline(column);
    } else if (
      ["custom", "yq"].includes(column.columnType) &&
      (get().timelineNearTop[column.id] ?? true)
    ) {
      activateAnalyticalTimelineRefresh(column);
    }
  },
  clearPendingPaneScroll: (paneIndex) => {
    set((state) =>
      state.pendingScrollPaneIndex === paneIndex
        ? { pendingScrollPaneIndex: undefined }
        : {},
    );
  },
  openMediaPreview: (status, media) => {
    const isVideo = isVideoMedia(media);
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
      cancelTimelineQueryColumn(columnId);
      cancelAnalyticalTimelineRefresh(columnId);
      void cancelQuoteConsumer(columnId).catch(() => undefined);
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
  };
});

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

function isUncertainMutationError(error: unknown) {
  return isResponseLossError(error);
}

const EMPTY_TIMELINE_STATUSES: TimelineStatus[] = [];

function timelineDisplayLimit(column: ColumnSummary) {
  return normalizeTimelineLimit(column.maxStatuses);
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
    notificationKindIsReblog(status.notificationKind);
  const hasMedia = status.media.length > 0;
  if (filter.excludeBoosts && isBoost) return false;
  if (filter.excludeMedia && hasMedia) return false;
  if (filter.includeMedia && !hasMedia) return false;
  return true;
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
