# Performance budgets and baseline

## 2026-07-11 large portable database incident

An ad-hoc macOS bundle built as `9.9.9` spent **132,646 ms** in the startup DB
phase before the first usable UI. The inspected portable database contained
929,957 cached statuses, and a process sample placed SQLite in the FTS5 index
merge path. Migration 020 was rebuilding the entire trigram index in one
transaction; startup also performed exhaustive integrity work proportional to
the multi-gigabyte cache. This was perceived as a hang because the work ran
before Awayuki could present progress.

The regression contract is now stricter than a total-ready-time budget:

- the Tauri/WebView window must become ready and subscribe to
  `app-startup-progress` before schema/session initialization starts;
- migration 020's schema and triggers are installed without its legacy
  all-at-once data backfill;
- migration 023 persists the FTS cursor in `awayuki.db`, commits at most 64
  statuses per normal chunk (hard maximum 500), and yields between chunks;
- local search retains an exact fallback until the resumable index is complete;
- full `foreign_key_check`/integrity scans are test or explicit-diagnostic work,
  never an unannounced first-window prerequisite.

`bun run startup:check`, the startup gate tests, the resumable backfill tests,
and `frontend/src/components/App.test.tsx` enforce these boundaries. The
historical 132,646 ms value remains the before measurement; post-change bundle
verification must record window/progress availability separately from eventual
background-index completion.

### Post-ready writer starvation regression

A subsequent schema-25 run made the window ready in 920 ms, but then ordinary
timeline/notification writes again timed out. The trace showed 30,371 ms for the
first writer-pool timeout and 154,038--154,330 ms after retries. This was not a WAL
failure or a SQLite `database is locked` error: startup retention had reserved the
only writer connection while running whole-cache window/correlated deletes, and the
FTS completion probe also counted the virtual table from a writer transaction.

The runtime no longer invokes the unbounded retention transaction automatically.
FTS completion is probed on a WAL reader using the status counter, document table,
and FTS `docsize` shadow table before a writer is acquired. A missing index is then
filled in 64-row transactions with a yield between chunks. The startup boundary
check rejects both an automatic retention call and a count probe that occurs after
writer acquisition. On the 420,000-status fixture, 24 interactive status updates
now complete at p95 2.397 ms (500 ms budget).

## Fixed fixtures

The performance workflow creates three deterministic SQLite datasets on separate
GitHub-hosted runners: 20,000 statuses for fast feedback, 420,000 statuses for the
normal large-cache case, and 1,000,000 statuses for the upper-bound cache case. Each
fixture also contains 1,000 synthetic accounts, up to 20,000 notifications, and a
256-status reply chain. Values use the reserved `benchmark.invalid` domain and
explicit non-secret fixture strings; no production content, account identifier, or
credential is read.

```bash
bun run benchmark:db -- 20000
bun run benchmark:db -- 420000
bun run benchmark:db -- 1000000
bun run benchmark:system -- build/benchmark-420000.db build/system-benchmark.json
cargo run --locked --release --example performance-yq -- build/yq-benchmark.json
bun run build
bun run bundle:check
```

`benchmark-db.mjs` rebuilds the schema from every checked-in migration and records
p50/p95, result rows, statement count, database bytes, and `EXPLAIN QUERY PLAN` for
FTS, aggregate timeline, counter, YQ candidate, notification, and recursive thread
queries. `benchmark-system.mjs` copies that synthetic DB to a disposable working
file and measures these fixed workloads:

- the ready milestone before a deterministic in-memory startup adapter performs four
  API-phase responses and four SQLite writes;
- warm startup with zero API requests;
- the three-statement notification context and one-statement recursive thread read;
- SQL-prefiltered YQ candidate evaluation;
- ten seconds equivalent at 100 stream events/s, including queue, drop, resync,
  throughput, and DB lag;
- an eight-hour equivalent bounded-retention state model;
- a streaming 32 MiB media copy with throughput and peak RSS delta.

The YQ example separately runs the real compiled `yq` parser/evaluator against
10,000 fixed statuses. This keeps the portable JS fixture honest while measuring the
same engine used by the application.

## Absolute budgets

The following ceilings/floors fail CI. Sub-5 ms query/startup ratios and machine-
sensitive media/build timings are still recorded but are not used as regression
gates.

| Area | Metric | Budget |
| --- | --- | ---: |
| DB | 420k/1M FTS first page p95 | 250 ms |
| DB | aggregate first page p95 | 120 ms |
| DB | write-time cache counter p95 | 5 ms |
| DB | YQ candidate / notification / thread p95 | 100 ms |
| startup | cold ready / complete p95 | 250 ms / 1,500 ms |
| startup | cold API calls / DB writes | at most 4 / 4 |
| startup | warm API calls | 0 |
| read model | notification / thread SQL statements | at most 3 / 1 |
| stream | throughput / queue / drop / resync | at least 100 events/s / at most 512 / 0 / 0 |
| stream | DB lag p95 | 100 ms |
| retained UI state | entities / cache / timers | at most 20,000 / 512 / 1 |
| media | throughput / peak RSS delta | at least 5 MiB/s / at most 96 MiB |
| YQ | compile / evaluate 10k p95 | 10 ms / 400 ms |
| Rust | release binary | 120 MiB |
| package | AppImage, DMG, or ZIP | 300 MiB |

The frontend bundle artifact records every JS/CSS chunk plus raw, gzip, and Brotli
sizes. It enforces these ceilings:

| Bundle metric | Ceiling |
| --- | ---: |
| initial raw | 650 KiB |
| initial gzip | 210 KiB |
| initial Brotli | 185 KiB |
| total JavaScript raw | 2,100 KiB |
| largest deferred chunk raw | 1,000 KiB |

## Regression policy and artifacts

Pull requests download the most recent successful `main` artifact created on the
same runner class. Latencies at or above their noise floor fail above 1.5x, while
bundle, Rust binary, and package sizes fail above 1.15x. Clean compile time, package
assembly time, media throughput, media RSS, and sub-noise-floor timings are trend
metrics; scheduler and filesystem variation make a hard PR ratio unreliable. A
fixture-version change is reported as an incompatible baseline and starts a new
series instead of comparing unlike datasets.

Every matrix leg uploads machine-readable JSON, including before/after comparisons,
and writes it to the workflow summary. The build leg additionally uploads raw/gzip/
Brotli bundle metrics, clean compile duration, Rust binary size, AppImage size, and
actual YQ results. PERF changes must cite the relevant JSON and mention any other
metric that regressed even when it remained below the hard limit.

The earlier 2026-07-11 local 420k run used fixture `awayuki-v1` and remains useful
only as historical evidence: FTS p95 0.355 ms, aggregate p95 1.433 ms, counter p95
0.013 ms, 70.951 s seed time, and 626,966,528 database bytes. Fixture `awayuki-v2`
adds notifications and the reply chain, so CI intentionally does not ratio-compare
the two fixture versions.
