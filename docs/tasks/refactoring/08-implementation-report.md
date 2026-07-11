# 08. 実装状況レポート

- 更新日: 2026-07-11
- 対象: 本リファクタリング作業後の未コミット working tree
- 判定: `完了（ローカル検証済み）` は実装と対応する回帰テストがあり、統合後の macOS ローカル品質ゲートを完走した状態。`部分完了` は受入条件にコード、実測、外部 runner、追加分割の明確な残件がある状態。Windows/Linux、署名・公証、実 release runner の成功をローカル結果から推測しない
- ローカル結果: Rust fmt/check/Clippy `-D warnings`、Rust 173 tests、TypeScript、ESLint、Frontend 139 tests、production build、IPC/docs/portable/startup/release/bundle checks、420,000-status DB/system benchmark が成功

## 変更不能なプロダクト契約

- 機能状態と資格情報の永続化先は `awayuki.db` だけである。access token、Bluesky session / app password、設定、キャッシュを Keychain、Credential Manager、Secret Service、registry、その他の OS store へ保存しない。任意の診断ログは機能状態ではない。
- アプリ停止後に `awayuki.db` を移動すれば、ログイン状態を含む機能状態を移行できる。WAL / SHM は実行時 sidecar であり、別の永続正本ではない。
- 自動 DB backup、別 DB recovery file、OS store からの migration / recovery path は作らない。本プロダクトは未リリースであり、存在しない旧方式の復旧互換性も持たせない。
- 全 OS で updater framework を持たない。Sparkle、WinSparkle と OS 側 updater state は削除し、更新は GitHub Releases からの手動操作に統一する。
- Home、Public、Notification は Unified Timeline である。Home は全 ActivityPub/Bluesky session、Public は全 ActivityPub session、Notification は全 session を統合する。active account は post/boost/favourite 等の操作主体だけであり、Timeline source を切り替えない。

この契約の実装・検査証跡は [README](../../../README.md)、[SQLite-only ADR](../../adr/0001-sqlite-only-portable-state.md)、[`credential_store.rs`](../../../src/auth/credential_store.rs)、[`storage_security.rs`](../../../src/state/storage_security.rs)、[`check-portable-state.mjs`](../../../scripts/check-portable-state.mjs) にある。

## 起動を 132646 ms 停止させた FTS migration 事故

`VERSION=9.9.9 ./scripts/build-app-bundle.sh` で作った app を既存 DB から起動した際、schema migration の DB phase が `132646 ms` 継続し、その間ウィンドウが利用可能にならなかった。原因は [`020_create_status_search_fts.sql`](../../../migrations/020_create_status_search_fts.sql) が FTS schema 作成と既存 `statuses` 全件 backfill を同じ blocking migration で実行したことだった。

対策は次の境界で固定した。

- [`desktop.rs`](../../../src/application/desktop.rs) は WebView を先に成立させ、migration、session restore、service startup を background worker へ移し、`app-startup-progress` を UI へ通知する。[`App.tsx`](../../../frontend/src/components/App.tsx) と [`App.test.tsx`](../../../frontend/src/components/App.test.tsx) は初期 snapshot より先に progress event を購読する。
- [`pool.rs`](../../../src/db/pool.rs) は legacy DB へ旧 migration の一括 data backfill を再実行せず、schema/checksum/transaction 契約を維持したまま data phase を分離する。
- [`023_resumable_status_search_backfill.sql`](../../../migrations/023_resumable_status_search_backfill.sql) は cursor、処理件数、完了状態を同じ `awayuki.db` に保持する。[`search_backfill.rs`](../../../src/services/search_backfill.rs) は小さい transaction に分割し、中断後に resume し、進捗 event を発行する。不完全な間の検索は正確性を落とさない fallback を使う。
- [`check-startup-boundaries.mjs`](../../../scripts/check-startup-boundaries.mjs) と [quality workflow](../../../.github/workflows/quality.yml) は blocking setup と旧一括 backfill の再混入を拒否する。

schema 25 の再試験では window ready が 920 ms まで改善した一方、起動後の無制限 retention と writer transaction 内の FTS virtual-table count が単一 writer pool connection を占有し、通常 write が 30,371 ms、retry 後 154,038--154,330 ms で timeout した。runtime から無制限 retention の自動呼出を削除し、FTS 完了判定を WAL reader の status counter / document / `docsize` 比較へ移した。420,000-status fixture の24件更新は p95 2.397 ms になった。

同じ再試験で Custom SQL の固定 500 ms budget、YQ の固定 scan budget、DB persist 完了前の stream emit、Home/Public の legacy `account_acct` 誤解釈も確認した。SQL/YQ は専用 analytics reader と規模連動 budget へ移し、代表 YQ 候補抽出は実 DB read-only で 20.073 s 相当から 0.55 s、stream は WebView emit を永続化より先行、Home/Public/Notification は Unified 契約へ戻した。Bluesky Notification は初回を revision baseline のみにして過去通知を再通知せず、新規だけを設定間隔で emit する。

適用済み [`024_enforce_single_active_account.sql`](../../../migrations/024_enforce_single_active_account.sql) へ `IF NOT EXISTS` を後付けして checksum mismatch で起動不能にした回帰は、024を実DB履歴と同じ SHA-384 へ復元して解消した。履歴なしlegacy DBの同名indexは [`pool.rs`](../../../src/db/pool.rs) のschema-aware bootstrapだけで判定し、versioned migrationはappend-onlyに保つ。実DBの一時コピーでは024のchecksum検証後、[`026_decouple_global_columns_from_accounts.sql`](../../../migrations/026_decouple_global_columns_from_accounts.sql) のみを10 msで適用し、schema 26、Home/Public/Notification描画、Mastodon/Bluesky session復元を確認した。

## 正しさ・データ

| Task | 状態 | 実装・テスト・CI 証跡 |
| --- | --- | --- |
| SAFE-01 | 部分完了 | read と mutation の retry policy を [`tauri.ts`](../../../frontend/src/api/tauri.ts) で分離し、応答喪失・二重実行を [`tauri.test.ts`](../../../frontend/src/api/tauri.test.ts) と [`mutationLifecycle.test.ts`](../../../frontend/src/domain/mutationLifecycle.test.ts) で固定。 |
| DATA-01 | 完了（ローカル検証済み） | immutableなversion/checksum/transaction と schema-aware legacy repair を [`pool.rs`](../../../src/db/pool.rs)、正規化・差分同期・resumable FTS・global column scopeを [`021`](../../../migrations/021_normalize_status_identity_and_viewer_state.sql)、[`022`](../../../migrations/022_incremental_startup_sync_and_read_models.sql)、[`023`](../../../migrations/023_resumable_status_search_backfill.sql)、[`026`](../../../migrations/026_decouple_global_columns_from_accounts.sql) で実装。自動 backup / recovery file は作らない。 |
| DATA-02 | 部分完了 | login/logout/active account、column、status/timeline の複数更新を [`settings.rs`](../../../src/db/queries/settings.rs)、[`statuses.rs`](../../../src/db/queries/statuses.rs)、[`timeline.rs`](../../../src/db/queries/timeline.rs)、[`credential_store.rs`](../../../src/auth/credential_store.rs) の transaction / serialized lifecycle へ集約。 |
| CONF-01 | 部分完了 | backend の型付き setting query と frontend draft reducer を [`settings.rs`](../../../src/db/queries/settings.rs)、[`settingsDraft.ts`](../../../frontend/src/store/slices/settingsDraft.ts)、[`settingsDraft.test.ts`](../../../frontend/src/store/slices/settingsDraft.test.ts)、[`descriptors.test.ts`](../../../frontend/src/features/settings/descriptors.test.ts) で検証。 |
| ERR-01 | 部分完了 | stable code、safe message、request / operation ID を [`ipc/error.rs`](../../../src/ipc/error.rs)、[`ipcErrors.ts`](../../../frontend/src/api/ipcErrors.ts)、[`ipcErrors.test.ts`](../../../frontend/src/api/ipcErrors.test.ts) に実装。 |
| ASYNC-01 | 部分完了 | startup generation、retry、stream generation、UI resource cancellation を [`startup_gate.rs`](../../../src/application/startup_gate.rs)、[`streaming_service.rs`](../../../src/services/streaming_service.rs)、[`resources.ts`](../../../frontend/src/store/slices/resources.ts)、[`appStore.async.test.ts`](../../../frontend/src/store/appStore.async.test.ts) で固定。 |

## セキュリティ・信頼境界

| Task | 状態 | 実装・テスト・CI 証跡 |
| --- | --- | --- |
| CRED-01 | 部分完了 | Unix mode / umask と Windows current-user protected DACL、DB/WAL/SHM 一括補正、FAT/exFAT 非対応時 warning を [`storage_security.rs`](../../../src/state/storage_security.rs) に実装。macOS/Linux test は通過、Windows `cfg` integration test は追加済みだが Windows 実 runner の完走が残る。 |
| SEC-01 | 完了（ローカル検証済み） | HTML tag/attribute/class/scheme allowlist と custom emoji URL 制限を [`CustomEmoji.tsx`](../../../frontend/src/components/common/CustomEmoji.tsx) に集約。SVG、壊れ HTML、mention、hashtag、改行、危険 scheme を [`CustomEmoji.test.tsx`](../../../frontend/src/components/common/CustomEmoji.test.tsx) で検証。 |
| SEC-02 | 完了（ローカル検証済み） | 資格情報の SQLite-only persistence、rotation/logout serialization、DB 移動 test を [`credential_store.rs`](../../../src/auth/credential_store.rs) に実装し、[`check-portable-state.mjs`](../../../scripts/check-portable-state.mjs) と [ADR](../../adr/0001-sqlite-only-portable-state.md) で OS store / 別 backup を禁止。 |
| SEC-03 | 部分完了 | listener 所有、state 定数時間比較、timeout、cancel、single-use callback を [`callback_server.rs`](../../../src/auth/callback_server.rs)、S256 PKCE を [`oauth.rs`](../../../src/mastodon/oauth.rs) に実装し、再ログイン・期限切れ・encoding edge を unit test 化。 |
| SEC-04 | 部分完了 | begin/append/finish/cancel の chunk IPC と drop-path claim を [`mediaUpload.ts`](../../../frontend/src/api/mediaUpload.ts)、[`media_upload.rs`](../../../src/state/media_upload.rs)、[`ipc/contract.rs`](../../../src/ipc/contract.rs) に実装し、[`mediaUpload.test.ts`](../../../frontend/src/api/mediaUpload.test.ts) で cancel / failure を検証。 |
| SEC-05 | 完了（ローカル検証済み） | origin 固定、popup/download deny、style/lifecycle owner を [`sidecar_policy.rs`](../../../src/application/sidecar_policy.rs)、[`sidecar.ts`](../../../frontend/src/domain/sidecar.ts)、[`WorkspaceView.tsx`](../../../frontend/src/components/workspace/WorkspaceView.tsx)、[`sidecar.test.ts`](../../../frontend/src/domain/sidecar.test.ts) に実装。 |
| SEC-06 | 完了（ローカル検証済み） | Sparkle / WinSparkle dependency と updater module を削除し、[`Cargo.toml`](../../../Cargo.toml)、[README](../../../README.md)、[`check-portable-state.mjs`](../../../scripts/check-portable-state.mjs) で全 OS 手動更新を固定。 |
| SEC-07 | 完了（ローカル検証済み） | locked release build、download digest、source/artifact manifest、action SHA pin を [`build-appimage.sh`](../../../scripts/build-appimage.sh)、[`artifact-manifest.mjs`](../../../scripts/artifact-manifest.mjs)、[shared artifact workflow](../../../.github/workflows/build-artifacts.yml)、[`check-release-boundaries.mjs`](../../../scripts/check-release-boundaries.mjs) で検査。 |
| SEC-08 | 部分完了 | reviewed entitlement と audit を [`Entitlements.plist`](../../../resources/Entitlements.plist)、[`audit-macos-entitlements.sh`](../../../scripts/audit-macos-entitlements.sh)、[entitlement 文書](../../security/macos-entitlements.md) に固定。署名済み実 artifact の Apple notarization / stapler 検証は release runner 実行が残る。 |
| SEC-09 | 部分完了 | release DevTools feature を [`Cargo.toml`](../../../Cargo.toml) から除外し、package inspection を [`package-smoke.sh`](../../../scripts/package-smoke.sh) に追加。3 OS release artifact 上の shortcut/API smoke は外部 runner 完走が残る。 |
| SEC-10 | 部分完了 | deny-default CSP と capability 境界を [`tauri.conf.json`](../../../tauri.conf.json)、[`default.json`](../../../capabilities/default.json)、[`sidecar_policy.rs`](../../../src/application/sidecar_policy.rs) に実装。remote media / sidecar を含む3 OS packaged WebView 回帰は package smoke 完走が残る。 |

## バックエンド・データ設計

| Task | 状態 | 実装・テスト・CI 証跡 |
| --- | --- | --- |
| ARCH-01 | 部分完了 | 入口を [`tauri_commands.rs`](../../../src/tauri_commands.rs) の shim にし、adapter/port、application、query を [`api/ports.rs`](../../../src/api/ports.rs)、[`application/desktop.rs`](../../../src/application/desktop.rs)、[`db/queries`](../../../src/db/queries) へ分離。ただし `desktop.rs` は依然約9.7k行で、command family/use-case 単位の追加分割が残る。 |
| ARCH-02 | 部分完了 | Rust command/capability registry と生成 TypeScript を [`ipc/contract.rs`](../../../src/ipc/contract.rs)、[`generate-ipc-contract.rs`](../../../src/bin/generate-ipc-contract.rs)、[`generated/contract.ts`](../../../frontend/src/api/generated/contract.ts)、[`contract.test.ts`](../../../frontend/src/api/contract.test.ts) で同期。 |
| ROUTE-01 | 部分完了 | server-aware status identity、acting account、viewer state と Unified Home/Public/Notification を [`identity.rs`](../../../src/domain/identity.rs)、[`desktop.rs`](../../../src/application/desktop.rs)、[`appStore.timeline.test.ts`](../../../frontend/src/store/appStore.timeline.test.ts) で固定。同一domain複数sessionの一部read経路は残件。 |
| ARCH-03 | 完了（ローカル検証済み） | protocol-neutral DTO、adapter error、capability snapshot、port を [`protocol.rs`](../../../src/domain/protocol.rs)、[`adapter_error.rs`](../../../src/domain/adapter_error.rs)、[`capability.rs`](../../../src/domain/capability.rs)、[`api/ports.rs`](../../../src/api/ports.rs) に導入。 |
| DATA-03 | 完了（ローカル検証済み） | canonical composite identity、viewer state、status-tag relation、orphan cleanup / FK を [`021`](../../../migrations/021_normalize_status_identity_and_viewer_state.sql)、[`models.rs`](../../../src/db/models.rs)、[`statuses.rs`](../../../src/db/queries/statuses.rs)、[`tags.rs`](../../../src/db/queries/tags.rs) へ反映。 |
| SQL-01 | 部分完了 | SQLite authorizer、progress/cancel budget、result/payload cap、plan inspection を [`custom_timeline.rs`](../../../src/db/queries/custom_timeline.rs)、[`SqlEditor.tsx`](../../../frontend/src/components/common/SqlEditor.tsx)、生成 IPC contract に実装。 |
| AUTH-01 | 完了（ローカル検証済み） | Bluesky refresh single-flight、成功/失敗共有、auth generation、rotation persistence と logout/re-login の stale write rejection を [`bluesky/client.rs`](../../../src/bluesky/client.rs) と [`credential_store.rs`](../../../src/auth/credential_store.rs) の concurrency test で検証。 |
| DEAD-01 | 完了（ローカル検証済み） | 旧 GPUI state/assets と repository-wide `dead_code` allow を削除し、[`Cargo.toml`](../../../Cargo.toml) と [architecture](../../architecture.md) を現行化。`cargo clippy --all-targets --locked -- -D warnings` は統合後ゼロ警告。 |

## フロントエンド設計

| Task | 状態 | 実装・テスト・CI 証跡 |
| --- | --- | --- |
| FE-01 | 部分完了 | window-first background initialization、progress/error/retry state を [`startup_gate.rs`](../../../src/application/startup_gate.rs)、[`desktop.rs`](../../../src/application/desktop.rs)、[`App.tsx`](../../../frontend/src/components/App.tsx)、[`App.test.tsx`](../../../frontend/src/components/App.test.tsx) に実装。 |
| FE-02 | 完了（ローカル検証済み） | normalized status entity と mutation reconciliation を [`timelineEntities.ts`](../../../frontend/src/domain/timelineEntities.ts)、[`timelineEntities.test.ts`](../../../frontend/src/domain/timelineEntities.test.ts)、[`appStore.timeline.test.ts`](../../../frontend/src/store/appStore.timeline.test.ts) で固定。 |
| FE-03 | 完了（ローカル検証済み） | settings draft、serialized mutation、resource-local error を [`settingsMutations.ts`](../../../frontend/src/domain/settingsMutations.ts)、[`settingsDraft.ts`](../../../frontend/src/store/slices/settingsDraft.ts) と各 test に実装。 |
| FE-04 | 完了（ローカル検証済み） | sidecar operation generation、close cleanup、navigation/reload/style ownership を [`sidecar.ts`](../../../frontend/src/domain/sidecar.ts) と [`WorkspaceView.tsx`](../../../frontend/src/components/workspace/WorkspaceView.tsx) に集約。 |
| FE-05 | 完了（ローカル検証済み） | instance login と Bluesky app-password login の submit intent / validation を [`LoginView.tsx`](../../../frontend/src/components/auth/LoginView.tsx) と [`LoginView.test.tsx`](../../../frontend/src/components/auth/LoginView.test.tsx) で分離。 |
| UI-01 | 部分完了 | mutation phase、uncertain result、confirmation queue を [`mutationLifecycle.ts`](../../../frontend/src/domain/mutationLifecycle.ts)、[`confirmationQueue.ts`](../../../frontend/src/domain/confirmationQueue.ts)、[`ConfirmationDialog.tsx`](../../../frontend/src/components/common/ConfirmationDialog.tsx) と test に共通化。 |
| FE-06 | 部分完了 | compose/panes/resources/session/settings の reducer slice と test を [`store/slices`](../../../frontend/src/store/slices) に追加。ただし [`appStore.ts`](../../../frontend/src/store/appStore.ts) はなお大きく、action orchestration の追加分離が残る。 |
| FE-07 | 部分完了 | Compose/Settings/Timeline を controller/view/feature へ分離し、[`compose`](../../../frontend/src/features/compose)、[`settings`](../../../frontend/src/features/settings)、[`timeline`](../../../frontend/src/features/timeline) と layout fixture test を追加。 |
| FE-08 | 完了（ローカル検証済み） | timeline type の label、capability、編集情報を [`timelineDescriptors.ts`](../../../frontend/src/domain/timelineDescriptors.ts) に集約し、[`timelineDescriptors.test.ts`](../../../frontend/src/domain/timelineDescriptors.test.ts) で exhaustiveness を検証。 |
| FE-09 | 部分完了 | semantic i18n key と日英辞書の型検査を [`i18n.ts`](../../../frontend/src/i18n.ts)、[`i18n.test.ts`](../../../frontend/src/i18n.test.ts)、[`useAppLocale.ts`](../../../frontend/src/hooks/useAppLocale.ts) に実装。 |
| FE-10 | 完了（ローカル検証済み） | strict mock command exhaustiveness と production marker scan を [`mock.ts`](../../../frontend/src/api/mock.ts)、[`contract.test.ts`](../../../frontend/src/api/contract.test.ts)、[`check-bundle-budget.mjs`](../../../scripts/check-bundle-budget.mjs) に実装。 |
| FE-11 | 完了（ローカル検証済み） | Dialog/Listbox/Menu/Tabs と focus utility を [`components/primitives`](../../../frontend/src/components/primitives)、keyboard/focus regression を [`primitives.test.tsx`](../../../frontend/src/components/primitives/primitives.test.tsx) に共通化。 |

## パフォーマンス

| Task | 状態 | 実装・テスト・CI 証跡 |
| --- | --- | --- |
| PERF-01 | 部分完了 | high-water/cursor、warm skip、full reconciliation resume、checkpoint を [`startup_sync.rs`](../../../src/services/startup_sync.rs) と [`022`](../../../migrations/022_incremental_startup_sync_and_read_models.sql) に実装。無制限 retention は writer starvation を起こしたため runtime 自動実行を削除し、bounded maintenance は残件。 |
| PERF-02 | 部分完了 | 初期 timeline 表示と missing quote hydration を [`timeline_service.rs`](../../../src/services/timeline_service.rs) と background scheduling へ分離し、batch/partial failure test を追加。 |
| PERF-03 | 部分完了 | raw/persistence queue、identity coalescing、drop/resync、generation clock と emit-before-persistence を [`streaming_service.rs`](../../../src/services/streaming_service.rs) に実装。notification side-effect handoffのbounded化と接続multiplexは残件。 |
| PERF-04 | 部分完了 | Bluesky status/notification のrevision diff、重複抑止、設定間隔維持を [`bluesky_fetch.rs`](../../../src/state/bluesky_fetch.rs)、[`bluesky/streaming.rs`](../../../src/bluesky/streaming.rs) に実装。pollごとの最新page API callとprocess-local revision stateは残る。 |
| PERF-05 | 部分完了 | timeline/status/account/tag write を transaction batch 化し、statement metrics を [`timeline_service.rs`](../../../src/services/timeline_service.rs)、[`statuses.rs`](../../../src/db/queries/statuses.rs) に実装。 |
| PERF-06 | 部分完了 | trigram FTS、keyset/capped result、reader-side completion probe、legacy resumable backfill、bounded merge policy と exact fallback を [`020`](../../../migrations/020_create_status_search_fts.sql)、[`023`](../../../migrations/023_resumable_status_search_backfill.sql)、[`025`](../../../migrations/025_bound_fts_merge_work.sql)、[`search_backfill.rs`](../../../src/services/search_backfill.rs) に実装。短語を含む旧LIKE fallbackの完全撤去は未達。 |
| PERF-07 | 部分完了 | YQ evaluator/regex compile cache、keyset、規模連動budget、HTML-safe contains SQL prefilterを [`yq_filter.rs`](../../../src/services/yq_filter.rs) に実装。実DB代表候補は20.073秒相当から0.55秒へ短縮したが、backend cancel、UI slow-query表示、allocation計測は残件。 |
| PERF-08 | 完了（ローカル検証済み） | notification context 3 statement、recursive thread 1 statement、limit-first aggregate、write-time counter を [`read_models.rs`](../../../src/db/queries/read_models.rs)、[`022`](../../../migrations/022_incremental_startup_sync_and_read_models.sql) と benchmark に実装。 |
| PERF-09 | 部分完了 | bounded entity retention、O(n) merge、micro-batch/coalescing、anchor preservation を [`appStore.ts`](../../../frontend/src/store/appStore.ts) と [`appStore.timeline.test.ts`](../../../frontend/src/store/appStore.timeline.test.ts) に実装。 |
| PERF-10 | 部分完了 | weighted LRU と media retry single-flight/coordinator を [`lru.ts`](../../../frontend/src/utils/lru.ts)、[`mediaRetryCoordinator.ts`](../../../frontend/src/utils/mediaRetryCoordinator.ts) と各 test に実装。 |
| PERF-11 | 部分完了 | feature chunk、raw/gzip/Brotli budget と mock marker scan を [`vite.config.ts`](../../../vite.config.ts)、[`check-bundle-budget.mjs`](../../../scripts/check-bundle-budget.mjs)、[performance workflow](../../../.github/workflows/performance.yml) に実装。 |
| PERF-12 | 部分完了 | shared reqwest policy、timeout、response size cap、streaming/cancel を [`api/http.rs`](../../../src/api/http.rs)、provider clients、[`mediaUpload.ts`](../../../frontend/src/api/mediaUpload.ts) に統一。 |
| PERF-13 | 部分完了 | priority/concurrency scheduler、render/batch metrics を [`requestScheduler.ts`](../../../frontend/src/utils/requestScheduler.ts)、[`renderMetrics.ts`](../../../frontend/src/utils/renderMetrics.ts) と各 test、[`benchmark-system.mjs`](../../../scripts/benchmark-system.mjs) に実装。 |
| PERF-14 | 部分完了 | bounded rotation、redaction、console forwarding control を [`logging.rs`](../../../src/state/logging.rs)、[`consoleLogging.ts`](../../../frontend/src/utils/consoleLogging.ts)、[`observability.rs`](../../../src/observability.rs) に実装。logging on/off の実 throughput 比較結果の記録が残る。 |
| PERF-15 | 部分完了 | SQLite reader を2〜4 connectionへ clampし、window state を watch channel + 1 resettable timer へ coalesceした証跡は [`pool.rs`](../../../src/db/pool.rs)、[`desktop.rs`](../../../src/application/desktop.rs) にある。変更前後の実 task/connection/RSS比較の記録が残る。 |

## 品質・運用・配布

| Task | 状態 | 実装・テスト・CI 証跡 |
| --- | --- | --- |
| QUAL-01 | 完了（ローカル検証済み） | fmt、Clippy `-D warnings`、Rust/Frontend test、typecheck、lint、build、contract/docs/portable/startup/release check を [quality workflow](../../../.github/workflows/quality.yml) と [`package.json`](../../../package.json) に必須化。 |
| QUAL-02 | 部分完了 | migration、OAuth、sanitizer、IPC retry、routing、async generation、accessibility、performance fixture を Rust unit test と [`frontend/src`](../../../frontend/src) の Vitest 群へ追加。 |
| OPS-01 | 部分完了 | UI→IPC→API→DB operation ID、phase duration、queue/sync/query metrics、redacted support snapshot を [`observability.rs`](../../../src/observability.rs)、[`api/observability.ts`](../../../frontend/src/api/observability.ts)、[`diagnostics.ts`](../../../frontend/src/api/diagnostics.ts) に実装。 |
| REL-01 | 完了（ローカル検証済み） | reusable 3 OS build、deterministic source archive、manifest digest/size/version、appcast invariant を [artifact workflow](../../../.github/workflows/build-artifacts.yml)、[`artifact-manifest.mjs`](../../../scripts/artifact-manifest.mjs)、[appcast workflow](../../../.github/workflows/update-appcast.yml) に実装。 |
| REL-02 | 部分完了 | macOS unsigned build→protected signing/notarization→secretless smoke を [artifact workflow](../../../.github/workflows/build-artifacts.yml) で job 分離し、[`check-release-boundaries.mjs`](../../../scripts/check-release-boundaries.mjs) で ref/権限境界を検査。protected environment の実 release run が残る。 |
| PKG-01 | 部分完了 | DMG/ZIP/AppImage の install/launch/content/uninstall smoke を [`package-smoke.sh`](../../../scripts/package-smoke.sh)、[Arch smoke](../../../.github/workflows/arch-package-smoke.yml)、[artifact workflow](../../../.github/workflows/build-artifacts.yml) に実装。3 OS hosted runner の完走結果が残る。 |
| DEP-01 | 完了（ローカル検証済み） | Rust/Bun/toolchain、locked git revision、feature inventory、deny/advisory/update policy を [`rust-toolchain.toml`](../../../rust-toolchain.toml)、[`Cargo.toml`](../../../Cargo.toml)、[`deny.toml`](../../../deny.toml)、[dependency policy](../../security/dependency-policy.md)、[audit workflow](../../../.github/workflows/dependency-audit.yml) に固定。 |
| DOC-01 | 部分完了 | 現行構成、portable contract、release/incident/security 運用を [architecture](../../architecture.md)、[ADR index](../../adr/README.md)、[release runbook](../../release-runbook.md)、[README](../../../README.md) に更新し、link check を追加。 |
| BUDGET-01 | 部分完了 | 20k/420k/1M DB、startup/read-model/stream/media/YQ/bundle/package budget、main比較を [`benchmark-db.mjs`](../../../scripts/benchmark-db.mjs)、[`benchmark-system.mjs`](../../../scripts/benchmark-system.mjs)、[`compare-performance.mjs`](../../../scripts/compare-performance.mjs)、[baseline](../../performance-baseline.md)、[performance workflow](../../../.github/workflows/performance.yml) に実装。 |

## 部分完了の主要残件

- 正しさ・data: provider idempotency key / operation ledger、全transaction境界のfault injection、setting schema versionとRust/TypeScript共通生成元が未実装。
- error・async・SQL: application/adapterに`Result<_, String>`と文字列分類が残り、frontend AbortSignalをIPC/HTTPのCancellationTokenへ接続していない。Custom SQLのauthorizer/budgetは動くが、ユーザー操作によるcancelは未実装。
- OAuth・media: PKCE非対応server互換test、binary chunkのJSON数値配列化解消、download progress/cancelとredirect先再検証が未完了。
- IPC・startup・mutation UI: IPC生成物はDTO型そのものを生成せず、startup listener登録失敗の明示UI、全mutationの共通lifecycle化も残る。
- routing: Unified/global SQLite列のaccount owner metadataはschema 26で除去した。一方、同一domain複数sessionで`session_for_domain`を使うprofile/AIR/threadのread source、AIRの明示account routing、active actor別viewer stateの即時再評価は未確定。
- frontend architecture・i18n: controllerの追加分割、原文文字列keyのsemantic i18n移行、user-facing `String(error)`の撤去が未完了。
- performance: 各表の実装は存在するが、quoteの完全非同期DTO、全段bounded queue、server/account接続multiplex、persistent Bluesky cursor、1,000件write実測、FTS完了後LIKE撤去、end-to-end cancel、実React heap/frame/parse/interactive計測、translation priority queue、`api/detect.rs`の共有HTTP policy化が残る。deterministic modelを実アプリ計測とは扱わない。
- 品質・observability・docs・budget: P0/P1 failure-mode test、全commandをAPI/DB commitまで結ぶoperation ID、実装とarchitecture記述の継続同期、実app startup/React profiler/heapのbudgetが残る。

## 最終完了ゲート

`完了（ローカル検証済み）` はmacOS上の統合gateを通した判定であり、3 OS package、署名、公証、protected release環境の成功を含まない。`部分完了` は上記残件、各行の実計測、Windows/macOS/Linux package runnerまたは署名環境の証跡が揃うまで完了扱いにしない。
