import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

import {
  TIMELINE_HARD_MAX_STATUSES,
  createTimelineEntityState,
  reduceTimelineEntities,
  type TimelineEntityOperation,
} from "../frontend/src/domain/timelineEntities";
import type { TimelineStatus } from "../frontend/src/types/app";

const output = resolve(process.argv[2] ?? "build/timeline-benchmark.json");
const statusCount = 10_000;
const columnCount = 12;
const batchSize = 50;
const batchCount = 20;
const columnIds = Array.from(
  { length: columnCount },
  (_, index) => `column-${index}`,
);
const statuses = Array.from({ length: statusCount }, (_, index) =>
  fixtureStatus(`initial-${index}`, 20_000_000 - index * 1_000),
);

collectGarbage();
const heapBefore = process.memoryUsage().heapUsed;
let state = reduceTimelineEntities(
  createTimelineEntityState(),
  columnIds.map((columnId) => ({
    type: "replaceColumn" as const,
    columnId,
    statuses,
    limit: statusCount,
  })),
);
let peakHeap = process.memoryUsage().heapUsed;
const batchDurations: number[] = [];

for (let batch = 0; batch < batchCount; batch += 1) {
  const operations: TimelineEntityOperation[] = Array.from(
    { length: batchSize },
    (_, offset) => {
      const index = batch * batchSize + offset;
      return {
        type: "upsertInColumns",
        columnIds,
        status: fixtureStatus(`stream-${index}`, 30_000_000 + index * 1_000),
        limits: Object.fromEntries(
          columnIds.map((columnId) => [columnId, statusCount]),
        ),
      };
    },
  );
  const startedAt = performance.now();
  state = reduceTimelineEntities(state, operations);
  batchDurations.push(performance.now() - startedAt);
  peakHeap = Math.max(peakHeap, process.memoryUsage().heapUsed);
}

collectGarbage();
const retainedHeapDeltaBytes = Math.max(
  0,
  process.memoryUsage().heapUsed - heapBefore,
);
const peakHeapDeltaBytes = Math.max(0, peakHeap - heapBefore);
const maxColumnStatuses = Math.max(
  ...Object.values(state.columnKeys).map((keys) => keys.length),
);
const reducerBatchP95Ms = percentile(batchDurations, 0.95);
const metrics = {
  "timeline.entities": lowerMetric(
    state.entities.size,
    "count",
    TIMELINE_HARD_MAX_STATUSES,
  ),
  "timeline.maxColumnStatuses": lowerMetric(
    maxColumnStatuses,
    "count",
    TIMELINE_HARD_MAX_STATUSES,
  ),
  "timeline.peakHeapDeltaBytes": lowerMetric(
    peakHeapDeltaBytes,
    "bytes",
    64 * 1024 * 1024,
  ),
  "timeline.reducerBatchP95Ms": lowerMetric(reducerBatchP95Ms, "ms", 50),
};
const report = {
  schemaVersion: 1,
  fixtureId: "awayuki-timeline-v1-12x10000-1000burst",
  environment: {
    platform: process.platform,
    arch: process.arch,
    runtime: `bun ${Bun.version}`,
  },
  dataset: {
    columns: columnCount,
    statusesPerInputColumn: statusCount,
    streamEvents: batchSize * batchCount,
    batchSize,
    hardMaximum: TIMELINE_HARD_MAX_STATUSES,
  },
  details: {
    retainedHeapDeltaBytes,
    peakHeapDeltaBytes,
    reducerBatchP95Ms,
    reducerBatchDurationsMs: batchDurations,
  },
  metrics,
};

mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));

const failures = Object.entries(metrics).filter(
  ([, metric]) => !metric.absolute.passed,
);
if (failures.length > 0) process.exitCode = 1;

function fixtureStatus(id: string, createdAt: number): TimelineStatus {
  return {
    id,
    originalStatusId: id,
    sourceAcct: "alice@alpha.example",
    accountId: "alice",
    serverDomain: "alpha.example",
    uri: `https://alpha.example/statuses/${id}`,
    url: `https://alpha.example/statuses/${id}`,
    displayName: "Alice",
    acct: "alice@alpha.example",
    avatar: "",
    createdAt: new Date(createdAt).toISOString(),
    content: `<p>${id}</p>`,
    spoilerText: "",
    reblogsCount: 0,
    favouritesCount: 0,
    repliesCount: 0,
    visibility: "public",
    sensitive: false,
    favourited: false,
    reblogged: false,
    bookmarked: false,
    media: [],
    emojis: [],
    accountEmojis: [],
    tags: [],
  } as TimelineStatus;
}

function collectGarbage() {
  Bun.gc(true);
}

function percentile(values: number[], ratio: number) {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.max(0, Math.ceil(sorted.length * ratio) - 1)] ?? 0;
}

function lowerMetric(value: number, unit: string, max: number) {
  return {
    unit,
    value: Number(value.toFixed(3)),
    absolute: { max, passed: value <= max },
  };
}
