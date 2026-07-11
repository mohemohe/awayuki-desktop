# Current architecture

## Runtime boundary

Awayuki は1つの Tauri application processで動く。React WebViewは表示とinteractionを担当し、
network、SQLite、local file、sidecar WebView、OAuth listenerはRust command境界の内側で扱う。
remote API responseをfrontendから直接fetchしない。

Tauriの同期`setup`はstorage permission確認とSQLite pool openだけを行い、WebViewを先に表示する。
checksum migration、session復元、window state、streaming準備はbackground startup gateで段階実行し、
`app_snapshot`はready/errorを待つ。Frontendは`app-startup-progress`を購読し、数GBのcache更新中も
進捗画面と安全な再試行操作を表示する。remote startup syncとFTS backfillはready後に開始する。

## Backend

- `src/tauri_commands.rs`: 旧module pathを保つ薄いIPC facade。
- `src/application/desktop.rs`: runtime lifecycle、application use case、Tauri event bridgeの現行実装。
- `src/ipc/`: command family別のTauri IPC入口と生成contract registry。
- `src/api/`: protocol共通client dispatch、server kind、共有HTTP policy。
- `src/{mastodon,misskey,bluesky}/`: protocol adapter、型、OAuth/auth、stream/polling。
- `src/services/`: timeline取得・batch保存・quote job、bounded streaming pipeline、YQ plan。
- `src/db/`: `sqlx::migrate!`、single writer / capped reader pool、repository query。
- `src/auth/`: callback listener、SQLite-only credential lifecycle、in-memory session。
- `src/state/`: path、private file permission、logging、typed runtime settings。

HTTP clientはconnect/request/body/redirect上限を共有する。streamはbounded queue、sequence、
generation、resync markerを持つ。status pageは最大64件または40msの短いtransactionへbatch化する。

## Persistent state

`awayuki.db` が唯一の永続状態であり、access token、Bluesky session/app password、設定、column、
status cacheを含む。OS Keychain、Credential Manager、Secret Service、registryへ状態を保存しない。
DBを移動すればログイン状態を含めて復元できる。WAL/SHMは実行中のsidecarであり、停止後の移動は
checkpoint済みDBを対象にする。

Migrationは`migrations/NNN_*.sql`をappend-onlyで追加し、checksum付きtransactionとして適用する。
数GBのDBを走査するfull integrity / foreign-key checkはwindow表示を止める起動経路では実行せず、
migration testと明示診断で行う。別DB backupやOS側の復旧状態は作らない。
legacy FTS indexはSQLite内cursorを使う小さいtransactionへ分割し、chunk間でwriterを解放する。
完了前の検索はLIKE fallbackを使い、background化による検索漏れを許さない。

## Frontend

- `frontend/src/api/`: typed invoke wrapper。mutationはretryせず、allowlist readだけ1回retryする。
- `frontend/src/store/`: Zustand orchestration、domain reducer、bounded request scheduler。
- `frontend/src/domain/`: canonical timeline entity、mutation/confirmation/settings/sidecar lifecycle。
- `frontend/src/components/`: auth、workspace、timeline、compose、settings、mediaのfeature view。
- `frontend/src/utils/`: bounded LRU、media single-flight、format/security helper。

Timelineはentity map + ordered keyで正規化し、streamをmicro-batch更新する。Settingsはdraftと
key単位の直列保存、非同期requestはgeneration/AbortSignalで古い結果を破棄する。Settings、Login、
Media overlay、SQL editor、emoji catalogは必要時にdynamic importする。production graphにmockを含めない。

### Unified Timeline contract

Home、Public、Notificationはアカウント切替型の列ではなくUnified Timelineである。Homeは全ての
ActivityPub/Bluesky session、Publicは全てのActivityPub session、Notificationは全sessionの結果を
統合する。historical column rowに`account_acct`が残っていても、この3種のload、refresh、stream、
resyncを絞り込んではならない。BlueskyへActivityPub Publicを要求しない。

active accountはpost、boost、favourite、bookmark等の操作主体だけを表し、Timeline sourceを切り替えない。
List、Local、Hashtag等のprovider固有列は列自身の`account_acct`を使い、SQL、YQ、Searchはportable
SQLite全体を評価する。

## Trust boundaries

- federation HTMLはallowlist sanitizerを通し、linkはhttp/httpsだけをOS openerへ渡す。
- OAuth callbackはloopback listenerを1 ownerが保持し、state/session、PKCE、host/path/method、timeoutを検証する。
- sidecarはbackend lifecycle ownerがhttp/https navigationだけを許可し、popup/download/local schemeを拒否する。
- downloadは共有timeout、body上限、stream write、`create_new`、cleanupを使う。
- main WebView CSPはdefault denyを基準にobject/base/form/frameとexternal connectを閉じる。

## Build, test, and release

Rust 1.93.1、Bun 1.3.9、GitHub Actions commit SHA、Cargo/Bun lockfileを固定する。PR quality gateは
typecheck、ESLint、frontend test/build/bundle budget、fmt、clippy、Rust testを実行する。
Releaseとmanual production buildは同じreusable workflowとplatform scriptを使う。protected sourceを
検証し、clean runnerで3 OS packageの内容・起動smokeを行い、Arch packageはclean containerで
install / launch / uninstallする。SPDX SBOM、SHA-256 manifest、GitHub provenance/SBOM attestationも
同じpublish工程で生成する。詳細は`docs/release-runbook.md`を参照する。
macOSはsecretなしのbuild payloadと隔離sign/notarize jobを分け、署名鍵が存在するrunnerでは
repository codeや完成appを実行しない。
Sparkle / WinSparkleはOS側へ設定を保存するためbundleせず、更新は全OSでGitHub Releasesから手動で行う。
固定dataset、絶対予算、ローカル参考値は`docs/performance-baseline.md`を参照する。
