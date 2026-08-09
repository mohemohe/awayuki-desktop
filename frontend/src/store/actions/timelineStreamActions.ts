import type { StoreApi } from "zustand";

import {
  recordFrontendStreamGap,
  recordFrontendStreamResync,
  setPendingStreamEvents,
} from "../../api/observability";
import {
  timelineDescriptor,
  timelineTypeIsAnalytical,
} from "../../domain/timelineDescriptors";
import {
  canonicalStatusKey,
  statusKey,
  type StatusKey,
  type TimelineEntityOperation,
} from "../../domain/timelineEntities";
import type {
  ColumnSummary,
  TimelineStatus,
  TimelineStreamEvent,
} from "../../types/app";
import { incrementUnreadResources } from "../slices/notifications";
import type { AppStore, StreamPerformanceSnapshot } from "../appStore";
import {
  markNextRenderScenario,
  measureNextPaint,
} from "../../utils/renderMetrics";
import { AnalyticalTimelineRefreshCoordinator } from "../analyticalTimelineRefresh";

type TimelineEntityPatch = Pick<
  AppStore,
  | "entities"
  | "timelineKeys"
  | "timelineDeferredKeys"
  | "timelineUnread"
  | "timelineHasMore"
  | "canonicalIndex"
  | "timelines"
>;
type ColumnMembership = {
  canonicals: Set<StatusKey>;
  entries: Set<StatusKey>;
  canonicalByEntry: Map<StatusKey, StatusKey>;
};

type TimelineStreamContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  allColumns: (state: AppStore) => ColumnSummary[];
  entityPatch: (
    state: AppStore,
    operations: TimelineEntityOperation[],
  ) => TimelineEntityPatch;
  syncAllConsumers: (
    state: AppStore,
    entityState: TimelineEntityPatch,
  ) => Partial<AppStore>;
  columnMatchesEventAccount: (
    column: ColumnSummary,
    sourceAcct: string,
  ) => boolean;
  statusMatchesDisplayFilter: (
    status: TimelineStatus,
    column: ColumnSummary,
  ) => boolean;
  timelineDisplayLimit: (column: ColumnSummary) => number;
  timelineStatusMatchesSearchQuery: (
    status: TimelineStatus,
    query: string,
  ) => boolean;
  accountIdentityKey: (acct: string) => string;
};

let activeFlushForTest: (() => void) | undefined;
let activeAnalyticalCoordinator:
  | AnalyticalTimelineRefreshCoordinator
  | undefined;

export function flushTimelineStreamEventsForTest() {
  activeFlushForTest?.();
}

export async function flushAnalyticalTimelineRefreshesForTest() {
  await activeAnalyticalCoordinator?.flushForTest();
}

export function activateAnalyticalTimelineRefresh(column: ColumnSummary) {
  activeAnalyticalCoordinator?.activate(column);
}

export function cancelAnalyticalTimelineRefresh(columnId: string) {
  activeAnalyticalCoordinator?.cancel(columnId);
}

export function resetAnalyticalTimelineRefreshes() {
  activeAnalyticalCoordinator?.reset();
}

function columnReceivesStreamStatus(column: ColumnSummary, streamType: string) {
  const policy = timelineDescriptor(column.columnType)?.streamPolicy;
  if (policy === "home") return streamType === "user";
  if (policy === "public") return streamType === "public";
  if (policy === "local") return streamType === "public:local";
  if (policy === "hashtag") return streamType === `hashtag:${column.columnParam}`;
  if (policy === "list") return streamType === `list:${column.columnParam}`;
  return false;
}

export function createTimelineStreamActions({
  set,
  get,
  allColumns,
  entityPatch,
  syncAllConsumers,
  columnMatchesEventAccount,
  statusMatchesDisplayFilter,
  timelineDisplayLimit,
  timelineStatusMatchesSearchQuery,
  accountIdentityKey,
}: TimelineStreamContext): Pick<
  AppStore,
  "applyStreamEvent" | "applyTimelineCacheCommit"
> {
  const pendingEvents = new Map<string, TimelineStreamEvent>();
  const positions = new Map<string, { generation: number; sequence: number }>();
  const resyncs = new Set<string>();
  const batchDurations: number[] = [];
  let frame: number | undefined;
  let timer: number | undefined;

  const columnIsActive = (column: ColumnSummary) => {
    const state = get();
    const selected = state.activeTabs[column.paneIndex];
    if (selected) return selected === column.id;
    const first = allColumns(state)
      .filter((candidate) => candidate.paneIndex === column.paneIndex)
      .sort((left, right) => left.position - right.position)[0];
    return first?.id === column.id;
  };

  activeAnalyticalCoordinator?.reset();
  const analyticalCoordinator = new AnalyticalTimelineRefreshCoordinator({
    canAutoRefresh: (column) =>
      columnIsActive(column) && (get().timelineNearTop[column.id] ?? true),
    refresh: async (column) => {
      await get().loadTimeline(column, true);
      if (get().resourceStates[`timeline:${column.id}`]?.phase !== "succeeded") {
        throw new Error(`analytical timeline refresh failed: ${column.id}`);
      }
    },
  });
  activeAnalyticalCoordinator = analyticalCoordinator;

  const streamIdentity = (event: TimelineStreamEvent) =>
    `${event.serverDomain.toLowerCase()}:${accountIdentityKey(event.sourceAcct)}`;

  const eventKey = (event: TimelineStreamEvent) => {
    if (event.kind === "newNotification" && event.status?.notificationId) {
      return `notification:${event.serverDomain.toLowerCase()}:${event.status.notificationId}:${accountIdentityKey(event.sourceAcct)}`;
    }
    const statusId =
      event.statusId ?? event.status?.originalStatusId ?? event.status?.id;
    return statusId
      ? `status:${event.serverDomain.toLowerCase()}:${statusId}:${event.streamType}:${accountIdentityKey(event.sourceAcct)}`
      : `${event.kind}:${event.streamType}:${event.sourceAcct}`;
  };

  const coalesceEvent = (
    current: TimelineStreamEvent,
    next: TimelineStreamEvent,
  ) => {
    if (next.kind === "deleteStatus") return next;
    if (current.kind === "deleteStatus") return current;
    if (current.kind === "newStatus" && next.kind === "statusUpdate") {
      return { ...next, kind: "newStatus" as const };
    }
    return next;
  };

  const recordPosition = (event: TimelineStreamEvent) => {
    if (event.generation === undefined || event.sequence === undefined) {
      return "untracked" as const;
    }
    const key = streamIdentity(event);
    const previous = positions.get(key);
    positions.set(key, {
      generation: event.generation,
      sequence: event.sequence,
    });
    if (!previous || event.kind === "resync") return "continuous" as const;
    return previous.generation === event.generation &&
      previous.sequence + 1 === event.sequence
      ? ("continuous" as const)
      : ("gap" as const);
  };

  const scheduleResync = (event: TimelineStreamEvent) => {
    const key = streamIdentity(event);
    if (resyncs.has(key)) return;
    const state = get();
    const columns = allColumns(state).filter((column) =>
      columnMatchesEventAccount(column, event.sourceAcct),
    );
    const analytical = columns.filter((column) =>
      timelineTypeIsAnalytical(column.columnType),
    );
    const snapshots = columns.filter(
      (column) => !timelineTypeIsAnalytical(column.columnType),
    );
    resyncs.add(key);
    void Promise.all(
      snapshots.map((column) => state.loadTimeline(column, true)),
    ).finally(() => {
      for (const column of analytical) analyticalCoordinator.invalidate(column);
      resyncs.delete(key);
    });
  };

  const buildMembership = (
    state: Pick<
      AppStore,
      "timelineKeys" | "timelineDeferredKeys" | "entities"
    >,
  ) =>
    new Map(
      [...new Set([
        ...Object.keys(state.timelineKeys),
        ...Object.keys(state.timelineDeferredKeys),
      ])].map((columnId) => {
        const keys = [
          ...(state.timelineKeys[columnId] ?? []),
          ...(state.timelineDeferredKeys[columnId] ?? []),
        ];
        return [
          columnId,
          {
            canonicals: new Set(
              keys.flatMap((key) => {
                const status = state.entities.get(key);
                return status ? [canonicalStatusKey(status)] : [];
              }),
            ),
            entries: new Set(keys),
            canonicalByEntry: new Map(
              keys.flatMap((key) => {
                const status = state.entities.get(key);
                return status ? ([[key, canonicalStatusKey(status)]] as const) : [];
              }),
            ),
          },
        ] as const;
      }),
    );

  const addMembership = (
    membership: Map<string, ColumnMembership>,
    columnId: string,
    entry: StatusKey,
    canonical: StatusKey,
  ) => {
    let column = membership.get(columnId);
    if (!column) {
      column = {
        canonicals: new Set(),
        entries: new Set(),
        canonicalByEntry: new Map(),
      };
      membership.set(columnId, column);
    }
    column.canonicals.add(canonical);
    column.entries.add(entry);
    column.canonicalByEntry.set(entry, canonical);
  };

  const removeMembership = (
    membership: Map<string, ColumnMembership>,
    columnId: string,
    canonical: StatusKey,
  ) => {
    const column = membership.get(columnId);
    if (!column) return;
    column.canonicals.delete(canonical);
    for (const [entry, entryCanonical] of column.canonicalByEntry) {
      if (entryCanonical !== canonical) continue;
      column.entries.delete(entry);
      column.canonicalByEntry.delete(entry);
    }
  };

  const collectSqlInvalidations = (
    state: Pick<AppStore, "timelines" | "timelineNearTop" | "activeTabs">,
    columns: ColumnSummary[],
    membership: Map<string, ColumnMembership>,
    event: TimelineStreamEvent,
    refreshColumns: Map<string, ColumnSummary>,
    unreadColumns: Map<string, number>,
  ) => {
    const invalidateThread = (column: ColumnSummary) => {
      if (state.timelineNearTop[column.id] ?? true) {
        refreshColumns.set(column.id, column);
      } else {
        unreadColumns.set(column.id, (unreadColumns.get(column.id) ?? 0) + 1);
      }
    };
    const invalidateAnalytical = (column: ColumnSummary) => {
      if (
        !columnIsActive(column) ||
        !(state.timelineNearTop[column.id] ?? true)
      ) {
        unreadColumns.set(column.id, (unreadColumns.get(column.id) ?? 0) + 1);
      }
    };
    for (const column of columns) {
      if (!columnMatchesEventAccount(column, event.sourceAcct)) continue;
      if (
        !timelineTypeIsAnalytical(column.columnType) &&
        column.columnType !== "thread"
      ) {
        continue;
      }
      if (timelineTypeIsAnalytical(column.columnType)) {
        // Arbitrary SQL/YQ/KQ membership cannot be inferred safely from a generic
        // live event. It only updates unread state here; the post-commit event
        // is the authority that marks the query dirty and schedules a refresh.
        invalidateAnalytical(column);
        continue;
      }
      if (event.kind === "newNotification") continue;
      if (event.kind === "newStatus") {
        continue;
      }
      const eventStatus = event.status;
      const contains = eventStatus
        ? membership
            .get(column.id)
            ?.canonicals.has(canonicalStatusKey(eventStatus)) ?? false
        : Boolean(
            event.statusId &&
              state.timelines[column.id]?.some(
                (status) =>
                  status.serverDomain === event.serverDomain &&
                  (status.id === event.statusId ||
                    status.originalStatusId === event.statusId),
              ),
          );
      if (contains) invalidateThread(column);
    }
  };

  const deletesStatus = (event: TimelineStreamEvent, status: TimelineStatus) =>
    event.kind === "deleteStatus" &&
    event.serverDomain.toLowerCase() === status.serverDomain.toLowerCase() &&
    Boolean(
      event.statusId &&
        (event.statusId === status.id || event.statusId === status.originalStatusId),
    );

  const recordPerformance = (
    current: StreamPerformanceSnapshot,
    batchSize: number,
    durationMs: number,
  ) => {
    batchDurations.push(durationMs);
    if (batchDurations.length > 120) batchDurations.shift();
    const sorted = [...batchDurations].sort((left, right) => left - right);
    const p95Index = Math.max(0, Math.ceil(sorted.length * 0.95) - 1);
    return {
      batches: current.batches + 1,
      lastBatchSize: batchSize,
      lastDurationMs: durationMs,
      p95DurationMs: sorted[p95Index] ?? durationMs,
    };
  };

  const flush = () => {
    if (frame !== undefined) {
      window.cancelAnimationFrame(frame);
      frame = undefined;
    }
    if (timer !== undefined) {
      window.clearTimeout(timer);
      timer = undefined;
    }
    const events = [...pendingEvents.values()];
    pendingEvents.clear();
    setPendingStreamEvents(0);
    if (events.length === 0) return;

    const startedAt = performance.now();
    const refreshColumns = new Map<string, ColumnSummary>();
    markNextRenderScenario("timeline:stream");
    measureNextPaint("timeline:stream");
    set((state) => {
      const columns = allColumns(state);
      if (columns.length === 0) return {};
      const membership = buildMembership(state);
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
          const deletedCanonicals = new Set(
            [...state.entities.values()]
              .filter(
                (status) =>
                  status.serverDomain.toLowerCase() ===
                    event.serverDomain.toLowerCase() &&
                  (status.id === event.statusId ||
                    status.originalStatusId === event.statusId),
              )
              .map(canonicalStatusKey),
          );
          for (const [columnId] of membership) {
            for (const canonical of deletedCanonicals) {
              removeMembership(membership, columnId, canonical);
            }
          }
          continue;
        }

        const eventStatus = event.status;
        if (!eventStatus) continue;
        const canonical = canonicalStatusKey(eventStatus);
        const entry = statusKey(eventStatus);
        const matchingColumns: ColumnSummary[] = [];
        const removeFromColumns: string[] = [];

        for (const column of columns) {
          if (!columnMatchesEventAccount(column, event.sourceAcct)) continue;
          const columnMembership = membership.get(column.id);
          const contains = event.kind === "newNotification"
            ? columnMembership?.entries.has(entry) ?? false
            : columnMembership?.canonicals.has(canonical) ?? false;
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
          } else {
            const receives =
              event.kind === "newNotification"
                ? timelineDescriptor(column.columnType)?.streamPolicy ===
                  "notification"
                : columnReceivesStreamStatus(column, event.streamType) || contains;
            if (!receives) continue;
            if (!statusMatchesDisplayFilter(eventStatus, column)) {
              if (contains) removeFromColumns.push(column.id);
              continue;
            }
            matchingColumns.push(column);
          }
          if (
            event.kind !== "statusUpdate" &&
            !(state.timelineNearTop[column.id] ?? true)
          ) {
            preserveAnchorColumns.add(column.id);
          }
        }

        if (removeFromColumns.length > 0) {
          operations.push({
            type: "removeFromColumns",
            target: eventStatus,
            columnIds: removeFromColumns,
          });
          for (const columnId of removeFromColumns) {
            removeMembership(membership, columnId, canonical);
          }
        }
        if (event.kind !== "statusUpdate") {
          for (const column of matchingColumns) {
            addMembership(membership, column.id, entry, canonical);
          }
        }
        operations.push({
          type: "upsertInColumns",
          columnIds: matchingColumns.map((column) => column.id),
          status: eventStatus,
          // Near-top columns are always capped at their display limit, even
          // when explicit pagination previously grew past it — otherwise a
          // long-running session accumulates every streamed status. Rows the
          // cap drops re-enable load-more, so trimmed history stays
          // reachable. Anchor-preserving columns route insertions to deferred
          // keys, where the same limit bounds the pending backlog without
          // touching visible rows.
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

      const patch = entityPatch(state, operations);
      const mediaDeleted = Boolean(
        state.mediaPreview &&
          events.some((event) => deletesStatus(event, state.mediaPreview!.status)),
      );
      const composeDeleted = Boolean(
        state.composeTarget &&
          events.some((event) => deletesStatus(event, state.composeTarget!.status)),
      );
      return {
        ...patch,
        timelineUnread: incrementUnreadResources(
          patch.timelineUnread,
          unreadColumns,
        ),
        ...syncAllConsumers(state, patch),
        ...(mediaDeleted ? { mediaPreview: null } : {}),
        ...(composeDeleted ? { composeTarget: null } : {}),
        streamPerformance: recordPerformance(
          state.streamPerformance,
          events.length,
          performance.now() - startedAt,
        ),
      };
    });

    for (const column of refreshColumns.values()) {
      void get().loadTimeline(column, true);
    }
  };

  const queue = (event: TimelineStreamEvent) => {
    const continuity = recordPosition(event);
    if (continuity === "gap") recordFrontendStreamGap();
    if (event.kind === "resync") recordFrontendStreamResync();
    if (event.kind === "resync" || continuity === "gap") {
      scheduleResync(event);
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
    const key = eventKey(normalized);
    const existing = pendingEvents.get(key);
    pendingEvents.set(
      key,
      existing ? coalesceEvent(existing, normalized) : normalized,
    );
    setPendingStreamEvents(pendingEvents.size);
    if (frame !== undefined || timer !== undefined) return;
    frame = window.requestAnimationFrame(flush);
    timer = window.setTimeout(flush, 40);
  };

  activeFlushForTest = flush;
  return {
    applyStreamEvent: queue,
    applyTimelineCacheCommit: () => {
      for (const column of allColumns(get())) {
        if (timelineTypeIsAnalytical(column.columnType)) {
          analyticalCoordinator.invalidate(column);
        }
      }
    },
  };
}
