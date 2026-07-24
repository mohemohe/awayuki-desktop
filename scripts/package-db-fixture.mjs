import { Database } from "bun:sqlite";
import {
  createHash,
} from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const [command, databaseArg, ...rest] = process.argv.slice(2);
if (!command || !databaseArg) usage();
const databasePath = resolve(databaseArg);

if (command === "create-legacy") createLegacy();
else if (command === "verify-fresh") verify(false);
else if (command === "verify-upgraded") verify(true);
else if (command === "report") report();
else usage();

function createLegacy() {
  mkdirSync(dirname(databasePath), { recursive: true });
  for (const suffix of ["", "-wal", "-shm"]) {
    rmSync(`${databasePath}${suffix}`, { force: true });
  }
  const db = new Database(databasePath, { create: true, strict: true });
  db.exec("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON;");
  for (const migration of migrationFiles().filter((item) => item.version <= 19)) {
    db.exec(readFileSync(migration.path, "utf8"));
  }
  db.query(
    `INSERT INTO servers(domain, streaming_url, server_kind)
     VALUES ('127.0.0.1:9', 'ws://127.0.0.1:9', 'mastodon')`,
  ).run();
  db.query(
    `INSERT INTO accounts
     (id, server_domain, username, acct, display_name, created_at)
     VALUES ('package-user', '127.0.0.1:9', 'package-user',
             'package-user@127.0.0.1:9', 'Package Fixture',
             '2020-01-01T00:00:00Z')`,
  ).run();
  db.query(
    `INSERT INTO login_accounts
     (acct, server_domain, account_id, display_name, is_active, access_token,
      server_kind, app_password)
     VALUES ('package-user@127.0.0.1:9', '127.0.0.1:9', 'package-user',
             'Package Fixture', 0, 'fixture-token-not-a-secret', 'mastodon', NULL)`,
  ).run();
  db.query(
    `INSERT INTO statuses
     (id, server_domain, uri, created_at, account_id, content, fetched_at)
     VALUES ('package-status', '127.0.0.1:9',
             'https://127.0.0.1:9/statuses/package-status',
             '2020-01-01T00:00:00Z', 'package-user',
             'synthetic package fixture', '2020-01-01T00:00:00Z')`,
  ).run();
  db.query(
    `INSERT INTO timeline_entries
     (timeline_type, server_domain, status_id, account_acct, position_at)
     VALUES ('home', '127.0.0.1:9', 'package-status',
             'package-user@127.0.0.1:9', '2020-01-01T00:00:00Z')`,
  ).run();
  db.query(
    `INSERT INTO notifications
     (id, server_domain, notification_type, created_at, account_id, status_id,
      fetched_at, account_acct)
     VALUES ('package-notification', '127.0.0.1:9', 'mention',
             '2020-01-01T00:00:00Z', 'package-user', 'package-status',
             '2020-01-01T00:00:00Z', 'package-user@127.0.0.1:9')`,
  ).run();
  db.query(
    "INSERT INTO app_settings(key, value) VALUES ('package_smoke_sentinel', 'preserve-me')",
  ).run();
  db.close();
  console.log(`created migration-019 legacy fixture at ${databasePath}`);
}

function verify(requireLegacySentinel) {
  if (!existsSync(databasePath) || statSync(databasePath).size < 1) {
    throw new Error(`database is missing or empty: ${databasePath}`);
  }
  const db = new Database(databasePath, { readonly: true, strict: true });
  const expectedVersion = migrationFiles().at(-1).version;
  const applied = db
    .query(
      "SELECT version FROM _sqlx_migrations WHERE success = TRUE ORDER BY version",
    )
    .all()
    .map((row) => Number(row.version));
  if (applied.at(-1) !== expectedVersion) {
    throw new Error(
      `schema is not ready: expected ${expectedVersion}, got ${applied.at(-1) ?? "none"}`,
    );
  }
  // The migrated database deliberately retains migration 031's dormant FTS
  // parent until an explicit cache clear. Its tokenizer is a Rust callback and
  // cannot be registered by Bun. A database-wide quick_check would try to
  // connect that retired virtual table and report a verifier-environment
  // error instead of corruption. Check every ordinary table (including FTS
  // shadow B-trees) plus the current built-in-tokenizer ICU virtual table.
  const quickCheckTables = db
    .query(
      `SELECT name
         FROM sqlite_schema
        WHERE type = 'table'
          AND name NOT LIKE 'sqlite_%'
          AND upper(trim(sql)) NOT LIKE 'CREATE VIRTUAL TABLE%'
        ORDER BY name`,
    )
    .all()
    .map((row) => String(row.name));
  quickCheckTables.push("status_search_icu_fts", "account_search_icu_fts");
  for (const table of quickCheckTables) {
    const quotedTable = table.replaceAll("'", "''");
    const integrity = db.query(`PRAGMA quick_check('${quotedTable}')`).get();
    if (Object.values(integrity ?? {})[0] !== "ok") {
      throw new Error(
        `SQLite quick_check failed for ${table}: ${JSON.stringify(integrity)}`,
      );
    }
  }
  const foreignKeyViolations = db.query("PRAGMA foreign_key_check").all();
  if (foreignKeyViolations.length) {
    throw new Error(
      `SQLite foreign_key_check failed: ${JSON.stringify(foreignKeyViolations.slice(0, 5))}`,
    );
  }
  if (hasTable(db, "client_credentials")) {
    throw new Error("removed client credential table is still present");
  }
  for (const table of [
    "status_search_icu_content",
    "status_search_icu_fts",
    "status_search_index_queue",
    "status_search_icu_backfill_state",
    "status_search_index_control",
    "account_search_icu_content",
    "account_search_icu_fts",
    "account_search_index_queue",
    "account_search_icu_backfill_state",
  ]) {
    if (!hasTable(db, table)) {
      throw new Error(`latest asynchronous ICU search table is missing: ${table}`);
    }
  }
  const indexState = db
    .query(
      `SELECT control.index_updates_enabled AS enabled,
              control.merge_debt AS status_merge_debt,
              control.account_merge_debt AS account_merge_debt,
              status_backfill.completed AS status_completed,
              account_backfill.completed AS account_completed,
              (SELECT COUNT(*) FROM status_search_index_queue) AS status_pending,
              (SELECT COUNT(*) FROM account_search_index_queue) AS account_pending
         FROM status_search_index_control control
         JOIN status_search_icu_backfill_state status_backfill
           ON status_backfill.singleton = control.singleton
         JOIN account_search_icu_backfill_state account_backfill
           ON account_backfill.singleton = control.singleton
        WHERE control.singleton = 1`,
    )
    .get();
  if (
    Number(indexState?.enabled) !== 1 ||
    Number(indexState?.status_completed) !== 1 ||
    Number(indexState?.account_completed) !== 1 ||
    Number(indexState?.status_pending) !== 0 ||
    Number(indexState?.account_pending) !== 0
  ) {
    throw new Error(
      `asynchronous ICU search index is not settled: ${JSON.stringify(indexState)}`,
    );
  }
  if (requireLegacySentinel) {
    const sentinel = db
      .query("SELECT value FROM app_settings WHERE key = 'package_smoke_sentinel'")
      .get();
    if (sentinel?.value !== "preserve-me") {
      throw new Error("legacy app setting was not preserved");
    }
    const account = db
      .query(
        `SELECT access_token FROM login_accounts
          WHERE acct = 'package-user@127.0.0.1:9'`,
      )
      .get();
    if (account?.access_token !== "fixture-token-not-a-secret") {
      throw new Error("SQLite-only login credential was not preserved");
    }
    const identity = db
      .query(
        `SELECT canonical_uri FROM status_identities
          WHERE status_id = 'package-status' AND server_domain = '127.0.0.1:9'`,
      )
      .get();
    if (!identity) throw new Error("legacy status identity was not upgraded");
    const indexedStatus = db
      .query(
        `SELECT 1 AS indexed
           FROM status_search_icu_content
          WHERE status_id = 'package-status'
            AND server_domain = '127.0.0.1:9'`,
      )
      .get();
    if (Number(indexedStatus?.indexed) !== 1) {
      throw new Error("legacy status was not incorporated into the ICU index");
    }
    const indexedAccount = db
      .query(
        `SELECT 1 AS indexed
           FROM account_search_icu_content
          WHERE account_id = 'package-user'
            AND server_domain = '127.0.0.1:9'`,
      )
      .get();
    if (Number(indexedAccount?.indexed) !== 1) {
      throw new Error("legacy account was not incorporated into the ICU index");
    }
  }
  db.close();
  assertNoRecoveryCopies();
  console.log(
    `verified schema ${expectedVersion}${requireLegacySentinel ? " and legacy state" : ""}`,
  );
}

function report() {
  const [
    outputArg,
    platform,
    artifactArg,
    binaryRemovedArg,
    binaryBytesArg,
    releaseSecurityArg,
    releaseWebviewArg,
  ] = rest;
  if (!outputArg || !platform || !artifactArg) usage();
  verify(true);
  const output = resolve(outputArg);
  const packageBytes = statSync(resolve(artifactArg)).size;
  const binaryBytes = Number(binaryBytesArg);
  if (!Number.isSafeInteger(binaryBytes) || binaryBytes < 1) {
    throw new Error(`invalid packaged binary size: ${binaryBytesArg}`);
  }
  const binarySizePassed = binaryBytes <= 120 * 1024 * 1024;
  const packageSizePassed = packageBytes <= 300 * 1024 * 1024;
  const reportValue = {
    schemaVersion: 1,
    fixtureId: "awayuki-package-smoke-v1",
    platform,
    artifact: basename(artifactArg),
    result: "passed",
    tests: {
      packageContents: true,
      freshDatabaseLaunch: true,
      legacyDatabaseUpgrade: true,
      upgradedDatabaseRestart: true,
      uninstallRemovedBinary: binaryRemovedArg === "true",
      uninstallPreservedDatabase: existsSync(databasePath),
      sqliteOnlyStatePreserved: true,
      automaticRecoveryCopyAbsent: true,
      releaseSecurityAttested: releaseSecurityArg === "true",
      remoteImageLoaded: releaseWebviewArg === "true",
      protocolMediaLoaded: releaseWebviewArg === "true",
      customEmojiLoaded: releaseWebviewArg === "true",
      remoteVideoLoaded: releaseWebviewArg === "true",
      sidecarPreviewHideRestore: releaseWebviewArg === "true",
      cspViolationReportClean: releaseWebviewArg === "true",
      binarySizeBudget: binarySizePassed,
      packageSizeBudget: packageSizePassed,
    },
    buildMetrics: {
      binaryBytes,
      binaryBudgetBytes: 120 * 1024 * 1024,
      packageBytes,
      packageBudgetBytes: 300 * 1024 * 1024,
    },
    database: {
      bytes: statSync(databasePath).size,
      sha256: createHash("sha256")
        .update(readFileSync(databasePath))
        .digest("hex"),
      latestMigration: migrationFiles().at(-1).version,
    },
  };
  if (Object.values(reportValue.tests).some((passed) => !passed)) {
    throw new Error("one or more package smoke assertions did not pass");
  }
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, `${JSON.stringify(reportValue, null, 2)}\n`);
  console.log(JSON.stringify(reportValue, null, 2));
}

function hasTable(db, name) {
  return Boolean(
    db.query("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?")
      .get(name),
  );
}

function assertNoRecoveryCopies() {
  const name = basename(databasePath);
  const forbidden = readdirSync(dirname(databasePath)).filter(
    (entry) =>
      entry.startsWith(name) &&
      entry !== name &&
      entry !== `${name}-wal` &&
      entry !== `${name}-shm`,
  );
  if (forbidden.length) {
    throw new Error(
      `unexpected database backup/recovery copy exists: ${forbidden.join(", ")}`,
    );
  }
}

function migrationFiles() {
  return readdirSync(join(root, "migrations"))
    .filter((name) => /^\d+_.+\.sql$/.test(name))
    .map((name) => ({
      version: Number(name.match(/^(\d+)_/)[1]),
      path: join(root, "migrations", name),
    }))
    .sort((left, right) => left.version - right.version);
}

function usage() {
  console.error(
    "usage: package-db-fixture.mjs create-legacy|verify-fresh|verify-upgraded DATABASE\n" +
      "       package-db-fixture.mjs report DATABASE OUTPUT PLATFORM ARTIFACT BINARY_REMOVED BINARY_BYTES RELEASE_SECURITY_PASSED RELEASE_WEBVIEW_PASSED",
  );
  process.exit(2);
}
