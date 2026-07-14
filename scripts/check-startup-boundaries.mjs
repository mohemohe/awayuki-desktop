import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const read = (path) => readFileSync(resolve(root, path), "utf8");
const failures = [];

for (const entry of readdirSync(resolve(root, "src/application"), {
  withFileTypes: true,
})) {
  if (!entry.isFile() || !entry.name.endsWith(".rs")) continue;
  const source = read(`src/application/${entry.name}`);
  for (const forbidden of [
    "hydrate_and_resolve_quotes(",
    "hydrate_missing_quotes(",
    "resolve_pending_quotes_with_backoff(",
  ]) {
    if (source.includes(forbidden)) {
      failures.push(
        `quote network hydration leaked into application response path: ${entry.name}:${forbidden}`,
      );
    }
  }
}

const commands = read("src/application/desktop.rs");
const runtimeApplication = read("src/application/runtime.rs");
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
if (!runtimeApplication.includes("start_runtime_initialization") ||
    !runtimeApplication.includes("spawn_runtime_initialization_worker(state.inner().clone(), app, operation_id)")) {
  failures.push("frontend-ready handshake does not start the background initializer");
}
if (!store.includes('invokeTypedCommand("start_runtime_initialization")')) {
  failures.push("snapshot loading does not send the frontend-ready handshake");
}
const listenIndex = app.indexOf('listen<AppStartupProgressEvent>(');
const loadIndex = app.indexOf("void loadSnapshot();", listenIndex);
if (listenIndex < 0 || loadIndex < listenIndex) {
  failures.push("startup progress listener must be registered before snapshot loading");
}

for (const required of [
  "APP_STARTUP_PROGRESS_EVENT",
  "wait_until_ready",
]) {
  if (!runtimeApplication.includes(required)) {
    failures.push(`startup progress/readiness contract is missing: ${required}`);
  }
}

const readyIndex = runtimeApplication.indexOf("state.startup_gate().mark_ready();");
const postReadyScheduleIndex = runtimeApplication.indexOf(
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
  const indexerIndex = postReadyCoordinator.indexOf("schedule_status_search_indexer(&state);");
  if (indexerIndex < 0 || startupSyncIndex < indexerIndex) {
    failures.push("low-priority status search indexing must start post-ready without waiting for network synchronization");
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

const searchIndexer = read("src/services/search_indexer.rs");
for (const required of [
  "status_search_icu_backfill_state",
  "status_search_index_queue",
  "run_index_step",
  "writer.try_acquire()",
  "icu_search::index_text",
  "transaction.commit()",
  "tokio::time::sleep",
  "progress_tx.try_send",
  "AND generation = ?",
  "upsert_status_backfill_content_if_current",
  "upsert_account_backfill_content_if_current",
  "FROM cache_counters WHERE name = 'statuses'",
  "FROM cache_counters WHERE name = 'accounts'",
  "enter_low_priority_write",
  "load_merge_debt",
  "select_merge_target",
  "const QUEUE_CHUNK_SIZE: i64 = 8",
  "const BACKFILL_CHUNK_SIZE: i64 = 32",
]) {
  if (!searchIndexer.includes(required)) {
    failures.push(`low-priority ICU search indexer invariant is missing: ${required}`);
  }
}
const icuSearch = read("src/db/icu_search.rs");
const sqliteSearchExtensions = read("src/db/short_search_tokenizer.rs");
for (const required of [
  "matches_fields",
  "matches_index_text",
  "normalize_and_fold",
]) {
  if (!icuSearch.includes(required)) {
    failures.push(`ICU search semantic invariant is missing: ${required}`);
  }
}
for (const required of [
  "awayuki_icu_match",
  "awayuki_icu_index_match",
  "sqlite3_create_function_v2",
]) {
  if (!sqliteSearchExtensions.includes(required)) {
    failures.push(`connection-local ICU search extension is missing: ${required}`);
  }
}
if (!commands.includes("database.writer(),\n                database.reader(),")) {
  failures.push("the post-ready search indexer must receive separate writer and reader pools");
}

const asyncSearchMigration = read("migrations/032_async_icu_status_search.sql");
for (const required of [
  "DROP TRIGGER IF EXISTS status_search_fts_status_insert",
  "DROP TRIGGER IF EXISTS status_search_short_status_insert",
  "CREATE TABLE status_search_index_queue",
  "CREATE TRIGGER status_search_index_status_insert",
  "generation BLOB NOT NULL DEFAULT (randomblob(16))",
]) {
  if (!asyncSearchMigration.includes(required)) {
    failures.push(`asynchronous search migration invariant is missing: ${required}`);
  }
}

const asyncSearchControlMigration = read("migrations/033_control_async_search_index.sql");
for (const required of [
  "CREATE TABLE status_search_index_control",
  "index_updates_enabled",
  "merge_debt",
  "cache_counter_status_delete",
  "DROP TRIGGER IF EXISTS status_search_index_status_delete",
]) {
  if (!asyncSearchControlMigration.includes(required)) {
    failures.push(`bulk-safe asynchronous search control is missing: ${required}`);
  }
}
const asyncAccountSearchMigration = read("migrations/034_async_icu_account_search.sql");
for (const required of [
  "ADD COLUMN account_merge_debt",
  "CREATE TABLE account_search_icu_content",
  "CREATE VIRTUAL TABLE account_search_icu_fts",
  "CREATE TABLE account_search_index_queue",
  "CREATE TABLE account_search_icu_backfill_state",
  "CREATE TRIGGER account_search_index_account_insert",
  "CREATE TRIGGER account_search_index_account_update",
  "AFTER UPDATE OF id, server_domain, acct, display_name ON accounts",
  "SELECT OLD.id, OLD.server_domain, 'delete'",
  "CREATE TRIGGER account_search_index_account_delete",
]) {
  if (!asyncAccountSearchMigration.includes(required)) {
    failures.push(`asynchronous account search migration invariant is missing: ${required}`);
  }
}
const icuSegmentRefreshMigration = read(
  "migrations/035_reindex_icu_nonword_segments.sql",
);
for (const required of [
  "UPDATE status_search_icu_backfill_state",
  "UPDATE account_search_icu_backfill_state",
  "cursor_status_id = NULL",
  "cursor_account_id = NULL",
  "processed_count = 0",
]) {
  if (!icuSegmentRefreshMigration.includes(required)) {
    failures.push(`bounded ICU segment refresh invariant is missing: ${required}`);
  }
}
for (const forbidden of [
  "DELETE FROM status_search_icu_content",
  "INSERT INTO status_search_index_queue",
  "FROM statuses",
]) {
  if (icuSegmentRefreshMigration.includes(forbidden)) {
    failures.push(`ICU segment refresh must remain O(1) at startup: ${forbidden}`);
  }
}
const settingsQueries = read("src/db/queries/settings.rs");
for (const required of [
  "SET index_updates_enabled = 0",
  "VALUES ('delete-all')",
  "DELETE FROM status_search_index_queue",
  "DELETE FROM account_search_index_queue",
  "UPDATE account_search_icu_backfill_state",
  "account_merge_debt = 0",
  "UPDATE cache_counters",
  "SET index_updates_enabled = 1",
]) {
  if (!settingsQueries.includes(required)) {
    failures.push(`bulk cache clear search-index invariant is missing: ${required}`);
  }
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}

console.log("non-blocking startup and low-priority search indexing boundaries verified");
