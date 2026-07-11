import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const read = (path) => readFileSync(resolve(root, path), "utf8");
const failures = [];

const commands = read("src/application/desktop.rs");
const runtimeCommands = read("src/ipc/runtime.rs");
const app = read("frontend/src/components/App.tsx");
const store = read("frontend/src/store/actions/sessionActions.ts");
const setupStart = commands.indexOf(".setup(|app| {");
const handlerStart = commands.indexOf(".invoke_handler(", setupStart);
if (setupStart < 0 || handlerStart < 0) {
  failures.push("Tauri setup boundary could not be located");
} else {
  const setup = commands.slice(setupStart, handlerStart);
  for (const forbidden of [
    "run_migrations",
    "foreign_key_check",
    "quick_check",
    "restart_streaming",
    "schedule_startup_sync",
    "schedule_post_ready_work",
    "run_to_completion",
    "schedule_runtime_initialization",
  ]) {
    if (setup.includes(forbidden)) {
      failures.push(`blocking work leaked into Tauri setup: ${forbidden}`);
    }
  }
  for (const required of [
    "open_runtime_state",
    "app.manage",
  ]) {
    if (!setup.includes(required)) {
      failures.push(`Tauri setup is missing the non-blocking startup step: ${required}`);
    }
  }
}

if (!runtimeCommands.includes("start_runtime_initialization")) {
  failures.push("frontend-ready startup handshake command is missing");
}
if (!commands.includes("start_runtime_initialization_impl") ||
    !commands.includes("schedule_runtime_initialization(state.inner().clone(), app)")) {
  failures.push("frontend-ready handshake does not start the background initializer");
}
if (!store.includes('invokeCommand("start_runtime_initialization")')) {
  failures.push("snapshot loading does not send the frontend-ready handshake");
}
const listenIndex = app.indexOf('listen<AppStartupProgressEvent>(');
const loadIndex = app.indexOf("void loadSnapshot();", listenIndex);
if (listenIndex < 0 || loadIndex < listenIndex) {
  failures.push("startup progress listener must be registered before snapshot loading");
}

for (const required of [
  "app-startup-progress",
  "wait_until_ready",
  "schedule_post_ready_work",
]) {
  if (!commands.includes(required)) {
    failures.push(`startup progress/readiness contract is missing: ${required}`);
  }
}

const readyIndex = commands.indexOf("state.startup.mark_ready();");
const postReadyScheduleIndex = commands.indexOf(
  "schedule_post_ready_work(state);",
  readyIndex,
);
if (readyIndex < 0 || postReadyScheduleIndex < readyIndex) {
  failures.push("startup reconciliation must be scheduled only after readiness is observable");
}

const postReadyStart = commands.indexOf("fn schedule_post_ready_work(");
const postReadyEnd = commands.indexOf("async fn run_startup_sync(", postReadyStart);
if (postReadyStart < 0 || postReadyEnd < 0) {
  failures.push("post-ready startup coordinator could not be located");
} else {
  const postReadyCoordinator = commands.slice(postReadyStart, postReadyEnd);
  const startupSyncIndex = postReadyCoordinator.indexOf("run_startup_sync(&state).await;");
  const backfillIndex = postReadyCoordinator.indexOf("schedule_status_search_backfill(&state);");
  if (startupSyncIndex < 0 || backfillIndex < startupSyncIndex) {
    failures.push("status search backfill must start after startup synchronization completes");
  }
}

const startupSyncStart = commands.indexOf("async fn sync_startup_accounts(");
const startupSyncEnd = commands.indexOf("async fn emit_startup_sync_event(", startupSyncStart);
if (startupSyncStart < 0 || startupSyncEnd < 0) {
  failures.push("startup synchronization implementation could not be located");
} else if (
  commands.slice(startupSyncStart, startupSyncEnd).includes("run_idle_maintenance")
) {
  failures.push("unbounded retention maintenance must not run automatically after startup sync");
}

const pool = read("src/db/pool.rs");
if (/"PRAGMA\s+quick_check/i.test(pool)) {
  failures.push("full quick_check may not run on the application startup path");
}
if (!pool.includes("20 =>") || !pool.includes("status_search_schema_only_sql")) {
  failures.push("legacy FTS backfill is not separated from migration 020 schema setup");
}

const searchBackfill = read("src/services/search_backfill.rs");
for (const required of [
  "status_search_backfill_state",
  "run_chunk",
  "transaction.commit()",
  "tokio::time::sleep",
  "status_count == document_count && document_count == fts_count",
  "count_index_rows(reader).await?",
  "let mut transaction = writer.begin().await?",
  "FROM cache_counters WHERE name = 'statuses'",
  "FROM status_search_fts_docsize",
]) {
  if (!searchBackfill.includes(required)) {
    failures.push(`resumable search backfill invariant is missing: ${required}`);
  }
}
const countFunctionStart = searchBackfill.indexOf("async fn count_index_rows(");
const countFunctionEnd = searchBackfill.indexOf("async fn load_state(", countFunctionStart);
const countFunction = searchBackfill.slice(countFunctionStart, countFunctionEnd);
if (countFunction.includes("COUNT(*) FROM status_search_fts)")) {
  failures.push("startup backfill must not scan the FTS virtual table to count indexed rows");
}
const chunkStart = searchBackfill.indexOf("pub async fn run_chunk(");
const countProbeIndex = searchBackfill.indexOf("count_index_rows(reader).await?", chunkStart);
const writerTransactionIndex = searchBackfill.indexOf(
  "let mut transaction = writer.begin().await?",
  chunkStart,
);
if (
  chunkStart < 0 ||
  countProbeIndex < chunkStart ||
  writerTransactionIndex < countProbeIndex
) {
  failures.push("the initial FTS count probe must finish on a reader before acquiring the writer");
}
if (!commands.includes("database.writer(),\n                database.reader(),")) {
  failures.push("the post-ready search backfill must receive separate writer and reader pools");
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}

console.log("non-blocking startup and resumable migration boundaries verified");
