# CLAUDE.md

Awayuki は Tauri 2 / Rust backend と React / TypeScript frontend で構成する、Mastodon、
Paon、Misskey、Bluesky 対応の3 OS desktop clientです。

作業前に [docs/architecture.md](docs/architecture.md) を読み、次の品質ゲートを実行してください。

```bash
bun install --frozen-lockfile
bun run typecheck
bun run lint
bun run test
bun run build
bun run bundle:check
bun run docs:check
bun run portable-state:check
bun run startup:check
bun run release:check
bun run ipc:check
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

重要な契約:

- `awayuki.db` が資格情報を含む唯一の永続状態です。OS credential store、registry、別file、自動migration backupへ状態を分離しません。
- Tauriの同期`setup`で全件migration / integrity checkを実行しません。windowと進捗UIを先に表示し、大規模cache処理はSQLite内cursorを使うbackground jobへ分割します。
- mutation IPC は自動retryしません。再試行可能なのは明示したread commandだけです。
- migrationは`migrations/`と`sqlx::migrate!`を正本とし、managed checksum history導入後に適用済みmigrationを変更しません。
- protocol固有IDだけでstatusを識別せず、server/protocolを含むcanonical identityを使います。
- remote HTML、sidecar、OAuth callback、download pathは信頼境界として検証します。
- release / manual buildは固定toolchain、lockfile、署名environment、artifact manifestを共有します。

portable stateの判断は [docs/adr/0001-sqlite-only-portable-state.md](docs/adr/0001-sqlite-only-portable-state.md)、
配布手順は [docs/release-runbook.md](docs/release-runbook.md) を参照してください。
