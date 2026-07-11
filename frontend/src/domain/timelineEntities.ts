import type { TimelineStatus } from "../types/app";

export type StatusKey = string;

export const TIMELINE_HARD_MAX_STATUSES = 1_000;

export type TimelineEntityState = {
  entities: Map<StatusKey, TimelineStatus>;
  columnKeys: Record<string, StatusKey[]>;
  canonicalIndex: Map<StatusKey, Set<StatusKey>>;
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
      limit: number;
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
      limits: Record<string, number>;
      updateOnly?: boolean;
      preserveAnchorColumns?: ReadonlySet<string>;
    }
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
    canonicalIndex: new Map(),
    timelines: {},
  };
}

export function clampTimelineLimit(limit: number) {
  const finite = Number.isFinite(limit) ? Math.floor(limit) : 100;
  return Math.min(TIMELINE_HARD_MAX_STATUSES, Math.max(1, finite || 100));
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
  if (status.notificationId) {
    return `${canonical}:notification:${status.notificationId}`;
  }
  if (status.id && status.originalStatusId && status.id !== status.originalStatusId) {
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
    canonicalIndex: cloneSetMap(previous.canonicalIndex),
    serverIdIndex: buildServerIdIndex(previous.entities),
  };

  for (const operation of operations) {
    switch (operation.type) {
      case "replaceColumn": {
        mutable.columnKeys[operation.columnId] = normalizeStatusList(
          mutable,
          operation.statuses,
          operation.limit,
        );
        break;
      }
      case "appendPage": {
        const current = mutable.columnKeys[operation.columnId] ?? [];
        const appended = normalizeStatusList(
          mutable,
          operation.statuses,
          operation.limit,
        );
        mutable.columnKeys[operation.columnId] = dedupeKeys(
          [...current, ...appended],
          operation.limit,
        );
        break;
      }
      case "mergeDelta": {
        const incoming = normalizeStatusList(
          mutable,
          operation.statuses,
          operation.limit,
        );
        const current = mutable.columnKeys[operation.columnId] ?? [];
        mutable.columnKeys[operation.columnId] = mergeOrderedKeys(
          mutable.entities,
          incoming,
          current,
          operation.limit,
        );
        break;
      }
      case "upsertInColumns": {
        const key = upsertStatus(mutable, operation.status);
        const canonical = canonicalStatusKey(operation.status);
        replaceCanonicalAliases(mutable, canonical, operation.status);
        for (const columnId of operation.columnIds) {
          const current = mutable.columnKeys[columnId] ?? [];
          const existingKey = findCanonicalKeyInColumn(
            mutable,
            current,
            canonical,
          );
          if (existingKey) {
            // Keep the wrapper/event key and its position; only its entity was
            // replaced above. This keeps scroll anchors and notification data.
            continue;
          }
          if (operation.updateOnly || operation.preserveAnchorColumns?.has(columnId)) {
            continue;
          }
          mutable.columnKeys[columnId] = insertKeyByCreatedAt(
            mutable.entities,
            current,
            key,
            operation.limits[columnId] ?? TIMELINE_HARD_MAX_STATUSES,
          );
        }
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
          if (!current) continue;
          mutable.columnKeys[columnId] = current.filter(
            (key) => !aliases.has(key),
          );
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
  for (const key of aliases) {
    const status = state.entities.get(key);
    if (status) return status;
  }
  return undefined;
}

type MutableTimelineEntityState = Pick<
  TimelineEntityState,
  "entities" | "columnKeys" | "canonicalIndex"
> & {
  serverIdIndex: Map<string, Set<StatusKey>>;
};

function normalizeStatusList(
  state: MutableTimelineEntityState,
  statuses: TimelineStatus[],
  limit: number,
) {
  const result: StatusKey[] = [];
  const seen = new Set<StatusKey>();
  const cappedLimit = clampTimelineLimit(limit);
  for (const status of statuses) {
    const key = upsertStatus(state, status);
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(key);
    if (result.length >= cappedLimit) break;
  }
  return result;
}

function upsertStatus(
  state: MutableTimelineEntityState,
  incoming: TimelineStatus,
): StatusKey {
  let quote = incoming.quote ?? null;
  if (quote) {
    const quoteKey = upsertStatus(state, quote);
    quote = state.entities.get(quoteKey) ?? quote;
  }
  const normalized = quote === incoming.quote ? incoming : { ...incoming, quote };
  const key = statusKey(normalized);
  state.entities.set(key, normalized);
  addCanonicalAlias(state.canonicalIndex, canonicalStatusKey(normalized), key);
  addServerIdAlias(state.serverIdIndex, normalized.serverDomain, normalized.id, key);
  addServerIdAlias(
    state.serverIdIndex,
    normalized.serverDomain,
    normalized.originalStatusId,
    key,
  );
  return key;
}

function replaceCanonicalAliases(
  state: MutableTimelineEntityState,
  canonical: StatusKey,
  updated: TimelineStatus,
) {
  const aliases = state.canonicalIndex.get(canonical);
  if (!aliases || aliases.size === 0) {
    upsertStatus(state, updated);
    return;
  }
  for (const key of aliases) {
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
  for (const key of aliases) {
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
  const removed = new Set(aliases);
  for (const key of removed) state.entities.delete(key);
  state.canonicalIndex.delete(canonical);
  for (const [columnId, keys] of Object.entries(state.columnKeys)) {
    const filtered = keys.filter((key) => !removed.has(key));
    if (filtered.length !== keys.length) state.columnKeys[columnId] = filtered;
  }
}

function removeCanonicalById(
  state: MutableTimelineEntityState,
  serverDomain: string,
  statusId: string,
) {
  const canonicals = new Set<StatusKey>();
  const aliases = state.serverIdIndex.get(
    serverStatusIdIndexKey(serverDomain, statusId),
  );
  for (const key of aliases ?? []) {
    const entity = state.entities.get(key);
    if (entity) canonicals.add(canonicalStatusKey(entity));
  }
  for (const canonical of canonicals) removeCanonicalAliases(state, canonical);
}

function relinkNestedStatuses(state: MutableTimelineEntityState) {
  for (const [key, entity] of state.entities) {
    if (!entity.quote) continue;
    const quoteAliases = state.canonicalIndex.get(canonicalStatusKey(entity.quote));
    if (!quoteAliases) continue;
    const quoteKey = quoteAliases.values().next().value as StatusKey | undefined;
    const quote = quoteKey ? state.entities.get(quoteKey) : undefined;
    if (quote && quote !== entity.quote) state.entities.set(key, { ...entity, quote });
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
  for (const key of state.entities.keys()) {
    if (!retained.has(key)) state.entities.delete(key);
  }
  state.canonicalIndex = buildCanonicalIndex(state.entities);
}

function materializeTimelines(
  entities: Map<StatusKey, TimelineStatus>,
  columnKeys: Record<string, StatusKey[]>,
) {
  return Object.fromEntries(
    Object.entries(columnKeys).map(([columnId, keys]) => [
      columnId,
      keys.flatMap((key) => {
        const entity = entities.get(key);
        return entity ? [entity] : [];
      }),
    ]),
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
  const cappedLimit = clampTimelineLimit(limit);
  while (
    result.length < cappedLimit &&
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

function insertKeyByCreatedAt(
  entities: Map<StatusKey, TimelineStatus>,
  current: StatusKey[],
  key: StatusKey,
  limit: number,
) {
  if (current.includes(key)) return current;
  const timestamp = createdAtTimestamp(entities.get(key));
  let low = 0;
  let high = current.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (createdAtTimestamp(entities.get(current[middle])) >= timestamp) {
      low = middle + 1;
    } else {
      high = middle;
    }
  }
  const result = [...current.slice(0, low), key, ...current.slice(low)];
  return result.slice(0, clampTimelineLimit(limit));
}

function findCanonicalKeyInColumn(
  state: MutableTimelineEntityState,
  keys: StatusKey[],
  canonical: StatusKey,
) {
  const aliases = state.canonicalIndex.get(canonical);
  if (!aliases) return undefined;
  for (const key of keys) {
    if (aliases.has(key)) return key;
  }
  return undefined;
}

function dedupeKeys(keys: StatusKey[], limit: number) {
  const result: StatusKey[] = [];
  const seen = new Set<StatusKey>();
  for (const key of keys) {
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(key);
    if (result.length >= clampTimelineLimit(limit)) break;
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
  const parsed = Date.parse(status.createdAt);
  return Number.isFinite(parsed) ? parsed : Number.NEGATIVE_INFINITY;
}

function addCanonicalAlias(
  index: Map<StatusKey, Set<StatusKey>>,
  canonical: StatusKey,
  key: StatusKey,
) {
  const aliases = index.get(canonical);
  if (aliases) aliases.add(key);
  else index.set(canonical, new Set([key]));
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

function buildCanonicalIndex(entities: Map<StatusKey, TimelineStatus>) {
  const index = new Map<StatusKey, Set<StatusKey>>();
  for (const [key, status] of entities) {
    addCanonicalAlias(index, canonicalStatusKey(status), key);
  }
  return index;
}

function cloneSetMap(source: Map<StatusKey, Set<StatusKey>>) {
  return new Map(
    [...source].map(([key, values]) => [key, new Set(values)] as const),
  );
}
