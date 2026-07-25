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
- migration 029 disables the character-index status triggers, and migration 032
  removes the remaining synchronous trigram/short n-gram status triggers. Status
  writes only coalesce a portable queue key; the replacement
  ICU4X status/account indexer uses `try_acquire`, at most 8 live or 32
  backfill rows per transaction, duty-cycle yield,
  and bounded idle-time FTS merges;
- local search uses indexed and recent pending rows immediately. While either
  resumable ICU index is incomplete, the connection-local ICU4X matcher only
  evaluates status/account `MATERIALIZED` windows capped at 256 rows with the
  same NFKC/case-fold/segment-prefix semantics, without reading dormant n-gram
  indexes or scanning the complete cache;
- full `foreign_key_check`/integrity scans are test or explicit-diagnostic work,
  never an unannounced first-window prerequisite.

`bun run startup:check`, the startup gate tests, the asynchronous indexer tests,
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
The replacement ICU indexer reads progress on a WAL reader and never waits in the
single-writer pool: it performs ICU normalization/segmentation only after an idle
probe, revalidates the durable queue generation inside a 64-row writer transaction,
and yields for three times the complete CPU/write/merge duration. The startup
boundary check rejects both an automatic retention call and synchronous FTS status
triggers. The preceding trigger-mitigation implementation recorded p95 2.397 ms
for 24 updates on the 420,000-status fixture (500 ms budget). Migration 032 changes
that fixture to queue-only writes, so its replacement value must be recorded by a
new `awayuki-v4-async-icu` run rather than attributed to the unmeasured code.

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
cargo run --locked --release --example performance-readers -- build/benchmark-420000.db build/reader-pool-benchmark.json
AWAYUKI_LOG_BENCHMARK_OUTPUT=build/logging-benchmark.json cargo test --locked --release state::logging::tests::logging_on_off_stream_benchmark -- --ignored --nocapture
bun run benchmark:timeline -- build/timeline-benchmark.json
bun run benchmark:bluesky -- build/bluesky-polling-benchmark.json
bun run benchmark:webview
bun run build
bun run bundle:check
```

`benchmark-db.mjs` executes every checked-in migration and records p50/p95, result
rows, statement count, database bytes, and `EXPLAIN QUERY PLAN` for FTS, aggregate
timeline, counter, YQ candidate, notification, and recursive thread queries. Bun
cannot register the Rust-only `awayuki_short` tokenizer, so migration 031 alone is
executed with that now-dormant tokenizer declaration replaced by built-in
`unicode61`; its cleanup, tables, and triggers are still executed and the
substitution is recorded in the artifact. ICU4X segmentation semantics are covered
by Rust fixtures; its production-path cost belongs to the still-pending Rust-backed
large-DB run and is not approximated in JavaScript. `benchmark-system.mjs` copies
that synthetic DB to a disposable working file and measures these fixed workloads:

- the ready milestone before a deterministic in-memory startup adapter performs four
  API-phase responses and four SQLite writes;
- warm startup with zero API requests;
- a same-database before/after warm-start comparison of API calls, write calls,
  ready time, and DB/WAL growth for legacy exhaustive versus checkpoint sync;
- the three-statement notification context and one-statement recursive thread read;
- SQL-prefiltered YQ candidate evaluation;
- ten seconds equivalent at 100 stream events/s, including queue, drop, resync,
  throughput, DB lag, and benchmark-process peak RSS delta;
- an eight-hour equivalent bounded-retention state model;
- a streaming 32 MiB media copy with throughput and peak RSS delta.

The YQ example separately runs the real compiled `yq` parser/evaluator against
10,000 fixed statuses. It compares per-status Context/regex-cache construction with
query-session reuse using a counting allocator, so timing and allocation regressions
fail together. It also stops at a 40-result page for low- and high-selectivity regex
queries and records scanned rows, allocations, allocated bytes, and page p95. This
keeps the portable JS fixture honest while measuring the same engine used by the
application.

The reader-pool example runs 2, 4, and CPU-sized SQLx pools in separate
processes against the 420,000-status fixture. It records acquire p95, query p95,
throughput, and peak RSS delta, selects the lowest-RSS count within 10% of the
best query p95, and fails when that selection differs from the runtime cap.

The logging fixture feeds 16,384 synthetic non-user stream records through the
same verbose sampling, redaction, bounded queue, and rotating file worker used by
the runtime. It compares logging disabled/enabled producer p95 and total throughput,
including worker drain, and requires zero drops. A local release run recorded
0.000042/0.001708 ms producer p95, 1,242,801 enabled events/s, and zero drops.

`benchmark-timeline.mts` drives the real frontend entity reducer with 12 columns,
10,000 input statuses per column, and 1,000 stream events in 50-event batches. It
requires every requested 10,000-status column to remain available, while enforcing
a 128 MiB fixture peak heap-delta ceiling and a 50 ms reducer-batch p95. This heap
budget measures the fixed 12-column fixture; it is not a retention cap. Explicit
pagination has no global status-count cap. React/WebView paint timing remains a
separate runtime metric.

`benchmark-bluesky-polling.mjs` fixes one signed-in Bluesky source, Unified Home,
a 40-status unchanged page, the default 30-second freshness interval, and one hour.
It reads the production interval/page constants and refuses an implementation that
silently stretches the user interval or loses the conditional checkpoint UPSERT.
Against audit revision `0bb67a2`, API calls remain 120 -> 120, while status DB
writes and WebView events fall from 4,800 each to zero after the revision baseline.
The API-call acceptance therefore remains explicitly false. The authenticated
AppView Home/Notification endpoints have no selective push equivalent: the
[AT Protocol firehose](https://atproto.com/specs/sync) streams repository changes,
and [Jetstream](https://github.com/bluesky-social/jetstream) can filter public
records by collection/repository DID but does not reproduce a viewer's AppView
timeline or notifications. A DID-filtered firehose or a longer poll interval is
not accepted as a substitute because either can omit Unified Timeline updates.

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
| startup | warm DB writes / DB+WAL growth | 0 / 0 bytes |
| read model | notification / thread SQL statements | at most 3 / 1 |
| stream | throughput / queue / drop / resync | at least 100 events/s / at most 512 / 0 / 0 |
| stream | DB lag p95 | 100 ms |
| stream | benchmark-process peak RSS delta | 64 MiB |
| Bluesky unchanged Home, 1h | API calls / DB writes / events | trend 120 / enforced 0 / enforced 0 |
| logging | enabled producer p95 / throughput / drops | below 5 ms / at least 100 events/s / 0 |
| retained UI state | entities / cache / timers | at most 20,000 / 512 / 1 |
| timeline reducer fixture | entities / column / peak heap delta | exactly 10,000 / exactly 10,000 / 128 MiB |
| timeline reducer | 50-event batch p95 | 50 ms |
| media | throughput / peak RSS delta | at least 5 MiB/s / at most 96 MiB |
| YQ | compile / evaluate 10k p95 | 10 ms / 400 ms |
| YQ | query-session allocations / bytes | below per-status Context baseline |
| Rust | release binary | 120 MiB |
| package | AppImage, DMG, or ZIP | 300 MiB |

The frontend bundle artifact records every JS/CSS chunk plus raw, gzip, and Brotli
sizes. It enforces these ceilings:

At runtime, the in-memory support bundle also reports the end of initial static
module evaluation, the interval after the last initial script response, DOM
interactive, first React commit, the second animation frame after the usable
snapshot commits, and WebView-provided JS heap usage/limit. These numbers contain
no account or content identifiers and are never persisted. The post-script interval
is a Resource Timing approximation, not an engine-level parser trace; before/after
cold-start comparisons must use repeated launches on the same host.

The same in-memory payload aggregates render commits into only three fixed
categories: timeline stream batches, timeline scrolling, and profile opening.
React Profiler supplies development/test samples; the production fixture records
the same fixed stream/profile categories at layout commit because production React
does not invoke the Profiler callback. Each
category retains at most 240 durations and reports integer commit/sample counts,
average, p95, and last duration. Pane, column, account, and status identifiers are
not included. A recorded performance artifact still requires driving those
scenarios in the packaged WebView; the synthetic reducer benchmark is not treated
as equivalent paint evidence.

`benchmark:webview` is a macOS packaged-WKWebView fixture. It builds an isolated
temporary `.app`, places `PORTABLE` beside that temporary executable, and therefore
creates only a disposable `awayuki.db` inside the temporary bundle directory. It
waits until the WebView is actually visible before resetting render samples; a
hidden/occluded WebView is not accepted as paint evidence. The 2026-07-12 local run
used 1,000 retained statuses and 100 events/s for ten seconds. Stream commit p95 was
10 ms and double-rAF frame p95 was 33 ms, scroll frame p95 was 66 ms, profile-open
commit/frame was 8/4 ms, and main-process peak RSS was 112,345,088 bytes. The window
was hidden for 31,192 ms before the fixture was raised; this wait is reported
separately and excluded from scenario samples. Module evaluation was 36 ms and the
first React commit was 54 ms. `firstInteractiveAfterVisibilityMs` subtracts only
that explicit visibility wait (101 ms in this run); it is not a substitute for the
same-host before/after cold-launch series required by PERF-11.

### PERF-11 same-host cold-start comparison

The startup-only mode was applied with the same instrumentation to audit baseline
commit `0bb67a2` and the current implementation. Both used a newly created portable
database beside an isolated temporary executable. WebView occlusion time is shown
separately and excluded from the interactive comparison.

| Metric | `0bb67a2` before | Current after | Change |
| --- | ---: | ---: | ---: |
| Initial JS raw | 550.82 kB | 469.24 kB | -14.81% |
| Initial JS gzip | 168.63 kB | 148.61 kB | -11.87% |
| Module evaluate approximation | 314 ms | 339 ms | +7.96% |
| First React commit | 321 ms | 346 ms | +7.79% |
| Interactive after visibility | 376 ms | 372 ms | -1.06% |
| Main-process peak RSS | 112,181,248 bytes | 121,044,992 bytes | +7.90% |

The package-only WebView security fixture is a separate 2.81 kB raw / 1.35 kB
gzip lazy chunk. Its runtime activation listener increased the initial JS from
469.03/148.50 kB to 469.24/148.61 kB raw/gzip (+0.04%/+0.07%); this remains in
the reported current value instead of being hidden as test-only overhead.

This is an honest mixed result: feature splitting materially reduced transferred and
parsed source size, while the single matched cold run did not improve module/commit
timing and increased process RSS. Those regressions remain visible as trend metrics;
they are not hidden by the passing 1,500 ms interactive ceiling. WKWebView returned
no JavaScript heap values, so process RSS is the memory comparison of record.

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

Fixture `awayuki-v3` added deterministic CJK short-search rows and the migration-028
Unicode character index. On the historical 2026-07-12 local 420,000-status run, `東京`
first-result p95 was 48.537 ms and the 40-row first-page p95 was 23.096 ms. Both
plans used the character FTS posting intersection and status primary-key lookup;
neither returned to a full `statuses` scan. Those values do not validate the ICU
replacement.

Fixture `awayuki-v4-async-icu` applies migration 032, times status updates as
queue-only writes, and preloads only deterministic ASCII ICU postings needed by the
SQLite query-plan case. It intentionally does not implement an ICU tokenizer in
JavaScript. A new 420,000-status artifact and a Rust-backed 930,000-status run are
still required before PERF-06 latency can be marked complete; v3/v4 database-size
and latency ratios are incompatible by construction.
