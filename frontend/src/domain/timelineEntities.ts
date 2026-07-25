import type { TimelineStatus } from "../types/app";

export type StatusKey = string;
export type CanonicalAliases = StatusKey | ReadonlySet<StatusKey>;
export type CanonicalIndex = Map<StatusKey, CanonicalAliases>;

const createdAtTimestampCache = new WeakMap<TimelineStatus, number>();

export type TimelineEntityState = {
  entities: Map<StatusKey, TimelineStatus>;
  columnKeys: Record<string, StatusKey[]>;
  deferredColumnKeys: Record<string, StatusKey[]>;
  canonicalIndex: CanonicalIndex;
  timelines: Record<string, TimelineStatus[]>;
};

export type TimelineEntityOperation =
  | {
      type: "replaceColumn";
      columnId: string;
      statuses: TimelineStatus[];
      limit: number;
    }
  | {
      type: "appendPage";
      columnId: string;
      statuses: TimelineStatus[];
    }
  | {
      type: "mergeDelta";
      columnId: string;
      statuses: TimelineStatus[];
      limit: number;
    }
  | {
      type: "upsertInColumns";
      columnIds: string[];
      status: TimelineStatus;
      limits: Partial<Record<string, number>>;
      updateOnly?: boolean;
      preserveAnchorColumns?: ReadonlySet<string>;
    }
  | { type: "flushDeferredColumn"; columnId: string; limit: number }
  | { type: "replaceCanonical"; target: TimelineStatus; status: TimelineStatus }
  | {
      type: "patchCanonical";
      target: TimelineStatus;
      patch: Partial<TimelineStatus>;
    }
  | { type: "removeCanonical"; target: TimelineStatus }
  | {
      type: "removeFromColumns";
      target: TimelineStatus;
      columnIds: string[];
    }
  | {
      type: "removeCanonicalId";
      serverDomain: string;
      statusId: string;
    }
  | { type: "removeColumn"; columnId: string };

export function createTimelineEntityState(): TimelineEntityState {
  return {
    entities: new Map(),
    columnKeys: {},
    deferredColumnKeys: {},
    canonicalIndex: new Map(),
    timelines: {},
  };
}

export function normalizeTimelineLimit(limit: number) {
  const finite = Number.isFinite(limit) ? Math.floor(limit) : 100;
  return Math.max(1, finite || 100);
}

/**
 * Canonical identity is protocol aware and never relies on a server-local ID
 * without including its server. Timeline wrappers (notifications/reblogs) use
 * a distinct `statusKey`, but are indexed by this canonical subject key so a
 * single mutation can update every representation deterministically.
 */
export function canonicalStatusKey(status: TimelineStatus): StatusKey {
  const uri = status.uri?.trim();
  if (uri?.startsWith("at://")) return `atproto:${uri}`;
  if (uri) {
    try {
      const parsed = new URL(uri);
      if (parsed.protocol === "http:" || parsed.protocol === "https:") {
        parsed.hostname = parsed.hostname.toLowerCase();
        parsed.hash = "";
        return `activitypub:${parsed.toString()}`;
      }
    } catch {
      // A malformed remote URI is not trusted as a globally unique identity.
    }
  }
  return canonicalStatusIdKey(
    status.serverDomain,
    status.originalStatusId || status.id,
  );
}

export function canonicalStatusIdKey(
  serverDomain: string,
  statusId: string,
): StatusKey {
  return `server:${serverDomain.trim().toLowerCase()}:status:${statusId}`;
}

export function statusKey(status: TimelineStatus): StatusKey {
  const canonical = canonicalStatusKey(status);
  return statusKeyFromCanonical(status, canonical);
}

function statusKeyFromCanonical(
  status: TimelineStatus,
  canonical: StatusKey,
): StatusKey {
  if (status.notificationId) {
    const sourceAcct =
      status.sourceAcct?.trim().replace(/^@+/, "").toLowerCase() ?? "";
    return `${canonical}:notification:${status.serverDomain.toLowerCase()}:${sourceAcct}:${status.notificationId}`;
  }
  if (
    status.id &&
    status.originalStatusId &&
    status.id !== status.originalStatusId
  ) {
    return `${canonical}:reblog:${status.serverDomain.toLowerCase()}:${status.id}`;
  }
  return canonical;
}

export function reduceTimelineEntities(
  previous: TimelineEntityState,
  operations: TimelineEntityOperation[],
): TimelineEntityState {
  if (operations.length === 0) return previous;

  const mutable = {
    entities: new Map(previous.entities),
    columnKeys: { ...previous.columnKeys },
    deferredColumnKeys: { ...previous.deferredColumnKeys },
    // The server-local ID index is only needed by delete operations. Building
    // it eagerly made every page append copy all loaded IDs first.
    canonicalIndex: new Map(previous.canonicalIndex),
    serverIdIndex: undefined,
  };
  let reusableReplacement:
    | {
        statuses: TimelineStatus[];
        limit: number;
        keys: StatusKey[];
      }
    | undefined;

  for (
    let operationIndex = 0;
    operationIndex < operations.length;
    operationIndex += 1
  ) {
    const operation = operations[operationIndex];
    if (!operation) continue;
    if (operation.type !== "replaceColumn") reusableReplacement = undefined;
    switch (operation.type) {
      case "replaceColumn": {
        const limit = normalizeTimelineLimit(operation.limit);
        const keys =
          reusableReplacement?.statuses === operation.statuses &&
          reusableReplacement.limit === limit
            ? reusableReplacement.keys
            : normalizeStatusList(mutable, operation.statuses, limit);
        reusableReplacement = { statuses: operation.statuses, limit, keys };
        setColumnKeys(
          mutable,
          operation.columnId,
          keys,
        );
        break;
      }
      case "appendPage": {
        const current = mutable.columnKeys[operation.columnId] ?? [];
        const appended = normalizeStatusList(mutable, operation.statuses);
        const membership = new Set(current);
        const newKeys = appended.filter((key) => {
          if (membership.has(key)) return false;
          membership.add(key);
          return true;
        });
        if (newKeys.length > 0) {
          setColumnKeys(mutable, operation.columnId, current.concat(newKeys));
        }
        break;
      }
      case "mergeDelta": {
        const incoming = normalizeStatusList(
          mutable,
          operation.statuses,
          operation.limit,
        );
        const current = mutable.columnKeys[operation.columnId] ?? [];
        setColumnKeys(
          mutable,
          operation.columnId,
          mergeOrderedKeys(
            mutable.entities,
            incoming,
            current,
            operation.limit,
          ),
        );
        break;
      }
      case "upsertInColumns": {
        const batch = [operation];
        const touchesExisting = operationTouchesExistingIdentity(
          mutable,
          operation,
        );
        const limits = new Map<string, number | undefined>();
        const targets = new Map<string, "visible" | "deferred">();
        const identities = new Map<string, Set<StatusKey>>();
        recordBatchPolicy(limits, targets, identities, operation);
        while (operationIndex + 1 < operations.length) {
          const next = operations[operationIndex + 1];
          if (
            !next ||
            next.type !== "upsertInColumns" ||
            touchesExisting ||
            operationTouchesExistingIdentity(mutable, next) ||
            Boolean(next.updateOnly) !== Boolean(operation.updateOnly) ||
            !recordBatchPolicy(limits, targets, identities, next)
          ) {
            break;
          }
          batch.push(next);
          operationIndex += 1;
        }
        applyUpsertBatch(mutable, batch, limits);
        break;
      }
      case "flushDeferredColumn": {
        const deferred = mutable.deferredColumnKeys[operation.columnId] ?? [];
        if (deferred.length === 0) break;
        setColumnKeys(
          mutable,
          operation.columnId,
          mergeOrderedKeys(
            mutable.entities,
            deferred,
            mutable.columnKeys[operation.columnId] ?? [],
            operation.limit,
          ),
        );
        delete mutable.deferredColumnKeys[operation.columnId];
        break;
      }
      case "replaceCanonical": {
        upsertStatus(mutable, operation.status);
        replaceCanonicalAliases(
          mutable,
          canonicalStatusKey(operation.target),
          operation.status,
        );
        break;
      }
      case "patchCanonical": {
        patchCanonicalAliases(
          mutable,
          canonicalStatusKey(operation.target),
          operation.patch,
        );
        break;
      }
      case "removeCanonical": {
        removeCanonicalAliases(mutable, canonicalStatusKey(operation.target));
        break;
      }
      case "removeFromColumns": {
        const aliases = mutable.canonicalIndex.get(
          canonicalStatusKey(operation.target),
        );
        if (!aliases) break;
        for (const columnId of operation.columnIds) {
          const current = mutable.columnKeys[columnId];
          if (current) {
            setColumnKeys(
              mutable,
              columnId,
              current.filter((key) => !canonicalAliasesHas(aliases, key)),
            );
          }
          const deferred = mutable.deferredColumnKeys[columnId];
          if (deferred) {
            setDeferredColumnKeys(
              mutable,
              columnId,
              deferred.filter((key) => !canonicalAliasesHas(aliases, key)),
            );
          }
        }
        break;
      }
      case "removeCanonicalId": {
        removeCanonicalById(
          mutable,
          operation.serverDomain,
          operation.statusId,
        );
        break;
      }
      case "removeColumn": {
        delete mutable.columnKeys[operation.columnId];
        delete mutable.deferredColumnKeys[operation.columnId];
        break;
      }
    }
  }

  relinkNestedStatuses(mutable);
  pruneUnreferencedEntities(mutable);
  const timelines = materializeTimelines(mutable.entities, mutable.columnKeys);
  return {
    entities: mutable.entities,
    columnKeys: mutable.columnKeys,
    deferredColumnKeys: mutable.deferredColumnKeys,
    canonicalIndex: mutable.canonicalIndex,
    timelines,
  };
}

export function statusForCanonical(
  state: Pick<TimelineEntityState, "entities" | "canonicalIndex">,
  target: TimelineStatus,
) {
  const aliases = state.canonicalIndex.get(canonicalStatusKey(target));
  if (!aliases) return undefined;
  for (const key of canonicalAliasKeys(aliases)) {
    const status = state.entities.get(key);
    if (status) return status;
  }
  return undefined;
}

type MutableTimelineEntityState = Pick<
  TimelineEntityState,
  "entities" | "columnKeys" | "deferredColumnKeys" | "canonicalIndex"
> & {
  serverIdIndex: Map<string, Set<StatusKey>> | undefined;
};

type UpsertInColumnsOperation = Extract<
  TimelineEntityOperation,
  { type: "upsertInColumns" }
>;

type PendingInsertion = {
  key: StatusKey;
  timestamp: number;
  sequence: number;
};

type PendingColumnChanges = {
  currentKeys: StatusKey[];
  currentMembership: Set<StatusKey>;
  currentInsertions: PendingInsertion[];
  deferredKeys: StatusKey[];
  deferredMembership: Set<StatusKey>;
  deferredInsertions: PendingInsertion[];
  deferredRemovals: Set<StatusKey>;
};

function setColumnKeys(
  state: MutableTimelineEntityState,
  columnId: string,
  keys: StatusKey[],
) {
  state.columnKeys[columnId] = keys;
}

function setDeferredColumnKeys(
  state: MutableTimelineEntityState,
  columnId: string,
  keys: StatusKey[],
) {
  if (keys.length === 0) {
    delete state.deferredColumnKeys[columnId];
  } else {
    state.deferredColumnKeys[columnId] = keys;
  }
}

function operationTouchesExistingIdentity(
  state: MutableTimelineEntityState,
  operation: UpsertInColumnsOperation,
) {
  const canonical = canonicalStatusKey(operation.status);
  const existingAliases = state.canonicalIndex.get(canonical);
  const notificationKey = operation.status.notificationId
    ? statusKeyFromCanonical(operation.status, canonical)
    : undefined;
  const aliases = notificationKey
    ? existingAliases && canonicalAliasesHas(existingAliases, notificationKey)
      ? [notificationKey]
      : []
    : existingAliases
      ? [...canonicalAliasKeys(existingAliases)]
      : [];
  if (aliases.length === 0) return false;
  return operation.columnIds.some((columnId) =>
    aliases.some(
      (key) =>
        state.columnKeys[columnId]?.includes(key) ||
        state.deferredColumnKeys[columnId]?.includes(key),
    ),
  );
}

function recordBatchPolicy(
  limits: Map<string, number | undefined>,
  targets: Map<string, "visible" | "deferred">,
  identities: Map<string, Set<StatusKey>>,
  operation: UpsertInColumnsOperation,
) {
  const canonical = canonicalStatusKey(operation.status);
  const identity = operation.status.notificationId
    ? statusKeyFromCanonical(operation.status, canonical)
    : canonical;
  for (const columnId of operation.columnIds) {
    const limit = normalizeOptionalTimelineLimit(operation.limits[columnId]);
    if (limits.has(columnId) && limits.get(columnId) !== limit) return false;
    const target = operation.preserveAnchorColumns?.has(columnId)
      ? "deferred"
      : "visible";
    if (targets.has(columnId) && targets.get(columnId) !== target) return false;
    if (identities.get(columnId)?.has(identity)) return false;
  }
  for (const columnId of operation.columnIds) {
    if (!limits.has(columnId)) {
      limits.set(
        columnId,
        normalizeOptionalTimelineLimit(operation.limits[columnId]),
      );
    }
    if (!targets.has(columnId)) {
      targets.set(
        columnId,
        operation.preserveAnchorColumns?.has(columnId)
          ? "deferred"
          : "visible",
      );
    }
    const seen = identities.get(columnId);
    if (seen) seen.add(identity);
    else identities.set(columnId, new Set([identity]));
  }
  return true;
}

function applyUpsertBatch(
  state: MutableTimelineEntityState,
  operations: UpsertInColumnsOperation[],
  limits: ReadonlyMap<string, number | undefined>,
) {
  const pending = new Map<string, PendingColumnChanges>();
  let sequence = 0;
  const changesFor = (columnId: string) => {
    const existing = pending.get(columnId);
    if (existing) return existing;
    const currentKeys = state.columnKeys[columnId] ?? [];
    const deferredKeys = state.deferredColumnKeys[columnId] ?? [];
    const created: PendingColumnChanges = {
      currentKeys,
      currentMembership: new Set(currentKeys),
      currentInsertions: [],
      deferredKeys,
      deferredMembership: new Set(deferredKeys),
      deferredInsertions: [],
      deferredRemovals: new Set(),
    };
    pending.set(columnId, created);
    return created;
  };

  for (const operation of operations) {
    const { key, canonical } = upsertStatusIdentity(
      state,
      operation.status,
    );
    replaceCanonicalAliases(state, canonical, operation.status);
    const timestamp = createdAtTimestamp(state.entities.get(key));
    const insertionSequence = sequence;
    sequence += 1;

    for (const columnId of operation.columnIds) {
      const changes = changesFor(columnId);
      // A notification is an event wrapper, not the canonical post itself.
      // Multiple users can boost/favourite the same post, and every event
      // remains visible in the Unified Notification Timeline.
      const existingKey = operation.status.notificationId
        ? changes.currentMembership.has(key)
          ? key
          : undefined
        : findCanonicalKeyInMembership(
            state,
            changes.currentMembership,
            canonical,
          );
      if (existingKey) {
        // The entity was updated above, but its existing wrapper and position
        // are stable so scroll anchors do not move on a status update.
        continue;
      }
      const existingDeferredKey = operation.status.notificationId
        ? changes.deferredMembership.has(key)
          ? key
          : undefined
        : findCanonicalKeyInMembership(
            state,
            changes.deferredMembership,
            canonical,
          );
      if (existingDeferredKey) {
        // A duplicate arriving after the viewport returned to the top promotes
        // the deferred row. The same path covers a cache refresh racing a post.
        if (
          !operation.updateOnly &&
          !operation.preserveAnchorColumns?.has(columnId)
        ) {
          changes.deferredMembership.delete(existingDeferredKey);
          changes.deferredRemovals.add(existingDeferredKey);
          if (!changes.currentMembership.has(existingDeferredKey)) {
            changes.currentMembership.add(existingDeferredKey);
            changes.currentInsertions.push({
              key: existingDeferredKey,
              timestamp: createdAtTimestamp(
                state.entities.get(existingDeferredKey),
              ),
              sequence: insertionSequence,
            });
          }
        }
        continue;
      }
      if (operation.updateOnly) continue;

      const insertion = { key, timestamp, sequence: insertionSequence };
      if (operation.preserveAnchorColumns?.has(columnId)) {
        changes.deferredMembership.add(key);
        changes.deferredInsertions.push(insertion);
      } else {
        changes.currentMembership.add(key);
        changes.currentInsertions.push(insertion);
      }
    }
  }

  for (const [columnId, changes] of pending) {
    const limit = limits.get(columnId);
    if (changes.currentInsertions.length > 0) {
      setColumnKeys(
        state,
        columnId,
        mergePendingInsertions(
          state.entities,
          changes.currentKeys,
          changes.currentInsertions,
          limit,
        ),
      );
    }
    if (
      changes.deferredInsertions.length > 0 ||
      changes.deferredRemovals.size > 0
    ) {
      const retainedDeferred = changes.deferredKeys.filter(
        (key) => !changes.deferredRemovals.has(key),
      );
      const retainedInsertions = changes.deferredInsertions.filter(
        ({ key }) => !changes.deferredRemovals.has(key),
      );
      setDeferredColumnKeys(
        state,
        columnId,
        mergePendingInsertions(
          state.entities,
          retainedDeferred,
          retainedInsertions,
          limit,
        ),
      );
    }
  }
}

function mergePendingInsertions(
  entities: Map<StatusKey, TimelineStatus>,
  current: StatusKey[],
  insertions: PendingInsertion[],
  limit?: number,
) {
  if (insertions.length === 0) {
    return limit === undefined
      ? current
      : current.slice(0, normalizeTimelineLimit(limit));
  }
  const incoming = [...insertions].sort((left, right) => {
    if (left.timestamp !== right.timestamp) {
      return right.timestamp - left.timestamp;
    }
    return left.sequence - right.sequence;
  });
  const result: StatusKey[] = [];
  const seen = new Set<StatusKey>();
  const normalizedLimit = normalizeOptionalTimelineLimit(limit);
  let currentIndex = 0;
  let incomingIndex = 0;
  while (
    (normalizedLimit === undefined || result.length < normalizedLimit) &&
    (currentIndex < current.length || incomingIndex < incoming.length)
  ) {
    const currentKey = current[currentIndex];
    const incomingEntry = incoming[incomingIndex];
    let next: StatusKey;
    if (currentKey === undefined) {
      next = incomingEntry.key;
      incomingIndex += 1;
    } else if (incomingEntry === undefined) {
      next = currentKey;
      currentIndex += 1;
    } else if (
      createdAtTimestamp(entities.get(currentKey)) >= incomingEntry.timestamp
    ) {
      // Sequential insertion places a new row after existing rows with the
      // same timestamp. Preserve that tie-break while batching the copies.
      next = currentKey;
      currentIndex += 1;
    } else {
      next = incomingEntry.key;
      incomingIndex += 1;
    }
    if (seen.has(next)) continue;
    seen.add(next);
    result.push(next);
  }
  return result;
}

function findCanonicalKeyInMembership(
  state: MutableTimelineEntityState,
  membership: ReadonlySet<StatusKey>,
  canonical: StatusKey,
) {
  const aliases = state.canonicalIndex.get(canonical);
  if (!aliases) return undefined;
  for (const key of canonicalAliasKeys(aliases)) {
    if (membership.has(key)) return key;
  }
  return undefined;
}

function normalizeOptionalTimelineLimit(limit: number | undefined) {
  return limit === undefined ? undefined : normalizeTimelineLimit(limit);
}

function normalizeStatusList(
  state: MutableTimelineEntityState,
  statuses: TimelineStatus[],
  limit?: number,
) {
  const result: StatusKey[] = [];
  const seen = new Set<StatusKey>();
  const normalizedLimit = limit === undefined
    ? undefined
    : normalizeTimelineLimit(limit);
  for (const status of statuses) {
    const key = upsertStatus(state, status);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(key);
    if (normalizedLimit !== undefined && result.length >= normalizedLimit) break;
  }
  return result;
}

function upsertStatus(
  state: MutableTimelineEntityState,
  incoming: TimelineStatus,
): StatusKey {
  return upsertStatusIdentity(state, incoming).key;
}

function upsertStatusIdentity(
  state: MutableTimelineEntityState,
  incoming: TimelineStatus,
) {
  let quote = incoming.quote ?? null;
  if (quote) {
    const quoteKey = upsertStatus(state, quote);
    quote = state.entities.get(quoteKey) ?? quote;
  }
  const normalized = quote === incoming.quote ? incoming : { ...incoming, quote };
  const canonical = canonicalStatusKey(normalized);
  const key = statusKeyFromCanonical(normalized, canonical);
  state.entities.set(key, normalized);
  addCanonicalAlias(state.canonicalIndex, canonical, key);
  if (state.serverIdIndex) {
    addServerIdAlias(
      state.serverIdIndex,
      normalized.serverDomain,
      normalized.id,
      key,
    );
    addServerIdAlias(
      state.serverIdIndex,
      normalized.serverDomain,
      normalized.originalStatusId,
      key,
    );
  }
  return { key, canonical };
}

function replaceCanonicalAliases(
  state: MutableTimelineEntityState,
  canonical: StatusKey,
  updated: TimelineStatus,
) {
  const aliases = state.canonicalIndex.get(canonical);
  if (!aliases) {
    upsertStatus(state, updated);
    return;
  }
  for (const key of canonicalAliasKeys(aliases)) {
    const current = state.entities.get(key);
    if (!current) continue;
    state.entities.set(key, mergeUpdatedStatusIntoEntry(current, updated));
  }
}

function patchCanonicalAliases(
  state: MutableTimelineEntityState,
  canonical: StatusKey,
  patch: Partial<TimelineStatus>,
) {
  const aliases = state.canonicalIndex.get(canonical);
  if (!aliases) return;
  for (const key of canonicalAliasKeys(aliases)) {
    const current = state.entities.get(key);
    if (current) state.entities.set(key, { ...current, ...patch });
  }
}

function removeCanonicalAliases(
  state: MutableTimelineEntityState,
  canonical: StatusKey,
) {
  const aliases = state.canonicalIndex.get(canonical);
  if (!aliases) return;
  const removed = new Set(canonicalAliasKeys(aliases));
  for (const key of removed) state.entities.delete(key);
  state.canonicalIndex.delete(canonical);
  for (const [columnId, keys] of Object.entries(state.columnKeys)) {
    const filtered = keys.filter((key) => !removed.has(key));
    if (filtered.length !== keys.length) setColumnKeys(state, columnId, filtered);
  }
  for (const [columnId, keys] of Object.entries(state.deferredColumnKeys)) {
    const filtered = keys.filter((key) => !removed.has(key));
    if (filtered.length !== keys.length) {
      setDeferredColumnKeys(state, columnId, filtered);
    }
  }
}

function removeCanonicalById(
  state: MutableTimelineEntityState,
  serverDomain: string,
  statusId: string,
) {
  const canonicals = new Set<StatusKey>();
  const serverIdIndex =
    state.serverIdIndex ??
    (state.serverIdIndex = buildServerIdIndex(state.entities));
  const aliases = serverIdIndex.get(
    serverStatusIdIndexKey(serverDomain, statusId),
  );
  for (const key of aliases ? canonicalAliasKeys(aliases) : []) {
    const entity = state.entities.get(key);
    if (entity) canonicals.add(canonicalStatusKey(entity));
  }
  for (const canonical of canonicals) removeCanonicalAliases(state, canonical);
}

function relinkNestedStatuses(state: MutableTimelineEntityState) {
  for (const [key, entity] of state.entities) {
    if (!entity.quote) continue;
    const quoteAliases = state.canonicalIndex.get(
      canonicalStatusKey(entity.quote),
    );
    if (!quoteAliases) continue;
    const quoteKey = firstCanonicalAlias(quoteAliases);
    const quote = quoteKey ? state.entities.get(quoteKey) : undefined;
    if (quote && quote !== entity.quote) {
      state.entities.set(key, { ...entity, quote });
    }
  }
}

function pruneUnreferencedEntities(state: MutableTimelineEntityState) {
  const retained = new Set<StatusKey>();
  const visit = (key: StatusKey) => {
    if (retained.has(key)) return;
    const entity = state.entities.get(key);
    if (!entity) return;
    retained.add(key);
    if (entity.quote) visit(statusKey(entity.quote));
  };
  for (const keys of Object.values(state.columnKeys)) {
    for (const key of keys) visit(key);
  }
  for (const keys of Object.values(state.deferredColumnKeys)) {
    for (const key of keys) visit(key);
  }
  for (const [key, entity] of state.entities) {
    if (retained.has(key)) continue;
    state.entities.delete(key);
    removeCanonicalAlias(
      state.canonicalIndex,
      canonicalStatusKey(entity),
      key,
    );
  }
}

function materializeTimelines(
  entities: Map<StatusKey, TimelineStatus>,
  columnKeys: Record<string, StatusKey[]>,
) {
  return Object.fromEntries(
    Object.entries(columnKeys).map(([columnId, keys]) => {
      const statuses = new Array<TimelineStatus>(keys.length);
      let statusIndex = 0;
      for (const key of keys) {
        const entity = entities.get(key);
        if (entity) {
          statuses[statusIndex] = entity;
          statusIndex += 1;
        }
      }
      statuses.length = statusIndex;
      return [columnId, statuses];
    }),
  );
}

function mergeOrderedKeys(
  entities: Map<StatusKey, TimelineStatus>,
  incoming: StatusKey[],
  current: StatusKey[],
  limit: number,
) {
  const result: StatusKey[] = [];
  const seen = new Set<StatusKey>();
  let incomingIndex = 0;
  let currentIndex = 0;
  const normalizedLimit = normalizeTimelineLimit(limit);
  while (
    result.length < normalizedLimit &&
    (incomingIndex < incoming.length || currentIndex < current.length)
  ) {
    const incomingKey = incoming[incomingIndex];
    const currentKey = current[currentIndex];
    let next: StatusKey;
    if (incomingKey === undefined) {
      next = currentKey;
      currentIndex += 1;
    } else if (currentKey === undefined) {
      next = incomingKey;
      incomingIndex += 1;
    } else if (
      createdAtTimestamp(entities.get(incomingKey)) >=
      createdAtTimestamp(entities.get(currentKey))
    ) {
      next = incomingKey;
      incomingIndex += 1;
    } else {
      next = currentKey;
      currentIndex += 1;
    }
    if (seen.has(next)) continue;
    seen.add(next);
    result.push(next);
  }
  return result;
}

function mergeUpdatedStatusIntoEntry(
  current: TimelineStatus,
  updated: TimelineStatus,
) {
  const updatedWithSource = {
    ...updated,
    sourceAcct: updated.sourceAcct ?? current.sourceAcct,
  };
  return {
    ...updatedWithSource,
    // The timeline event identity belongs to the current entry. An update may
    // arrive as a notification/reblog wrapper and must never leak that
    // wrapper's identity or metadata into the plain entity (and vice versa).
    id: current.id,
    uri: current.uri,
    originalStatusId: current.originalStatusId,
    createdAt: current.createdAt,
    originalCreatedAt:
      current.originalCreatedAt ?? updatedWithSource.originalCreatedAt,
    sourceAcct: current.sourceAcct ?? updated.sourceAcct,
    notificationId: current.notificationId,
    notificationKind: current.notificationKind,
    notificationLabel: current.notificationLabel,
    notificationAvatar: current.notificationAvatar,
    notificationAccountId: current.notificationAccountId,
    notificationAcct: current.notificationAcct,
    notificationDisplayName: current.notificationDisplayName,
    notificationAccountEmojis: current.notificationAccountEmojis,
  };
}

function createdAtTimestamp(status: TimelineStatus | undefined) {
  if (!status) return Number.NEGATIVE_INFINITY;
  const cached = createdAtTimestampCache.get(status);
  if (cached !== undefined) return cached;
  const parsed = Date.parse(status.createdAt);
  const timestamp = Number.isFinite(parsed) ? parsed : Number.NEGATIVE_INFINITY;
  createdAtTimestampCache.set(status, timestamp);
  return timestamp;
}

function addCanonicalAlias(
  index: CanonicalIndex,
  canonical: StatusKey,
  key: StatusKey,
) {
  const aliases = index.get(canonical);
  if (!aliases) {
    index.set(canonical, key);
    return;
  }
  if (!canonicalAliasesHas(aliases, key)) {
    // canonicalIndex starts as a shallow copy. Copy only the alias sets that
    // actually change so the previous immutable state is never mutated.
    index.set(canonical, new Set([...canonicalAliasKeys(aliases), key]));
  }
}

function removeCanonicalAlias(
  index: CanonicalIndex,
  canonical: StatusKey,
  key: StatusKey,
) {
  const aliases = index.get(canonical);
  if (!aliases || !canonicalAliasesHas(aliases, key)) return;
  if (typeof aliases === "string") {
    index.delete(canonical);
    return;
  }
  const retained = new Set(aliases);
  retained.delete(key);
  if (retained.size === 1) {
    index.set(canonical, retained.values().next().value as StatusKey);
  } else {
    index.set(canonical, retained);
  }
}

function canonicalAliasKeys(aliases: CanonicalAliases): Iterable<StatusKey> {
  return typeof aliases === "string" ? [aliases] : aliases;
}

function canonicalAliasesHas(aliases: CanonicalAliases, key: StatusKey) {
  return typeof aliases === "string" ? aliases === key : aliases.has(key);
}

function firstCanonicalAlias(aliases: CanonicalAliases) {
  return typeof aliases === "string"
    ? aliases
    : (aliases.values().next().value as StatusKey | undefined);
}

function buildServerIdIndex(entities: Map<StatusKey, TimelineStatus>) {
  const index = new Map<string, Set<StatusKey>>();
  for (const [key, status] of entities) {
    addServerIdAlias(index, status.serverDomain, status.id, key);
    addServerIdAlias(index, status.serverDomain, status.originalStatusId, key);
  }
  return index;
}

function addServerIdAlias(
  index: Map<string, Set<StatusKey>>,
  serverDomain: string,
  statusId: string,
  key: StatusKey,
) {
  const indexKey = serverStatusIdIndexKey(serverDomain, statusId);
  const aliases = index.get(indexKey);
  if (aliases) aliases.add(key);
  else index.set(indexKey, new Set([key]));
}

function serverStatusIdIndexKey(serverDomain: string, statusId: string) {
  return `${serverDomain.trim().toLowerCase()}\u0000${statusId}`;
}
