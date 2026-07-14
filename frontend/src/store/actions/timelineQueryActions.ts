import type { StoreApi } from "zustand";

import {
  invokeTypedCommand,
  invokeTypedReadCommand,
  invokeTypedReadCommandWithOperationId,
} from "../../api/tauri";
import { timelineDescriptor } from "../../domain/timelineDescriptors";
import {
  clampTimelineLimit,
  statusKey,
  type StatusKey,
  type TimelineEntityOperation,
} from "../../domain/timelineEntities";
import { t } from "../../i18n";
import type {
  ColumnSummary,
  TimelinePageResponse,
  TimelineRequest,
  TimelineStatus,
} from "../../types/app";
import {
  normalizeDisplayFilter,
  timelineDisplayFilterApplies,
} from "../../utils/columns";
import {
  frontendRequestScheduler,
  RequestCancelledError,
} from "../../utils/requestScheduler";
import { hasTopLevelSqlLimit } from "../../utils/sql";
import { clearUnreadResource } from "../slices/notifications";
import { reduceResourceStates } from "../slices/resources";
import type { AppStore } from "../appStore";

type PendingTimelineRefresh = {
  column: ColumnSummary;
};
type TimelineEntityPatch = Pick<
  AppStore,
  "entities" | "timelineKeys" | "canonicalIndex" | "timelines"
>;
type TimelineQueryContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  entityPatch: (
    state: AppStore,
    operations: TimelineEntityOperation[],
  ) => TimelineEntityPatch;
  statusMatchesDisplayFilter: (
    status: TimelineStatus,
    column: ColumnSummary,
  ) => boolean;
};

let resetActiveCoordinator: (() => void) | undefined;
let cancelActiveColumn: ((columnId: string) => void) | undefined;

export function resetTimelineQueryCoordinator() {
  resetActiveCoordinator?.();
}

export function cancelTimelineQueryColumn(columnId: string) {
  cancelActiveColumn?.(columnId);
}

export function cancelQuoteConsumer(quoteConsumerId: string) {
  return invokeTypedCommand("cancel_quote_consumer", {
    request: { quoteConsumerId },
  });
}

function isUnifiedTimelineColumn(column: ColumnSummary) {
  return ["home", "public", "notification"].includes(column.columnType);
}

function isGlobalSQLiteTimelineColumn(column: ColumnSummary) {
  return ["custom", "yq", "search", "thread"].includes(column.columnType);
}

function requestLane(column: ColumnSummary) {
  return ["custom", "yq"].includes(column.columnType)
    ? ("analytics" as const)
    : ("timeline" as const);
}

function requestAccountAcct(column: ColumnSummary) {
  return isUnifiedTimelineColumn(column) || isGlobalSQLiteTimelineColumn(column)
    ? undefined
    : (column.accountAcct ?? undefined);
}

function timelineDisplayLimit(column: ColumnSummary) {
  return clampTimelineLimit(column.maxStatuses);
}

function timelinePageLimit(column: ColumnSummary) {
  const strategy = timelineDescriptor(column.columnType)?.loadStrategy;
  const maxLimit = strategy === "thread" ? 300 : strategy === "airContext" ? 2 : 120;
  return Math.min(maxLimit, timelineDisplayLimit(column));
}

function logContext(column: ColumnSummary) {
  const accountScope = isUnifiedTimelineColumn(column)
    ? "unified"
    : isGlobalSQLiteTimelineColumn(column)
      ? "sqlite"
      : (column.accountAcct ?? "all");
  return `column=${column.id} type=${column.columnType} account=${accountScope} dynamic=${Boolean(column.dynamic)}`;
}

function columnSignature(column: ColumnSummary) {
  return JSON.stringify([
    column.columnType,
    column.columnParam ?? null,
    requestAccountAcct(column) ?? null,
    column.displayFilter ?? null,
    column.maxStatuses,
  ]);
}

function isVisible(state: Pick<AppStore, "activeTabs">, column: ColumnSummary) {
  return (state.activeTabs[column.paneIndex] ?? column.id) === column.id;
}

function elapsed(startedAt: number) {
  return (performance.now() - startedAt).toFixed(1);
}

async function cancellableRead(
  command: "load_timeline",
  request: TimelineRequest,
  signal: AbortSignal,
): Promise<TimelineStatus[]>;
async function cancellableRead(
  command: "refresh_timeline",
  request: TimelineRequest,
  signal: AbortSignal,
): Promise<TimelineStatus[]>;
async function cancellableRead(
  command: "load_more_timeline",
  request: TimelineRequest,
  signal: AbortSignal,
): Promise<TimelinePageResponse>;
async function cancellableRead(
  command: "load_timeline" | "refresh_timeline",
  request: TimelineRequest,
  signal: AbortSignal,
): Promise<TimelineStatus[]>;
async function cancellableRead(
  command: "load_timeline" | "refresh_timeline" | "load_more_timeline",
  request: TimelineRequest,
  signal: AbortSignal,
): Promise<TimelineStatus[] | TimelinePageResponse> {
  const operationId = crypto.randomUUID();
  const cancel = () => {
    void invokeTypedCommand("cancel_timeline_query", {
      request: { targetOperationId: operationId },
    }).catch(() => undefined);
  };
  signal.addEventListener("abort", cancel, { once: true });
  try {
    return command === "load_more_timeline"
      ? await invokeTypedReadCommandWithOperationId(
          "load_more_timeline",
          { request },
          operationId,
        )
      : command === "refresh_timeline"
        ? await invokeTypedReadCommandWithOperationId(
            "refresh_timeline",
            { request },
            operationId,
          )
        : await invokeTypedReadCommandWithOperationId(
            "load_timeline",
            { request },
            operationId,
          );
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

function parseThreadParam(columnParam?: string | null) {
  if (!columnParam) throw new Error(t("Thread target is missing"));
  const parsed = JSON.parse(columnParam) as {
    statusId?: unknown;
    serverDomain?: unknown;
    sourceAcct?: unknown;
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
    sourceAcct: typeof parsed.sourceAcct === "string" ? parsed.sourceAcct : undefined,
  };
}

function parseAirContextParam(columnParam?: string | null) {
  if (!columnParam) throw new Error(t("AIR context target is missing"));
  const parsed = JSON.parse(columnParam) as {
    statusId?: unknown;
    serverDomain?: unknown;
    accountId?: unknown;
    accountAcct?: unknown;
    sourceAcct?: unknown;
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
    accountAcct: typeof parsed.accountAcct === "string" ? parsed.accountAcct : undefined,
    sourceAcct: typeof parsed.sourceAcct === "string" ? parsed.sourceAcct : undefined,
  };
}

function oldest(statuses: TimelineStatus[]) {
  return statuses.length === 0
    ? undefined
    : statuses.reduce((value, status) =>
        Date.parse(status.createdAt) < Date.parse(value.createdAt) ? status : value,
      );
}

function columnHasSqlLimit(column: ColumnSummary) {
  return column.columnType === "custom" && hasTopLevelSqlLimit(column.columnParam ?? "");
}

function canLoadMoreFromApi(column: ColumnSummary) {
  return timelineDescriptor(column.columnType)?.pagination === "api";
}

function refreshMayWriteStatusCache(column: ColumnSummary, refresh: boolean) {
  return (
    refresh &&
    ["home", "public", "notification", "local", "list", "hashtag"].includes(
      column.columnType,
    )
  );
}

function pageHasMore(length: number, limit: number, refresh = false) {
  if (length === 0) return false;
  return length >= (refresh ? Math.min(limit, 80) : limit);
}

function receivesRealtime(column: ColumnSummary) {
  const policy = timelineDescriptor(column.columnType)?.streamPolicy;
  return Boolean(policy && policy !== "none" && policy !== "notification");
}

function mergeSorted(
  left: TimelineStatus[],
  right: TimelineStatus[],
  limit: number,
) {
  const result: TimelineStatus[] = [];
  const seen = new Set<StatusKey>();
  let leftIndex = 0;
  let rightIndex = 0;
  while (result.length < limit && (leftIndex < left.length || rightIndex < right.length)) {
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
    const key = statusKey(next);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(next);
  }
  return result;
}

export function createTimelineQueryActions({
  set,
  get,
  entityPatch,
  statusMatchesDisplayFilter,
}: TimelineQueryContext): Pick<AppStore, "loadTimeline" | "loadMoreTimeline"> {
  const inFlight = new Map<string, Promise<void>>();
  const pendingRefreshes = new Map<string, PendingTimelineRefresh>();
  const signatures = new Map<string, string>();
  resetActiveCoordinator = () => {
    pendingRefreshes.clear();
    signatures.clear();
  };
  cancelActiveColumn = (columnId) => {
    pendingRefreshes.delete(columnId);
    signatures.delete(columnId);
  };

  const filterStatuses = (statuses: TimelineStatus[], column: ColumnSummary) =>
    timelineDisplayFilterApplies(column)
      ? statuses.filter((status) => statusMatchesDisplayFilter(status, column))
      : statuses;

  const mergeLoadPage = (
    column: ColumnSummary,
    loaded: TimelineStatus[],
    current: TimelineStatus[],
    limit: number,
  ) => {
    const filteredCurrent = filterStatuses(current, column);
    if (!receivesRealtime(column) || current.length === 0) return loaded.slice(0, limit);
    const seen = new Set(loaded.map(statusKey));
    const oldestLoadedTime =
      loaded.length > 0
        ? Math.min(...loaded.map((status) => Date.parse(status.createdAt)))
        : Number.NEGATIVE_INFINITY;
    const streamed = filteredCurrent.filter(
      (status) =>
        !seen.has(statusKey(status)) && Date.parse(status.createdAt) >= oldestLoadedTime,
    );
    return streamed.length === 0
      ? loaded.slice(0, limit)
      : mergeSorted(loaded, streamed, limit);
  };

  const queueRefresh = (column: ColumnSummary) => {
    pendingRefreshes.set(column.id, { column });
  };

  const loadTimeline: AppStore["loadTimeline"] = async (
    column,
    refresh = false,
  ) => {
    const descriptor = timelineDescriptor(column.columnType);
    if (!descriptor) {
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
    // Profile panes own their account/profile requests in UserProfilePane.
    // Sending them through the generic timeline loader reaches the backend as
    // refresh_timeline(profile), which is intentionally unsupported.
    if (descriptor.loadStrategy === "profile") return;
    const signature = columnSignature(column);
    const running = inFlight.get(column.id);
    if (running) {
      const resourceChanged = signatures.get(column.id) !== signature;
      if (refresh || resourceChanged) {
        queueRefresh(column);
        if (resourceChanged) frontendRequestScheduler.cancel(`timeline:${column.id}`);
        console.debug(
          `[awayuki][ui-timeline] queued ${logContext(column)} refresh=${refresh} reason=${resourceChanged ? "resource_changed" : "in_flight"}`,
        );
      } else {
        console.info(
          `[awayuki][ui-timeline] coalesced ${logContext(column)} refresh=${refresh} reason=in_flight`,
        );
      }
      await running;
      return;
    }

    const run = async () => {
      const startedAt = performance.now();
      const limit = timelinePageLimit(column);
      const accountAcct = requestAccountAcct(column);
      const request: TimelineRequest = {
        columnType: column.columnType,
        columnParam: column.columnParam,
        limit,
        quoteConsumerId: column.id,
        ...(accountAcct ? { accountAcct } : {}),
        displayFilter: timelineDisplayFilterApplies(column)
          ? normalizeDisplayFilter(column.displayFilter)
          : undefined,
      };
      console.info(
        `[awayuki][ui-timeline] start ${logContext(column)} refresh=${refresh} delta=false limit=${limit}`,
      );
      const resourceKey = `timeline:${column.id}`;
      let generation = 0;
      set((state) => ({ loading: { ...state.loading, [column.id]: true } }));
      try {
        const statuses = await frontendRequestScheduler.schedule(
          {
            key: resourceKey,
            lane: requestLane(column),
            priority: isVisible(get(), column) ? 100 : 0,
            replace: false,
          },
          async (context) => {
            generation = context.generation;
            set((state) => ({
              resourceStates: reduceResourceStates(state.resourceStates, {
                type: "begin",
                key: resourceKey,
                generation,
                refreshing: refresh,
              }),
            }));
            const strategy = descriptor.loadStrategy;
            const result =
              strategy === "thread"
                ? await invokeTypedReadCommand("status_thread", {
                    request: {
                      ...parseThreadParam(column.columnParam),
                      limit,
                      quoteConsumerId: column.id,
                    },
                  })
                : strategy === "airContext"
                  ? await invokeTypedReadCommand("air_context", {
                      request: {
                        ...parseAirContextParam(column.columnParam),
                        limit,
                        quoteConsumerId: column.id,
                      },
                    })
                  : await cancellableRead(
                      refresh ? "refresh_timeline" : "load_timeline",
                      request,
                      context.signal,
                    );
            if (!context.isCurrent()) throw new RequestCancelledError(resourceKey);
            return result;
          },
        );
        const displayed = filterStatuses(statuses, column);
        console.info(
          `[awayuki][ui-timeline] success ${logContext(column)} refresh=${refresh} delta=false count=${statuses.length} display_count=${displayed.length} duration_ms=${elapsed(startedAt)}`,
        );
        set((state) => {
          if (state.resourceStates[resourceKey]?.generation !== generation) return {};
          const displayLimit = timelineDisplayLimit(column);
          // created_at is display order, not a durable change sequence. A
          // coalesced analytical refresh must replace the result so old rows
          // that enter or leave a YQ predicate are reconciled correctly.
          const operation: TimelineEntityOperation = {
            type: "replaceColumn",
            columnId: column.id,
            statuses: mergeLoadPage(
              column,
              displayed,
              state.timelines[column.id] ?? [],
              displayLimit,
            ),
            limit: displayLimit,
          };
          return {
            ...entityPatch(state, [operation]),
            loading: { ...state.loading, [column.id]: false },
            resourceStates: reduceResourceStates(state.resourceStates, {
              type: "succeed",
              key: resourceKey,
              generation: state.resourceStates[resourceKey]?.generation ?? generation,
            }),
            timelineUnread: clearUnreadResource(state.timelineUnread, column.id),
            timelineHasMore: {
              ...state.timelineHasMore,
              [column.id]: timelineDescriptor(column.columnType)?.pagination === "none"
                  ? false
                  : columnHasSqlLimit(column)
                    ? false
                    : canLoadMoreFromApi(column)
                      ? statuses.length > 0
                      : pageHasMore(statuses.length, limit, refresh),
            },
          };
        });
        if (refreshMayWriteStatusCache(column, refresh)) {
          get().applyTimelineCacheCommit();
        }
      } catch (error) {
        const cancelled = error instanceof RequestCancelledError;
        console.error(
          `[awayuki][ui-timeline] error ${logContext(column)} refresh=${refresh} delta=false duration_ms=${elapsed(startedAt)} error=${String(error)}`,
        );
        set((state) =>
          state.resourceStates[resourceKey]?.generation === generation
            ? {
                loading: { ...state.loading, [column.id]: false },
                resourceStates: reduceResourceStates(
                  state.resourceStates,
                  cancelled
                    ? { type: "cancel", key: resourceKey, generation }
                    : {
                        type: "fail",
                        key: resourceKey,
                        generation,
                        error: String(error),
                      },
                ),
              }
            : {},
        );
      } finally {
        inFlight.delete(column.id);
        if (signatures.get(column.id) === signature) signatures.delete(column.id);
        set({ requestMetrics: frontendRequestScheduler.metrics() });
        const pending = pendingRefreshes.get(column.id);
        if (pending) {
          pendingRefreshes.delete(column.id);
          await get().loadTimeline(pending.column, true);
        }
      }
    };
    const promise = Promise.resolve().then(run);
    inFlight.set(column.id, promise);
    signatures.set(column.id, signature);
    await promise;
  };

  const loadMoreTimeline: AppStore["loadMoreTimeline"] = async (column) => {
    if (timelineDescriptor(column.columnType)?.pagination === "none") return;
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
    const maxStatus = oldest(current);
    const accountAcct = requestAccountAcct(column);
    const request: TimelineRequest = {
      columnType: column.columnType,
      columnParam: column.columnParam,
      limit,
      quoteConsumerId: column.id,
      offset: current.length,
      maxStatusId: maxStatus?.id,
      maxServerDomain: maxStatus?.serverDomain,
      ...(accountAcct ? { accountAcct } : {}),
      displayFilter: timelineDisplayFilterApplies(column)
        ? normalizeDisplayFilter(column.displayFilter)
        : undefined,
    };
    const startedAt = performance.now();
    const resourceKey = `timeline:${column.id}:more`;
    let generation = 0;
    console.info(
      `[awayuki][ui-timeline] load_more_start ${logContext(column)} offset=${request.offset} limit=${limit}`,
    );
    set((state) => ({ loadingMore: { ...state.loadingMore, [column.id]: true } }));
    try {
      const response = await frontendRequestScheduler.schedule(
        {
          key: resourceKey,
          lane: requestLane(column),
          priority: isVisible(get(), column) ? 90 : -10,
          replace: false,
        },
        async (context) => {
          generation = context.generation;
          set((state) => ({
            resourceStates: reduceResourceStates(state.resourceStates, {
              type: "begin",
              key: resourceKey,
              generation,
            }),
          }));
          const page = canLoadMoreFromApi(column)
            ? await cancellableRead("load_more_timeline", request, context.signal)
            : {
                statuses: await cancellableRead("load_timeline", request, context.signal),
                hasMore: undefined,
              };
          if (!context.isCurrent()) throw new RequestCancelledError(resourceKey);
          return page;
        },
      );
      const displayed = filterStatuses(response.statuses, column);
      console.info(
        `[awayuki][ui-timeline] load_more_success ${logContext(column)} offset=${request.offset} count=${response.statuses.length} display_count=${displayed.length} duration_ms=${elapsed(startedAt)}`,
      );
      set((state) =>
        state.resourceStates[resourceKey]?.generation === generation
          ? {
              ...entityPatch(state, [
                {
                  type: "appendPage",
                  columnId: column.id,
                  statuses: displayed,
                  limit: timelineDisplayLimit(column),
                },
              ]),
              loadingMore: { ...state.loadingMore, [column.id]: false },
              resourceStates: reduceResourceStates(state.resourceStates, {
                type: "succeed",
                key: resourceKey,
                generation,
              }),
              timelineHasMore: {
                ...state.timelineHasMore,
                [column.id]:
                  typeof response.hasMore === "boolean"
                    ? response.hasMore
                    : pageHasMore(response.statuses.length, limit),
              },
            }
          : {},
      );
      if (canLoadMoreFromApi(column)) {
        get().applyTimelineCacheCommit();
      }
    } catch (error) {
      const cancelled = error instanceof RequestCancelledError;
      console.error(
        `[awayuki][ui-timeline] load_more_error ${logContext(column)} offset=${request.offset} duration_ms=${elapsed(startedAt)} error=${String(error)}`,
      );
      set((state) =>
        state.resourceStates[resourceKey]?.generation === generation
          ? {
              loadingMore: { ...state.loadingMore, [column.id]: false },
              resourceStates: reduceResourceStates(
                state.resourceStates,
                cancelled
                  ? { type: "cancel", key: resourceKey, generation }
                  : {
                      type: "fail",
                      key: resourceKey,
                      generation,
                      error: String(error),
                    },
              ),
            }
          : {},
      );
    } finally {
      set({ requestMetrics: frontendRequestScheduler.metrics() });
    }
  };

  return { loadTimeline, loadMoreTimeline };
}
