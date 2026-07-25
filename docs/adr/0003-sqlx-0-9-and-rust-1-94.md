# ADR-0003: SQLx 0.9 and Rust 1.94

- Status: Accepted
- Date: 2026-07-26

## Context

SQLx 0.9 requires Rust 1.94 and changes the runtime query API. `QueryBuilder` no longer carries a
query lifetime, migrations take an explicit history-table name, and dynamically constructed SQL
must be wrapped in `AssertSqlSafe` after an injection audit. SHA-256 formatting also changed with
the accompanying `sha2` update.

Awayuki intentionally constructs SQL for bounded search, YQ, custom timelines, schema inspection,
and tests. These call sites cannot all be replaced by static statements, but their dynamic
fragments come from fixed enums, validated identifiers, generated predicates, numeric limits, or
the custom-timeline sandbox. User values continue to use bind parameters.

## Decision

- Pin the local and CI toolchain to Rust 1.94.0.
- Upgrade SQLx to 0.9 and keep SQLite as its only database feature.
- Keep `libsqlite3-sys` pinned to SQLx's exact SQLite FFI version.
- Mark dynamic statements with `AssertSqlSafe` only at audited boundaries. Custom timeline SQL
  remains behind validation, the read-only SQLite authorizer, cancellation, VM/time budgets, and
  result limits.
- Let Dependabot dependency-only pull requests skip the ADR-diff gate. Human architecture changes
  and the implementation of a major dependency migration still require an ADR.

## Rejected alternatives

- Keeping Rust 1.93 and SQLx 0.8 would block the dependency update rather than resolve it.
- Enabling SQLx default features would restore unused server-database drivers and increase the
  dependency and audit surface.
- Treating arbitrary runtime strings as safe in a shared helper would hide the individual audit
  boundary and make future injection review harder.

## Data and compatibility

This decision does not change the database location, SQLite schema, WAL behavior, or account and
timeline semantics. All persistent state remains in the portable SQLite database and moving that
file remains sufficient to move the application state. Unified Home/Public/Notification timelines
and the active-account-as-action-actor contract are unchanged.

The product is pre-release, so no downgrade or recovery path is provided. Existing SQLite files do
not require a data migration for this dependency update.

## Security

Dynamic identifiers are fixed or validated before interpolation. Remote and user values remain
bound parameters except custom timeline SQL, which is explicitly user-authored and executed only
inside the existing read-only sandbox. `AssertSqlSafe` is not exposed as a general-purpose helper.

## Verification

- Frontend typecheck, lint, tests, production build, and bundle budget.
- Rust formatting, clippy with warnings denied, and all-target tests.
- Release compilation and the SQLite reader-pool performance example.
- Dependency audit and feature-boundary checks.
