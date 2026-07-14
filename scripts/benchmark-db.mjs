import { Database } from "bun:sqlite";
import { mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const statusCount = Number(process.argv[2] ?? 20_000);
if (!Number.isSafeInteger(statusCount) || statusCount < 1 || statusCount > 1_000_000) {
  throw new Error("status count must be between 1 and 1,000,000");
}
mkdirSync(join(root, "build"), { recursive: true });
const databasePath = join(root, "build", `benchmark-${statusCount}.db`);
const metricsPath = join(root, "build", `benchmark-${statusCount}.json`);
rmSync(databasePath, { force: true });

const db = new Database(databasePath, { create: true, strict: true });
db.exec("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA foreign_keys=ON;");
// Bun's SQLite connection cannot install Awayuki's process-local Rust FTS5
// tokenizer. Still execute migration 031 so its table/trigger inventory and
// cleanup match production; substitute a built-in tokenizer only for the
// now-dormant legacy short-index declaration. Migration 032 drops that index's
// status triggers before fixture rows are inserted. ICU tokenization semantics
// stay in Rust fixtures, and production-path cost must be measured there rather
// than approximated in JavaScript.
const applicationTokenizerSubstitutions = new Map([
  [
    "031_create_short_search_fts.sql",
    ["tokenize = 'awayuki_short'", "tokenize = 'unicode61 remove_diacritics 2'"],
  ],
]);
for (const migration of readdirSync(join(root, "migrations")).sort()) {
  if (!migration.endsWith(".sql")) continue;
  let sql = readFileSync(join(root, "migrations", migration), "utf8");
  const substitution = applicationTokenizerSubstitutions.get(migration);
  if (substitution) {
    const [from, to] = substitution;
    if (!sql.includes(from)) {
      throw new Error(`tokenizer substitution target is missing from ${migration}`);
    }
    sql = sql.replace(from, to);
  }
  db.exec(sql);
}

for (const requiredTable of [
  "status_search_short_content",
  "status_search_short_fts",
  "status_search_short_backfill_state",
]) {
  const row = db.query("SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?").get(
    requiredTable,
  );
  if (!row) throw new Error(`migration 031 stand-in table is missing: ${requiredTable}`);
}
for (const obsoleteObject of [
  "status_search_char_fts",
  "status_search_char_positions",
  "status_search_char_backfill_state",
  "status_search_fts_account_update",
  "status_search_fts_account_insert",
  "status_search_fts_account_delete",
  "status_search_fts_status_insert",
  "status_search_fts_status_update",
  "status_search_fts_status_delete",
  "status_search_short_status_insert",
  "status_search_short_status_update",
  "status_search_short_status_delete",
]) {
  const row = db.query("SELECT type FROM sqlite_schema WHERE name = ?").get(obsoleteObject);
  if (row) throw new Error(`obsolete search object survived migrations: ${obsoleteObject}`);
}

db.query(
  "INSERT INTO servers (domain, streaming_url, server_kind) VALUES (?, ?, ?)",
).run("benchmark.invalid", "wss://benchmark.invalid", "mastodon");
const insertAccount = db.prepare(
  `INSERT INTO accounts
   (id, server_domain, username, acct, display_name, created_at)
   VALUES (?, 'benchmark.invalid', ?, ?, ?, ?)`,
);
const seedAccounts = db.transaction(() => {
  for (let index = 0; index < 1_000; index += 1) {
    const id = `account-${index.toString().padStart(4, "0")}`;
    insertAccount.run(id, id, `${id}@benchmark.invalid`, `User ${index}`, "2020-01-01T00:00:00Z");
  }
});
seedAccounts();
db.query(
  `INSERT INTO login_accounts
   (acct, server_domain, account_id, display_name, avatar, is_active,
    access_token, server_kind)
   VALUES ('account-0000@benchmark.invalid', 'benchmark.invalid', 'account-0000',
           'Benchmark Viewer', '', 1, 'benchmark-token', 'mastodon')`,
).run();

const insertStatus = db.prepare(
  `INSERT INTO statuses
   (id, server_domain, uri, created_at, account_id, content, visibility,
    spoiler_text, tags_json, fetched_at, in_reply_to_id)
   VALUES (?, 'benchmark.invalid', ?, ?, ?, ?, ?, ?, '[]', ?, ?)`,
);
// Fixture creation is an explicit bulk operation, not the production ingest
// path under measurement. Avoid materializing hundreds of thousands of queue
// rows only to discard them before the synthetic ICU corpus is installed.
db.exec(
  "UPDATE status_search_index_control SET index_updates_enabled = 0 WHERE singleton = 1",
);
const insertTimeline = db.prepare(
  `INSERT INTO timeline_entries
   (timeline_type, server_domain, status_id, account_acct, position_at)
   VALUES ('home', 'benchmark.invalid', ?, 'account-0000@benchmark.invalid', ?)`,
);
const baseTime = Date.parse("2026-01-01T00:00:00Z");
const seedStarted = performance.now();
const seedStatuses = db.transaction((start, end) => {
  for (let index = start; index < end; index += 1) {
    const id = `status-${index.toString().padStart(7, "0")}`;
    const createdAt = new Date(baseTime - index * 1_000).toISOString();
    const account = `account-${(index % 1_000).toString().padStart(4, "0")}`;
    const marker =
      index % 113 === 0
        ? "東京 短文"
        : index % 97 === 0
          ? "benchmark needle"
          : "ordinary timeline text";
    insertStatus.run(
      id,
      `https://benchmark.invalid/statuses/${id}`,
      createdAt,
      account,
      `${marker} ${index}`,
      index % 11 === 0 ? "private" : "public",
      index % 101 === 0 ? "content warning" : "",
      createdAt,
      index > 0 && index < 256
        ? `status-${(index - 1).toString().padStart(7, "0")}`
        : null,
    );
    if (index < Math.min(statusCount, 50_000)) insertTimeline.run(id, createdAt);
  }
});
for (let start = 0; start < statusCount; start += 5_000) {
  seedStatuses(start, Math.min(start + 5_000, statusCount));
}
db.query(
  "UPDATE cache_counters SET value = ?, updated_at = datetime('now') WHERE name = 'statuses'",
).run(statusCount);
db.exec(
  "UPDATE status_search_index_control SET index_updates_enabled = 1 WHERE singleton = 1",
);

const encodeFtsToken = (value) =>
  `x${Buffer.from(value.normalize("NFKC").toLowerCase(), "utf8").toString("hex")}`;
const benchmarkFtsToken = encodeFtsToken("benchmark");
db.query(
  `INSERT INTO status_search_icu_content(status_id, server_domain, token_text)
   SELECT id, server_domain, ? || ' ' || ?
     FROM statuses
    WHERE content LIKE '%benchmark%'`,
).run(benchmarkFtsToken, encodeFtsToken("needle"));
db.exec("DELETE FROM status_search_index_queue");

const notificationCount = Math.min(statusCount, 20_000);
const insertNotification = db.prepare(
  `INSERT INTO notifications
   (id, server_domain, account_acct, notification_type, created_at,
    account_id, status_id, fetched_at)
   VALUES (?, 'benchmark.invalid', 'account-0000@benchmark.invalid', ?, ?, ?, ?, ?)`,
);
const seedNotifications = db.transaction(() => {
  for (let index = 0; index < notificationCount; index += 1) {
    const statusId = `status-${index.toString().padStart(7, "0")}`;
    const createdAt = new Date(baseTime - index * 1_000).toISOString();
    const account = `account-${(index % 1_000).toString().padStart(4, "0")}`;
    insertNotification.run(
      `notification-${index.toString().padStart(7, "0")}`,
      index % 2 === 0 ? "mention" : "favourite",
      createdAt,
      account,
      statusId,
      createdAt,
    );
  }
});
seedNotifications();
db.exec("ANALYZE");
const seedMs = performance.now() - seedStarted;

const cases = {
  ftsFirstPage: {
    sql: `SELECT d.status_id FROM status_search_icu_fts f
          JOIN status_search_icu_content d ON d.docid = f.rowid
          WHERE status_search_icu_fts MATCH '"${benchmarkFtsToken}"*' LIMIT 40`,
    budgetMs: statusCount >= 400_000 ? 250 : 100,
  },
  aggregateHome: {
    sql: `WITH candidate_entries AS (
            SELECT te.server_domain,
                   te.status_id,
                   te.account_acct AS source_acct,
                   te.position_at AS latest_position,
                   COALESCE(NULLIF(identity.canonical_uri, ''), NULLIF(s.uri, ''),
                            te.server_domain || ':' || te.status_id) AS canonical_uri
              FROM timeline_entries te
              JOIN statuses s
                ON s.id = te.status_id AND s.server_domain = te.server_domain
              LEFT JOIN status_identities identity
                ON identity.status_id = te.status_id
               AND identity.server_domain = te.server_domain
             WHERE te.timeline_type = 'home'
             ORDER BY te.position_at DESC, te.server_domain DESC,
                      te.status_id DESC, te.account_acct DESC
             LIMIT 512
          ), ranked AS (
            SELECT *, ROW_NUMBER() OVER (
              PARTITION BY canonical_uri
              ORDER BY latest_position DESC, server_domain DESC,
                       status_id DESC, source_acct DESC
            ) AS canonical_rank
            FROM candidate_entries
          )
          SELECT server_domain, status_id, source_acct
            FROM ranked
           WHERE canonical_rank = 1
           ORDER BY latest_position DESC, server_domain DESC, status_id DESC
           LIMIT 40`,
    budgetMs: statusCount >= 400_000 ? 120 : 50,
  },
  statusCount: {
    sql: "SELECT value FROM cache_counters WHERE name = 'statuses'",
    budgetMs: 5,
  },
  statusCountFullScanReference: {
    sql: "SELECT COUNT(*) FROM statuses",
    budgetMs: statusCount >= 400_000 ? 120 : 50,
  },
  yqCandidatePage: {
    sql: `SELECT s.id, s.content, s.visibility, s.server_domain
            FROM statuses s
           WHERE s.visibility = 'public'
             AND s.server_domain = 'benchmark.invalid'
           ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
           LIMIT 250`,
    budgetMs: statusCount >= 400_000 ? 100 : 50,
  },
  notificationPage: {
    sql: `SELECT n.id, n.server_domain, n.account_acct, n.account_id, n.status_id
            FROM notifications n
           ORDER BY n.created_at DESC, n.server_domain DESC, n.id DESC,
                    n.account_acct DESC
           LIMIT 40`,
    budgetMs: statusCount >= 400_000 ? 80 : 40,
  },
  threadPage: {
    sql: `WITH RECURSIVE
          ancestors(id, server_domain, in_reply_to_id, depth, path) AS (
            SELECT id, server_domain, in_reply_to_id, 0,
                   char(31) || id || char(31)
              FROM statuses
             WHERE id = 'status-0000127' AND server_domain = 'benchmark.invalid'
            UNION ALL
            SELECT parent.id, parent.server_domain, parent.in_reply_to_id,
                   ancestors.depth + 1,
                   ancestors.path || parent.id || char(31)
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
            UNION
            SELECT id, server_domain FROM descendants
          )
          SELECT statuses.id, statuses.server_domain
            FROM selected
            -- Keep the bounded CTE outermost; a reorder scans the entire status cache.
            CROSS JOIN statuses
              ON statuses.id = selected.id
             AND statuses.server_domain = selected.server_domain
           LIMIT 500`,
    budgetMs: statusCount >= 400_000 ? 100 : 50,
  },
};

const results = {};
for (const [name, testCase] of Object.entries(cases)) {
  const statement = db.prepare(testCase.sql);
  const rows = statement.all();
  const samples = [];
  for (let run = 0; run < 15; run += 1) {
    const started = performance.now();
    statement.all();
    samples.push(performance.now() - started);
  }
  samples.sort((left, right) => left - right);
  results[name] = {
    p50Ms: percentile(samples, 0.5),
    p95Ms: percentile(samples, 0.95),
    budgetMs: testCase.budgetMs,
    resultRows: rows.length,
    statementCount: 1,
    plan: db.prepare(`EXPLAIN QUERY PLAN ${testCase.sql}`).all(),
  };
}

// Exercise the production status-update path repeatedly. Migration 032 must
// keep this work to 24 coalesced queue keys; ICU segmentation and FTS writes
// belong to the post-ready indexer and therefore cannot enter this timing.
const updateWriteBatch = db.prepare(
  `UPDATE statuses
      SET content = content || ?
    WHERE rowid IN (SELECT rowid FROM statuses ORDER BY rowid LIMIT 24)`,
);
const writeStatusBatch = db.transaction((suffix) => updateWriteBatch.run(suffix));
const writeSamples = [];
for (let run = 0; run < 32; run += 1) {
  const started = performance.now();
  writeStatusBatch(run % 2 === 0 ? " " : "\u200b");
  writeSamples.push(performance.now() - started);
}
writeSamples.sort((left, right) => left - right);
results.statusWriteBatch = {
  p50Ms: percentile(writeSamples, 0.5),
  p95Ms: percentile(writeSamples, 0.95),
  budgetMs: statusCount >= 400_000 ? 500 : 150,
  resultRows: 24,
  statementCount: 1,
  plan: db
    .prepare(
      `EXPLAIN QUERY PLAN UPDATE statuses
          SET content = content || ' '
        WHERE rowid IN (SELECT rowid FROM statuses ORDER BY rowid LIMIT 24)`,
    )
    .all(),
};

db.close();
const metrics = {
  schemaVersion: 1,
  environment: { runtime: `bun ${Bun.version}`, platform: process.platform, arch: process.arch },
  dataset: {
    statuses: statusCount,
    accounts: 1_000,
    notifications: notificationCount,
    threadDepth: Math.min(statusCount, 256),
    synthetic: true,
    seed: "awayuki-v4-async-icu",
    applicationTokenizerSubstitutions: Object.fromEntries(
      applicationTokenizerSubstitutions,
    ),
  },
  seedMs,
  databaseBytes: statSync(databasePath).size,
  results,
};
writeFileSync(metricsPath, `${JSON.stringify(metrics, null, 2)}\n`);
console.log(JSON.stringify(metrics, null, 2));

const failures = Object.entries(results)
  .filter(([, result]) => result.p95Ms > result.budgetMs)
  .map(([name, result]) => `${name} p95 ${result.p95Ms}ms exceeds ${result.budgetMs}ms`);
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}

function percentile(sorted, value) {
  return Number(sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * value) - 1)].toFixed(3));
}
