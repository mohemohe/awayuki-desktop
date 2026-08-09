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
- `src/application/desktop/release_security_smoke.rs`: loopback限定のpackage WebView fixture注入とstdout-only release security attestation。SQLite/OS store/side fileへ保存しない。
- `src/application/desktop/stream_bridge.rs`: provider eventをsource account付きWebView payloadへ変換し、UI先行配信とnotification side effect順序を管理するbounded bridge。
- `src/application/desktop/stream_subscription.rs`: columnからprovider streamへの購読計画。Unified Home / Public / Notificationは全signed-in source、Local / List / Hashtagは明示column account、KQは保存済みsource selectorでscopeする。
- `src/application/desktop/stream_notification.rs`: stream source accountに紐づくnotification保存とnative通知抑止判定。Active accountには依存しない。
- `src/application/window_persistence.rs`: native window eventを1 owner workerへ集約するdebounce/flush lifecycle。
- `src/ipc/`: command family別のTauri IPC入口と生成contract registry。
- `src/api/`: protocol共通client dispatch、server kind、共有HTTP policy。
- `src/{mastodon,misskey,bluesky}/`: protocol adapter、型、OAuth/auth、stream/polling。
- `src/services/`: timeline取得・batch保存・quote job、bounded streaming pipeline、YQ/KQ plan。
- `src/db/`: `sqlx::migrate!`、single writer / 500接続の共有lazy WAL reader pool、repository query。
- `src/auth/`: callback listener、SQLite-only credential lifecycle、in-memory session。
- `src/state/`: path、private file permission、logging、typed runtime settings。

HTTP clientはconnect/request/body/redirect上限を共有する。streamはbounded queue、sequence、
generation、resync markerを持つ。status pageは最大64件または40msの短いtransactionへbatch化する。
quote hydrationは初期pageから分離し、source accountとcanonical identityでdedupeする。表示paneは
consumer ownerとして参照され、pane closeは共有ownerを残したまま不要jobだけをcancelする。
Active account切替はquote/timeline source lifecycleを変更せず、実logoutだけがsource jobを停止する。
HTTP mutationはUI operation IDごとのcancellation tokenを持つ。account scope変更またはapp終了は
provider futureまでcancelするが、dispatch後の外部結果はuncertainとして扱い、自動retryしない。

## Persistent state

`awayuki.db` が唯一の永続状態であり、access token、Bluesky session/app password、設定、column、
status cache、startup sync / Bluesky polling checkpointを含む。OS Keychain、Credential Manager、
Secret Service、registryへ状態を保存しない。
DBを移動すればログイン状態を含めて復元できる。WAL/SHMは実行中のsidecarであり、停止後の移動は
checkpoint済みDBを対象にする。

Migrationは`migrations/NNN_*.sql`をappend-onlyで追加し、checksum付きtransactionとして適用する。
migration 032は通常status writeを占有した旧trigram/短文n-gram triggerを全て停止する。
旧FTS tableはmigration中の大規模な`DROP`でwriterを占有しないためdormant schemaとして残るが、
検索・全status writeからは参照しない。migration 034はaccount名にも同じ非同期ICU index、
coalescing queue、resumable backfillを追加し、account全件への前景ICU走査を除去する。
backfill中は最新status、pending status/account、account cursor直後を各256件の
`MATERIALIZED` windowへ固定してからconnection-local ICU4X関数で評価する。検索語は最大8語とし、
9語目以降を全statusのscalar fallbackへ戻さない。
頻出prefixも実candidate branchごとに10,000件でmaterializeし、最新10,000 statusは保存済みICU tokenを
再分節せず照合する。bounded sourceを`created_at / server_domain / id`順へ戻してからcandidateを切るため、
無順序FTS postingの上限が最新結果を押し出さない。
migration 035は旧ICU indexに無かったpunctuation/emoji segmentを非同期更新するため、既存postingsを
消去せずstatus/account backfill cursorだけをO(1)でresetする。起動migrationで全cacheをqueue化・再token化しない。
FTS5 shadow tableを直接変更する破損リスクや、parent tableの大規模`DROP`を
interactive writerへ戻すことを避けるため、物理容量回収は明示的なoffline maintenanceの残件とする。
数GBのDBを走査するfull integrity / foreign-key checkはwindow表示を止める起動経路では実行せず、
migration testと明示診断で行う。別DB backupやOS側の復旧状態は作らない。
status/account保存は同じ`awayuki.db`内の各coalescing index queueへkeyを追加するだけとし、ICU4XのNFKC・
case folding・dictionary segmentation、FTS更新、segment mergeはpost-readyの低優先度indexerが
行う。indexerはwriterを待たず`try_acquire`成功時だけlive queueを8件、backfillを32件まで処理し、cursor、queue、indexを同じ
portable DB内に保持する。pendingと移行gapの限定windowだけを共有WAL reader上のICU4X関数で補い、
10秒のquery budget内で検索するため、移行中だけn-gramやASCII限定`lower()`、無制限scalar scanへ意味を戻さない。
migration 033のtransaction-local control rowにより、明示的なcache全消去はstatus/account件数分の
queue/counter triggerを抑止し、現行・旧FTS payloadの消去とcounter 0設定を同じwriter transactionで行う。

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
List、Local、Hashtag等のprovider固有列は列自身の`account_acct`を使い、SQL、YQ、KQ、Searchはportable
SQLite全体を評価する。

## Trust boundaries

- federation HTMLはallowlist sanitizerを通し、linkはhttp/httpsだけをOS openerへ渡す。
- OAuth callbackはloopback listenerを1 ownerが保持し、state/session、PKCE、host/path/method、timeoutを検証する。
- sidecarはbackend lifecycle ownerが同一originのhttp/https navigationだけを許可し、別originのhttp/https navigationとpopupはOSの既定ブラウザへ渡す。download/local schemeは拒否する。
- downloadは共有timeout、body上限、stream write、`create_new`、cleanupを使う。
- main WebView CSPはdefault denyを基準にobject/base/form/frameとexternal connectを閉じる。
- remote image/mediaとinline styleの例外、threat model、削除条件は[`security/csp-policy.md`](security/csp-policy.md)を正本とし、CIで検査する。

## Build, test, and release

Rust 1.94.0、Bun 1.3.9、GitHub Actions commit SHA、Cargo/Bun lockfileを固定する。PR quality gateは
typecheck、ESLint、frontend test/build/bundle budget、fmt、clippy、Rust testを実行する。
Releaseとmanual production buildは同じreusable workflowとplatform scriptを使う。protected sourceを
検証し、clean runnerで3 OS packageの内容・起動smokeを行い、Arch packageはclean containerで
install / launch / uninstallする。SPDX SBOM、SHA-256 manifest、GitHub provenance/SBOM attestationも
同じpublish工程で生成する。詳細は`docs/release-runbook.md`を参照する。
macOSはsecretなしのbuild payloadと隔離sign/notarize jobを分け、署名鍵が存在するrunnerでは
repository codeや完成appを実行しない。
macOS packageは`Sparkle.framework`、Windows packageは`WinSparkle.dll`を同梱し、公開release
から生成したappcastを起動時に確認して更新を通知する。更新確認用OS preference / registry値は
Awayukiのユーザーデータではなく、機能状態・資格情報・設定は引き続き`awayuki.db`だけに保存する。
固定dataset、絶対予算、ローカル参考値は`docs/performance-baseline.md`を参照する。
