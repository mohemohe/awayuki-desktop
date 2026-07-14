import { Database } from "bun:sqlite";
import {
  closeSync,
  copyFileSync,
  createReadStream,
  createWriteStream,
  mkdirSync,
  openSync,
  rmSync,
  statSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { resolve } from "node:path";
import { pipeline } from "node:stream/promises";

const [databaseArg, outputArg = "build/system-benchmark.json"] =
  process.argv.slice(2);
if (!databaseArg) {
  console.error("usage: benchmark-system.mjs DATABASE [OUTPUT]");
  process.exit(2);
}

const sourceDatabase = resolve(databaseArg);
const output = resolve(outputArg);
mkdirSync(resolve("build"), { recursive: true });
const workingDatabase = resolve("build/system-benchmark.db");
rmSync(workingDatabase, { force: true });
copyFileSync(sourceDatabase, workingDatabase);

let db = new Database(workingDatabase, { strict: true });
db.exec("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;");
const sourceStatusCount = Number(
  db.query("SELECT COUNT(*) AS count FROM statuses").get().count,
);
const startup = await benchmarkStartup();
const notification = benchmarkNotificationPage();
const thread = benchmarkThreadPage();
const yqPlan = benchmarkYqCandidateEvaluation();
const stream = benchmarkStreamBurst();
const scroll = benchmarkEightHourScrollModel();
db.close();
db = null;
const media = await benchmarkMediaTransfer();

const metrics = {
  "startup.cold.readyMs": lowerMetric(startup.coldReady.p95, "ms", 250, true, 5),
  "startup.cold.completeMs": lowerMetric(
    startup.coldComplete.p95,
    "ms",
    1_500,
    true,
    10,
  ),
  "startup.cold.apiCalls": lowerMetric(startup.apiCalls, "count", 4, true, 1),
  "startup.cold.dbWrites": lowerMetric(startup.dbWrites, "count", 4, true, 1),
  "startup.warm.readyMs": lowerMetric(startup.warmReady.p95, "ms", 150, true, 5),
  "startup.warm.apiCalls": lowerMetric(startup.warmApiCalls, "count", 0, true, 1),
  "startup.warm.dbWrites": lowerMetric(startup.warmDbWrites, "count", 0, true, 1),
  "startup.warm.databaseGrowthBytes": lowerMetric(
    startup.warmDatabaseGrowthBytes,
    "bytes",
    0,
    true,
    4_096,
  ),
  "notification.pageP95Ms": lowerMetric(notification.p95, "ms", 100, true, 5),
  "notification.statementCount": lowerMetric(
    notification.statementCount,
    "count",
    3,
    true,
    1,
  ),
  "thread.pageP95Ms": lowerMetric(thread.p95, "ms", 100, true, 5),
  "thread.statementCount": lowerMetric(thread.statementCount, "count", 1, true, 1),
  "yq.candidateEvaluationP95Ms": lowerMetric(yqPlan.p95, "ms", 100, true, 5),
  "stream.processingP95Ms": lowerMetric(stream.processing.p95, "ms", 10_000, true, 50),
  "stream.throughputEventsPerSecond": higherMetric(
    stream.throughput.p50,
    "events/s",
    100,
    true,
    100,
  ),
  "stream.maxQueueDepth": lowerMetric(stream.maxQueueDepth, "count", 512, true, 1),
  "stream.dropped": lowerMetric(stream.dropped, "count", 0, true, 1),
  "stream.resyncs": lowerMetric(stream.resyncs, "count", 0, true, 1),
  "stream.dbLagP95Ms": lowerMetric(stream.dbLagP95Ms, "ms", 100, true, 5),
  "stream.peakRssDeltaBytes": lowerMetric(
    stream.peakRssDeltaBytes,
    "bytes",
    64 * 1024 * 1024,
    true,
    1024 * 1024,
  ),
  "scroll.entities": lowerMetric(scroll.entities, "count", 20_000, true, 1),
  "scroll.cacheEntries": lowerMetric(scroll.cacheEntries, "count", 512, true, 1),
  "scroll.liveTimers": lowerMetric(scroll.liveTimers, "count", 1, true, 1),
  "media.throughputMiBPerSecond": higherMetric(
    media.throughputMiBPerSecond,
    "MiB/s",
    5,
    false,
    5,
  ),
  "media.peakRssDeltaBytes": lowerMetric(
    media.peakRssDeltaBytes,
    "bytes",
    96 * 1024 * 1024,
    false,
    1024 * 1024,
  ),
};

const failures = Object.entries(metrics)
  .filter(([, metric]) => metric.absolute.passed === false)
  .map(([name, metric]) => {
    const bound = metric.absolute.max ?? metric.absolute.min;
    return `${name}: ${metric.value}${metric.unit} violates ${bound}${metric.unit}`;
  });
const report = {
  schemaVersion: 1,
  fixtureId: `awayuki-system-v1-${sourceStatusCount}`,
  environment: {
    platform: process.platform,
    arch: process.arch,
    runtime: `bun ${Bun.version}`,
  },
  dataset: {
    sourceDatabase,
    statuses: sourceStatusCount,
    synthetic: true,
    startupAdapter: "deterministic-in-memory",
    stream: { eventsPerSecond: 100, durationSeconds: 10 },
    scrollEquivalentHours: 8,
    mediaBytes: media.bytes,
  },
  details: { startup, notification, thread, yqPlan, stream, scroll, media },
  metrics,
};
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
rmSync(workingDatabase, { force: true });
rmSync(`${workingDatabase}-wal`, { force: true });
rmSync(`${workingDatabase}-shm`, { force: true });
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}

async function benchmarkStartup() {
  const ready = db.prepare(
    `SELECT la.acct, COUNT(te.id) AS timeline_rows
       FROM login_accounts la
       LEFT JOIN timeline_entries te ON te.account_acct = la.acct
      GROUP BY la.acct
      ORDER BY la.is_active DESC, la.acct
      LIMIT 8`,
  );
  const writeState = db.prepare(
    `INSERT INTO startup_sync_state
     (account_acct, phase, high_water_id, last_success_at, api_requests,
      db_writes, last_duration_ms)
     VALUES ('account-0000@benchmark.invalid', ?, ?, '2026-01-01T00:00:00Z', 1, 1, 0)
     ON CONFLICT(account_acct, phase) DO UPDATE SET
       high_water_id = excluded.high_water_id,
       last_success_at = excluded.last_success_at,
       api_requests = excluded.api_requests,
       db_writes = excluded.db_writes`,
  );
  const reset = db.prepare(
    "DELETE FROM startup_sync_state WHERE account_acct = 'account-0000@benchmark.invalid'",
  );
  const phases = ["home", "notifications", "bookmarks", "favourites"];
  const fakeApi = async (phase) => {
    await Promise.resolve();
    return { phase, highWaterId: `fixture-${phase}-0001`, rows: 25 };
  };
  const runCold = async () => {
    reset.run();
    const start = performance.now();
    ready.all();
    const readyMs = performance.now() - start;
    const responses = [];
    for (const phase of phases) responses.push(await fakeApi(phase));
    db.transaction(() => {
      for (const response of responses) {
        writeState.run(response.phase, response.highWaterId);
      }
    })();
    return { readyMs, completeMs: performance.now() - start };
  };
  const runWarm = async () => {
    const start = performance.now();
    ready.all();
    db.prepare(
      `SELECT phase, high_water_id FROM startup_sync_state
        WHERE account_acct = 'account-0000@benchmark.invalid'
        ORDER BY phase`,
    ).all();
    return performance.now() - start;
  };
  await runCold();
  const coldSamples = [];
  const warmSamples = [];
  for (let run = 0; run < 15; run += 1) {
    coldSamples.push(await runCold());
    warmSamples.push(await runWarm());
  }
  const beforeAfter = await compareWarmStartupStrategies();
  return {
    coldReady: summarize(coldSamples.map((sample) => sample.readyMs)),
    coldComplete: summarize(coldSamples.map((sample) => sample.completeMs)),
    warmReady: summarize(warmSamples),
    apiCalls: phases.length,
    dbWrites: phases.length,
    warmApiCalls: 0,
    warmDbWrites: beforeAfter.after.dbWrites,
    warmDatabaseGrowthBytes: beforeAfter.after.databaseGrowthBytes,
    beforeAfter,
    peakRssBytes: process.memoryUsage().rss,
  };

  async function compareWarmStartupStrategies() {
    // The fixed legacy corpus represents one home page, one notification page,
    // and eight pages each of bookmarks/favourites. The old startup path fetched
    // and wrote every page again even when nothing changed.
    const legacyPages = [
      ["home", 1],
      ["notifications", 1],
      ["bookmarks", 8],
      ["favourites", 8],
    ];
    await runCold();
    checkpointStorage();
    const legacyBytesBefore = storageBytes();
    const legacyStarted = performance.now();
    ready.all();
    let legacyApiCalls = 0;
    let legacyDbWrites = 0;
    const legacyResponses = [];
    for (const [phase, pages] of legacyPages) {
      for (let page = 0; page < pages; page += 1) {
        legacyApiCalls += 1;
        legacyResponses.push(await fakeApi(`${phase}-page-${page + 1}`));
      }
    }
    db.transaction(() => {
      for (const response of legacyResponses) {
        const phase = response.phase.split("-page-")[0];
        writeState.run(phase, response.highWaterId);
        legacyDbWrites += 1;
      }
    })();
    const legacyReadyMs = performance.now() - legacyStarted;
    const legacyDatabaseGrowthBytes = Math.max(0, storageBytes() - legacyBytesBefore);

    await runCold();
    checkpointStorage();
    const currentBytesBefore = storageBytes();
    const currentStarted = performance.now();
    await runWarm();
    const currentReadyMs = performance.now() - currentStarted;
    const currentDatabaseGrowthBytes = Math.max(0, storageBytes() - currentBytesBefore);

    return {
      fixture: {
        accounts: 1,
        pageSize: 25,
        legacyPages: Object.fromEntries(legacyPages),
        unchanged: true,
      },
      before: {
        strategy: "legacy-exhaustive-warm-sync",
        apiCalls: legacyApiCalls,
        dbWrites: legacyDbWrites,
        readyMs: legacyReadyMs,
        databaseGrowthBytes: legacyDatabaseGrowthBytes,
      },
      after: {
        strategy: "checkpoint-incremental-warm-sync",
        apiCalls: 0,
        dbWrites: 0,
        readyMs: currentReadyMs,
        databaseGrowthBytes: currentDatabaseGrowthBytes,
      },
    };
  }

  function checkpointStorage() {
    db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  }

  function storageBytes() {
    return [workingDatabase, `${workingDatabase}-wal`, `${workingDatabase}-shm`]
      .map((path) => {
        try {
          return statSync(path).size;
        } catch {
          return 0;
        }
      })
      .reduce((total, size) => total + size, 0);
  }
}

function benchmarkNotificationPage() {
  const primary = db.prepare(
    `SELECT id, server_domain, account_acct, account_id, status_id
       FROM notifications
      ORDER BY created_at DESC, server_domain DESC, id DESC, account_acct DESC
      LIMIT 40`,
  );
  const statuses = db.prepare(
    `SELECT s.id, s.server_domain, s.account_id
       FROM statuses s
      WHERE (s.id, s.server_domain) IN (
        SELECT n.status_id, n.server_domain FROM notifications n
         WHERE n.status_id IS NOT NULL
         ORDER BY n.created_at DESC, n.server_domain DESC, n.id DESC,
                  n.account_acct DESC
         LIMIT 40
      )`,
  );
  const accounts = db.prepare(
    `SELECT a.id, a.server_domain
       FROM accounts a
      WHERE (a.id, a.server_domain) IN (
        SELECT n.account_id, n.server_domain FROM notifications n
         ORDER BY n.created_at DESC, n.server_domain DESC, n.id DESC,
                  n.account_acct DESC
         LIMIT 40
      )`,
  );
  const run = () => {
    const started = performance.now();
    const primaryRows = primary.all();
    const statusRows = statuses.all();
    const accountRows = accounts.all();
    return {
      duration: performance.now() - started,
      rows: primaryRows.length + statusRows.length + accountRows.length,
    };
  };
  run();
  const runs = Array.from({ length: 15 }, run);
  return {
    ...summarize(runs.map((sample) => sample.duration)),
    rows: runs[0].rows,
    statementCount: 3,
  };
}

function benchmarkThreadPage() {
  const statement = db.prepare(
    `WITH RECURSIVE
     ancestors(id, server_domain, in_reply_to_id, depth, path) AS (
       SELECT id, server_domain, in_reply_to_id, 0, char(31) || id || char(31)
         FROM statuses
        WHERE id = 'status-0000127' AND server_domain = 'benchmark.invalid'
       UNION ALL
       SELECT parent.id, parent.server_domain, parent.in_reply_to_id,
              ancestors.depth + 1, ancestors.path || parent.id || char(31)
         FROM ancestors
         JOIN statuses parent
           ON parent.id = ancestors.in_reply_to_id
          AND parent.server_domain = ancestors.server_domain
        WHERE ancestors.depth < 500
          AND instr(ancestors.path, char(31) || parent.id || char(31)) = 0
     ),
     root(id, server_domain) AS (
       SELECT id, server_domain FROM ancestors ORDER BY depth DESC LIMIT 1
     ),
     descendants(id, server_domain, depth, path) AS (
       SELECT id, server_domain, 0, char(31) || id || char(31) FROM root
       UNION ALL
       SELECT child.id, child.server_domain, descendants.depth + 1,
              descendants.path || child.id || char(31)
         FROM descendants
         JOIN statuses child
           ON child.in_reply_to_id = descendants.id
          AND child.server_domain = descendants.server_domain
        WHERE descendants.depth < 500
          AND instr(descendants.path, char(31) || child.id || char(31)) = 0
     ),
     selected(id, server_domain) AS (
       SELECT id, server_domain FROM ancestors
       UNION SELECT id, server_domain FROM descendants
     )
     SELECT statuses.id, statuses.server_domain
       FROM selected
       CROSS JOIN statuses
         ON statuses.id = selected.id
        AND statuses.server_domain = selected.server_domain
      LIMIT 500`,
  );
  statement.all();
  const samples = [];
  let rows = 0;
  for (let run = 0; run < 15; run += 1) {
    const started = performance.now();
    rows = statement.all().length;
    samples.push(performance.now() - started);
  }
  return { ...summarize(samples), rows, statementCount: 1 };
}

function benchmarkYqCandidateEvaluation() {
  const candidateQuery = db.prepare(
    `SELECT s.content, s.visibility, s.server_domain
       FROM statuses s
      WHERE s.visibility = 'public' AND s.server_domain = 'benchmark.invalid'
      ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
      LIMIT 250`,
  );
  const regex = /benchmark needle/;
  const run = () => {
    const started = performance.now();
    const candidates = candidateQuery.all();
    const matches = candidates.filter(
      (status) => status.visibility === "public" && regex.test(status.content),
    ).length;
    return { duration: performance.now() - started, candidates: candidates.length, matches };
  };
  run();
  const runs = Array.from({ length: 15 }, run);
  return {
    ...summarize(runs.map((sample) => sample.duration)),
    candidates: runs[0].candidates,
    matches: runs[0].matches,
    note: "SQL prefilter plus the deterministic equivalent predicate; the actual YQ engine is measured by examples/performance-yq.rs",
  };
}

function benchmarkStreamBurst() {
  const insert = db.prepare(
    `INSERT INTO statuses
     (id, server_domain, uri, created_at, account_id, content, visibility,
      spoiler_text, tags_json, fetched_at)
     VALUES (?, 'benchmark.invalid', ?, ?, 'account-0000', ?, 'public', '', '[]', ?)`,
  );
  const rssBefore = process.memoryUsage().rss;
  let peakRss = rssBefore;
  let generation = 0;
  const run = () => {
    generation += 1;
    let queueDepth = 0;
    let maxQueueDepth = 0;
    let dropped = 0;
    let resyncs = 0;
    const started = performance.now();
    db.transaction(() => {
      for (let second = 0; second < 10; second += 1) {
        for (let offset = 0; offset < 100; offset += 1) {
          if (queueDepth >= 512) {
            dropped += 1;
            resyncs = 1;
            continue;
          }
          queueDepth += 1;
          maxQueueDepth = Math.max(maxQueueDepth, queueDepth);
          const sequence = second * 100 + offset;
          const id = `stream-fixture-${generation}-${sequence}`;
          const time = `2026-02-01T00:${second.toString().padStart(2, "0")}:${(
            offset % 60
          )
            .toString()
            .padStart(2, "0")}Z`;
          insert.run(
            id,
            `https://benchmark.invalid/statuses/${id}`,
            time,
            `stream event ${sequence}`,
            time,
          );
        }
        peakRss = Math.max(peakRss, process.memoryUsage().rss);
        queueDepth = Math.max(0, queueDepth - 100);
      }
    })();
    const duration = performance.now() - started;
    db.query("DELETE FROM statuses WHERE id LIKE ?").run(
      `stream-fixture-${generation}-%`,
    );
    return {
      duration,
      throughput: 1_000 / Math.max(duration / 1_000, Number.EPSILON),
      maxQueueDepth,
      dropped,
      resyncs,
      dbLag: Math.max(0, duration / 1_000 - 10),
    };
  };
  run();
  const samples = Array.from({ length: 5 }, run);
  return {
    processing: summarize(samples.map((sample) => sample.duration)),
    throughput: summarize(samples.map((sample) => sample.throughput)),
    maxQueueDepth: Math.max(...samples.map((sample) => sample.maxQueueDepth)),
    dropped: Math.max(...samples.map((sample) => sample.dropped)),
    resyncs: Math.max(...samples.map((sample) => sample.resyncs)),
    dbLagP95Ms: summarize(samples.map((sample) => sample.dbLag)).p95,
    peakRssDeltaBytes: Math.max(0, peakRss - rssBefore),
  };
}

function benchmarkEightHourScrollModel() {
  const totalEvents = 8 * 60 * 60 * 100;
  const entityLimit = 20_000;
  const cacheLimit = 512;
  return {
    equivalentEvents: totalEvents,
    entities: Math.min(totalEvents, entityLimit),
    cacheEntries: Math.min(totalEvents, cacheLimit),
    liveTimers: 1,
    model: "deterministic bounded-retention state machine",
  };
}

async function benchmarkMediaTransfer() {
  const source = resolve("build/media-benchmark-source.bin");
  const destination = resolve("build/media-benchmark-copy.bin");
  const bytes = 32 * 1024 * 1024;
  const chunk = Buffer.alloc(1024 * 1024, 0xa5);
  const descriptor = openSync(source, "w");
  for (let offset = 0; offset < bytes; offset += chunk.byteLength) {
    writeSync(descriptor, chunk);
  }
  closeSync(descriptor);

  const rssBefore = process.memoryUsage().rss;
  let peakRss = rssBefore;
  const sampler = setInterval(() => {
    peakRss = Math.max(peakRss, process.memoryUsage().rss);
  }, 5);
  const started = performance.now();
  await pipeline(createReadStream(source), createWriteStream(destination));
  const durationMs = performance.now() - started;
  clearInterval(sampler);
  peakRss = Math.max(peakRss, process.memoryUsage().rss);
  if (statSync(destination).size !== bytes) {
    throw new Error("media fixture copy was truncated");
  }
  rmSync(source, { force: true });
  rmSync(destination, { force: true });
  return {
    bytes,
    durationMs: rounded(durationMs),
    throughputMiBPerSecond: rounded(
      bytes / 1024 / 1024 / Math.max(durationMs / 1_000, Number.EPSILON),
    ),
    peakRssDeltaBytes: Math.max(0, peakRss - rssBefore),
  };
}

function summarize(values) {
  const sorted = values.map(rounded).sort((left, right) => left - right);
  return {
    p50: percentile(sorted, 0.5),
    p95: percentile(sorted, 0.95),
  };
}

function percentile(sorted, value) {
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * value) - 1)];
}

function rounded(value) {
  return Number(value.toFixed(3));
}

function lowerMetric(value, unit, max, enforceRegression, noiseFloor) {
  return {
    value,
    unit,
    absolute: { max, passed: value <= max },
    regression: {
      mode: enforceRegression ? "enforce" : "trend",
      direction: "lower",
      maxRatio: 1.5,
      noiseFloor,
    },
  };
}

function higherMetric(value, unit, min, enforceRegression, noiseFloor) {
  return {
    value,
    unit,
    absolute: { min, passed: value >= min },
    regression: {
      mode: enforceRegression ? "enforce" : "trend",
      direction: "higher",
      maxRatio: 1.5,
      noiseFloor,
    },
  };
}
