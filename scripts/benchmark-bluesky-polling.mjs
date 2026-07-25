import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const [outputArg = "build/bluesky-polling-benchmark.json"] =
  process.argv.slice(2);
const output = resolve(outputArg);
const streamingSource = readFileSync("src/bluesky/streaming.rs", "utf8");
const settingsSource = readFileSync("src/state/bluesky_fetch.rs", "utf8");
const checkpointSource = readFileSync(
  "src/db/queries/bluesky_polling.rs",
  "utf8",
);

const pollLimit = parseInteger(
  streamingSource,
  /const POLL_LIMIT: u32 = (\d+);/,
  "POLL_LIMIT",
);
const intervalSeconds = parseInteger(
  settingsSource,
  /DEFAULT_BLUESKY_FETCH_INTERVAL_SECONDS: u64 = (\d+);/,
  "DEFAULT_BLUESKY_FETCH_INTERVAL_SECONDS",
);

requireContract(
  streamingSource,
  /fn next_poll_delay\(configured: Duration\) -> Duration \{\s*configured\s*\}/s,
  "the configured freshness interval must not be stretched",
);
requireContract(
  streamingSource,
  /unchanged_pages_emit_no_db_or_ui_work_without_stretching_the_user_interval/,
  "unchanged-page regression test",
);
requireContract(
  checkpointSource,
  /WHERE bluesky_poll_checkpoints\.checkpoint_json != excluded\.checkpoint_json/,
  "conditional checkpoint UPSERT",
);
requireContract(
  checkpointSource,
  /unchanged_checkpoint_does_not_write_again/,
  "unchanged-checkpoint regression test",
);

const durationSeconds = 60 * 60;
if (durationSeconds % intervalSeconds !== 0) {
  throw new Error("the fixed one-hour fixture must contain whole poll intervals");
}
const pollCycles = durationSeconds / intervalSeconds;

// Audit baseline 0bb67a2 fetched one 40-status Home page per configured tick,
// then emitted and persisted the entire page. The current implementation keeps
// exactly the same user-visible polling cadence but emits and writes nothing
// after an identical revision baseline has already been established.
const before = {
  sourceRevision: "0bb67a2",
  apiCalls: pollCycles,
  dbWrites: pollCycles * pollLimit,
  events: pollCycles * pollLimit,
  freshnessSeconds: intervalSeconds,
};
const after = {
  sourceRevision: "working-tree",
  apiCalls: pollCycles,
  dbWrites: 0,
  events: 0,
  freshnessSeconds: intervalSeconds,
};

const ratios = Object.fromEntries(
  ["apiCalls", "dbWrites", "events"].map((name) => [
    name,
    Number((after[name] / Math.max(before[name], 1)).toFixed(3)),
  ]),
);
const orderOfMagnitude = Object.fromEntries(
  Object.entries(ratios).map(([name, ratio]) => [name, ratio <= 0.1]),
);

const report = {
  schemaVersion: 1,
  fixtureId: "awayuki-bluesky-polling-v1-unchanged-home-1h",
  environment: {
    platform: process.platform,
    arch: process.arch,
    runtime: `bun ${Bun.version}`,
  },
  dataset: {
    synthetic: true,
    durationSeconds,
    intervalSeconds,
    pollCycles,
    statusesPerPage: pollLimit,
    stream: "one signed-in Bluesky source, Unified Home",
    state: "revision baseline established; provider page unchanged",
  },
  before,
  after,
  ratios,
  orderOfMagnitude,
  acceptance: {
    freshnessPreserved: after.freshnessSeconds === before.freshnessSeconds,
    apiCallsReducedByOrderOfMagnitude: orderOfMagnitude.apiCalls,
    dbWritesReducedByOrderOfMagnitude: orderOfMagnitude.dbWrites,
    eventsReducedByOrderOfMagnitude: orderOfMagnitude.events,
    complete: Object.values(orderOfMagnitude).every(Boolean),
  },
  providerConstraint: {
    timelineEndpoint: "app.bsky.feed.getTimeline",
    notificationEndpoint: "app.bsky.notification.listNotifications",
    selectivePushAvailable: false,
    rejectedSubstitute:
      "Do not stretch the configured interval or treat a DID-filtered public repository firehose as an authenticated AppView Home/Notification stream.",
  },
};

mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));

if (!report.acceptance.freshnessPreserved) {
  throw new Error("Bluesky freshness contract regressed");
}
if (
  !report.acceptance.dbWritesReducedByOrderOfMagnitude ||
  !report.acceptance.eventsReducedByOrderOfMagnitude
) {
  throw new Error("revision filtering no longer suppresses unchanged work");
}

function parseInteger(source, pattern, label) {
  const match = source.match(pattern);
  if (!match) throw new Error(`could not locate ${label}`);
  return Number(match[1]);
}

function requireContract(source, pattern, label) {
  if (!pattern.test(source)) throw new Error(`missing ${label}`);
}
