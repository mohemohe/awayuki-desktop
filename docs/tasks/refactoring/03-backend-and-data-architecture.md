# 03. バックエンドとデータ設計

## ARCH-01: `tauri_commands.rs` を command / application / repository 境界へ分離する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | モジュール設計、依存方向 |
| 依存 | SAFE-01、DATA-01、DATA-02、QUAL-02 |

### 問題と根拠

[`src/tauri_commands.rs`](../../../src/tauri_commands.rs#L1) は 8,201 行あり、約 40 の command 登録（[`register`](../../../src/tauri_commands.rs#L1088)）、認証、sidecar、設定、SQL、同期、stream supervisor、DTO 変換、platform UI、テストを同居させる。command が DB query と外部 API を直接組み合わせるため、トランザクション、認可、acting account、エラー変換の境界が handler ごとに異なる。

### 方針

依存方向を `Tauri command -> application use case -> domain port -> adapter/repository` に固定する。

- `ipc/{auth,account,timeline,status,compose,settings,media,sidecar,maintenance}.rs`: 引数検証と DTO 変換のみ。
- `application/`: logout、post、refresh、save columns 等のユースケース、トランザクション所有者。
- `domain/`: protocol 非依存 identity、capability、error、event。
- `adapters/{mastodon,misskey,bluesky,sqlite}`: 外部仕様と保存形式。
- `streaming/`: 接続 supervisor、bounded queue、resync。

単なるファイル移動で巨大な shared context を作らない。まず既存挙動の characterization test を置き、ユースケース単位で移す。

### 受け入れ条件

- [x] Tauri handler は直接 SQL や protocol client を組み立てず、1 つの use case を呼ぶ。
- [x] transaction と外部 API の順序が use case のテストで確認できる。
- [x] module 間の循環依存がなく、domain が Tauri / sqlx / protocol DTO に依存しない。
- [x] `tauri_commands.rs` は登録と薄い facade だけになり、全既存 contract test が通る。

Tauri入口は`ipc/*`へ分割済みで、account lifecycle/read/mutation・profile/timeline query・account DTOを[`application/account.rs`](../../../src/application/account.rs)、compose upload/suggestion/emojiとそのDTOを[`application/compose.rs`](../../../src/application/compose.rs)、portable DB maintenance/診断/in-memory support bundleとそのDTOを[`application/maintenance.rs`](../../../src/application/maintenance.rs)、media downloadを[`application/media.rs`](../../../src/application/media.rs)、Unified notification cache readを[`application/notification.rs`](../../../src/application/notification.rs)、認証familyを[`application/auth.rs`](../../../src/application/auth.rs)、設定commit/runtime effectを[`application/preferences.rs`](../../../src/application/preferences.rs)、frontend-ready handshake・再試行・snapshot待機・background migration/session restoreを[`application/runtime.rs`](../../../src/application/runtime.rs)、型付き設定schema/snapshotを[`application/settings.rs`](../../../src/application/settings.rs)、post/action/vote/edit/deleteを[`application/status.rs`](../../../src/application/status.rs)、Timeline load/load-more/refresh・local種別選択・AIR・threadを[`application/timeline.rs`](../../../src/application/timeline.rs)、Timeline/nested poll/emoji/page/viewer-state DTOとAPI/SQLite Status/Notification純粋変換を[`application/timeline_view.rs`](../../../src/application/timeline_view.rs)、RuntimeState非依存の翻訳use caseを[`application/translation.rs`](../../../src/application/translation.rs)へ分離した。handlerはrequestを1 use caseへ渡すだけで、raw upload bodyの抽出はIPC層、downloadとlogin orchestrationはapplication層、cancellation registryとtemp uploadはstate層、資格情報transactionは既存SQLite application境界に置く。認証はprovider側の認証・資格情報確認が成功してからのみportable SQLite transactionを開始し、provider失敗時はtransactionを呼ばない順序をcharacterization testで固定した。設定はportable SQLite commit成功後だけlogging/stream effectを適用し、commit失敗時はeffectを実行しない。起動はWebViewのprogress listener登録後のhandshakeでのみbackground workerを開始し、重いportable SQLite migrationを同期Tauri setupへ戻さない。session restoreは`awayuki.db`から全accountを読み、Active accountをactorとして設定した後も全signed-in sessionからUnified streamを再開する。maintenanceは既存DTOを維持し、support bundleをfileへ保存せずメモリ上で返す。Active account切替はstreamを再構成せず操作元だけを変え、logoutだけがsigned-in source集合変更としてUnified streamを再構成する。status mutationは`acting_account_acct`とcanonical status identityを別々に検証し、viewer state/timeline membershipをacting account単位で保存する。compose uploadはbegin時のacting accountをresourceへ固定し、finish時のActive accountを参照しない。Home/Public/Notificationの分類は[`TimelineType::is_unified`](../../../src/services/timeline_service.rs)へ一元化し、legacy `account_acct`が残っていてもload/refreshをActive accountで狭めず全signed-in sourceのaggregateとして扱う。Notification cache readのapplication APIはaccount selector自体を持たず、全sourceのnotification contextを読む。local読取りもHome/Publicはaggregate query、Notificationはこのglobal notification queryを先に選び、明示source sessionを要求するのはLocal/List/Hashtag等のaccount-bound種別だけである。account-bound timeline entryとbookmark/favouriteのSQL・canonical URI重複排除・viewer account filterはIPC非依存の[`timeline_views.rs`](../../../src/db/queries/timeline_views.rs)へ、FTS/LIKE query・keyset cursor・filter bindは[`search.rs`](../../../src/db/queries/search.rs)へ、YQ compile/prefilter/keyset/account hydration/evaluator/budgetは[`yq_timeline.rs`](../../../src/services/yq_timeline.rs)へ移した。SQLite status/accountの一括取得、reblog/quote hydration、source/viewer state適用は[`timeline_hydration.rs`](../../../src/application/timeline_hydration.rs)へ分離した。profile/pinned timeline/AIR/threadは明示`source_acct`を優先し、同一domainに複数sessionがあってsourceが未指定ならActive accountへ暗黙fallbackせずSQLite cacheだけを使う。設定schema検証・破損値のSQLite内退避・snapshot読取りはdesktop/Tauriへ依存しない。`desktop.rs`は5,429行となり、account/composeを含む各application familyが自身のDTOを所有する。残るplatform runtime・stream supervisor等は責務単位で継続分離する。

provider streamの購読計画は追加で
[`stream_subscription.rs`](../../../src/application/desktop/stream_subscription.rs)へ
分離した。Unified Home/Public/Notificationは全signed-in sourceを購読し、
Local/List/Hashtagだけが明示column accountへ限定される。Active accountはこの
計画の入力ではなく、操作元actorだけを選ぶ。
stream notificationのSQLite保存とnative通知抑止判定も
[`stream_notification.rs`](../../../src/application/desktop/stream_notification.rs)へ
分離し、eventが保持するsource accountを保存先viewer contextとして使う。
provider eventのUI payload変換、generation/sequence、UI先行配信とnotification
side effectのbounded handoffは
[`stream_bridge.rs`](../../../src/application/desktop/stream_bridge.rs)へ分離した。
package限定のCSP attestationとloopback WebView fixture注入も
[`release_security_smoke.rs`](../../../src/application/desktop/release_security_smoke.rs)へ
分離し、通常runtimeから環境変数gate・URL検証・stdout-only reportを隔離した。

## ARCH-02: Rust を正本に型付き IPC command と DTO を生成する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | IPC 契約、型安全性 |
| 依存 | SAFE-01、ERR-01 |

### 問題と根拠

フロントエンドの [`invokeCommand<T>`](../../../frontend/src/api/tauri.ts#L8) は `command: string` と `Record<string, unknown>` を受け、`T` は呼び出し側の assertion に過ぎない。Rust DTO と [`frontend/src/types/app.ts`](../../../frontend/src/types/app.ts#L1) の型、command 名、serde casing、nullability、timeline type が手作業で二重管理される。mock には 3 つ目の契約がある。

### 方針

- Specta / ts-rs 等、または専用 build step で command map、args/result、enum、error を生成する。
- command metadata に SAFE-01 の副作用分類、timeout、cancel、capability を含める。
- 外部／DB から来る値は生成型だけを信じず、IPC 境界で version と runtime validation を行う。
- 旧 command は段階的に generated client へ移し、完了後に string API を削除する。

### 受け入れ条件

- [x] Rust の DTO / command 変更が TypeScript compile または contract test を失敗させる。
- [x] enum に未知値が来たときの forward-compatible な扱いが定義されている。
- [x] mock も同じ command map を実装し、未実装 command は compile error または明示例外になる。
- [x] code generation の差分が CI で検証される。

Rustの[`ipc/dto.rs`](../../../src/ipc/dto.rs)に、Rust field型・serde名・optionality・TypeScript field型を単一定義から展開するDTO macroを追加した。generatorはDTO schema、TypeScript type、command別args/result mapを出力し、全JSON commandとraw media chunkをtyped registryへ登録する。status/post mutationはcanonical identityまたは投稿内容とacting accountを別の必須fieldとして保持し、raw media chunkはJSON配列へ戻さず生成済みraw result型と`Uint8Array` IPCを使う。Frontendのgeneric `invokeCommand<T>`、`invokeReadCommand<T>`、operation-ID付きgeneric APIは削除済みである。contract testはrawを含む全commandがtyped registryに存在することを検証し、Rust DTO/command変更後に生成物またはcall siteが古ければ`ipc:check`かTypeScript compileが失敗する。

## ROUTE-01: 投稿 identity と acting account を全変更操作で明示する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | マルチアカウント、ドメイン identity |
| 依存 | ARCH-02、FE-01 |

### 問題と根拠

unified timeline での [`status_action`](../../../src/tauri_commands.rs#L3334) は active client と `status_id` だけで実行する。server domain、canonical URI、source account、操作するアカウントが要求にない。フロントエンドは既に URI / domain を含む `statusIdentity` を持つが、mutation payload は `originalStatusId` と action だけである。また [`session_for_domain`](../../../src/tauri_commands.rs#L4107) は同一 domain の複数 session から HashMap の最初の一致を選ぶ。

### 方針

- domain identity を `{ protocol, server_domain, canonical_uri, remote_id }`、操作主体を `acting_account_acct` として別々に持つ。
- mutation request は acting account を必須にし、暗黙の active account / first domain match を廃止する。
- remote status は acting server 上で URI lookup してから操作し、変換結果を短期 cache する。
- frontend entity key、DB unique key、stream event、notification、media overlay を同じ identity 規則へ移す。
- compose draft、upload 中 operation、取得済み media ID、custom emoji cache を account generation に紐付け、account switch 後に旧 account の resource を投稿へ流用しない。

### 受け入れ条件

- [x] 同じ instance の 2 アカウントで、どちらが操作主体か決定的である。
- [x] 異なる server で同じ文字列 ID の fixture を誤更新しない。
- [x] remote post の favorite / boost / vote / follow が canonical URI 解決を通る。
- [x] active account 切替中の旧要求は別アカウントで実行されない。
- [x] account switch 中の upload 完了結果、draft attachment、custom emoji response は元 account にだけ帰属し、別 account の compose へ混入しない。

## ARCH-03: protocol 非依存ドメインモデルと capability を導入する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / L** |
| 種別 | adapter 設計、機能交渉 |
| 依存 | ARCH-01、ARCH-02、ROUTE-01 |

### 問題と根拠

[`ApiClient`](../../../src/api/client.rs#L25) は 559 行の enum dispatch で、3 protocol に同じメソッド集合を見せる。共通層が Mastodon の型と `MastodonError` に寄り、未対応操作が空配列になる箇所と error になる箇所が混在する。利用側は「本当に 0 件」と「この protocol では未対応」を区別できない。

### 方針

- `TimelineReader`、`StatusMutator`、`RelationshipManager`、`MediaUploader`、`StreamingProvider` 等の小さな port に分ける。
- protocol DTO は adapter 内で domain DTO へ変換し、共通 error へ source-aware に写像する。
- session snapshot で capability と制約（文字数、visibility、poll、stream type 等）を UI へ公開する。
- unsupported は空結果ではなく typed capability error とし、UI が操作を出さない。

### 受け入れ条件

- [x] protocol 名を条件分岐せず capability で UI / use case の可否を決められる。
- [x] unsupported と empty result をテストで区別できる。
- [x] Mastodon 固有 DTO / error が domain と Misskey / Bluesky adapter の公開契約へ漏れない。
- [x] 新しい protocol を追加するとき既存 adapter の match を全メソッドで編集しなくてよい。

## DATA-03: DB の参照整合性と保存モデルを明示する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | スキーマ、参照整合性 |
| 依存 | DATA-01、DATA-02 |

### 問題と根拠

[`notifications`](../../../migrations/004_create_notifications.sql#L1) と [`timeline_entries`](../../../migrations/005_create_timeline_entries.sql#L1) は status / account への FK を持たず、logout や cache clear が手動の削除順序へ依存する。notification の primary key に受信 account が含まれず、同じ server 上の複数 account で ID が衝突し得る。status 行の `favourited` / `reblogged` / `bookmarked` / `muted` 等も viewer account に依存する値だが（[`003_create_statuses.sql`](../../../migrations/003_create_statuses.sql#L21)）、canonical status と同じ行に 1 値だけ保存され、複数 account の同期で上書きされる。さらに検索・絞り込み対象を含む複数フィールドを JSON TEXT に格納する（[`L25`](../../../migrations/003_create_statuses.sql#L25)）。

### 方針

- 既存 orphan を監査・修復してから composite FK と意図した `ON DELETE` を追加する。
- login account、server account、status identity、timeline membership の関係を ER 図と ADR で確定する。
- canonical status content と account-scoped viewer state を分け、後者を `(login_account, status_identity)` で保存する。notification identity も受信 account を key に含める。
- 検索／join／保持期限に使う値は正規化列または generated column と index を持つ。
- 起動時は schema version と `foreign_keys=ON` を定数時間で確認し、全件
  `PRAGMA foreign_key_check` は明示的な診断と migration fixture test で行う。
  multi-GB cache の全走査を初回ウィンドウ表示の前提にしない。

### 受け入れ条件

- [x] account / status 削除後に orphan notification、timeline entry、tag mapping が残らない。
- [x] cascade と retain の判断が entity ごとに文書化される。
- [x] 既存 DB を無損失で移行し、`foreign_key_check` が空になる。
- [x] canonical identity の unique 制約が protocol 間の正規ケースを壊さない。
- [x] 同一 server の 2 account で favourite / bookmark / mute と notification が相互上書き・衝突しない。

## SQL-01: Custom SQL を resource-limited read sandbox にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | 拡張機能、DB 可用性 |
| 依存 | ERR-01、BUDGET-01 |

### 問題と根拠

[`query_custom_statuses`](../../../src/tauri_commands.rs#L5114) は先頭が `SELECT` かを文字列で確認する。top-level LIMIT がある query は外側の hard limit で包まず、`LIMIT 1000000`、再帰 CTE、巨大 cross join 等を read pool で実行できる。OFFSET の扱いも手書き SQL scanner に依存し、既存 LIMIT 付き query の 2 ページ目は空になる。

### 方針

- custom query 専用の read-only connection を使い、SQLite authorizer で許可 opcode / object を限定する。
- progress handler / interrupt で命令数と wall-clock time を制限する。
- 結果 row 数と IPC payload size は、ユーザー SQL に LIMIT があっても常に外側で hard cap する。
- SQL parser または限定 DSL で syntax と pagination を扱い、keyset / cursor 契約を定める。
- `EXPLAIN QUERY PLAN` と cancel を UI へ提供する。

### 受け入れ条件

- [x] 巨大 LIMIT、recursive CTE、cross join、pragma、attach、write attempt が定めた予算内で停止／拒否される。
- [x] query timeout が通常 timeline reader を長時間占有しない。
- [x] pagination が重複／欠落なく動作し、結果上限を迂回できない。
- [x] safe error は query 位置を示すが、内部パスや他の秘密情報を露出しない。

Custom SQL専用analytics pool、SQLite authorizer/progress handler、VM命令・wall-clock・row・payload budget、外側pagination cap、`EXPLAIN QUERY PLAN`を実装済みである。frontend schedulerの`AbortSignal`をcaller-owned operation ID、`cancel_timeline_query`、backend cancellation registry、SQLite progress handlerまで接続し、pane close/query変更は開始済みSQL/API/DB futureを停止する。validation errorはraw SQLを返さず、scannerが特定した1始まりの行・桁だけをallowlist済み`safe_details`へ載せる。cancelled codeを含む全失敗は安全なIPC envelopeで返す。

## AUTH-01: Bluesky token refresh を single-flight・世代管理する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | 認証状態、並行制御 |
| 依存 | SEC-02、ERR-01 |

### 問題と根拠

Bluesky token snapshot は lock contention 時に空文字へ落ち得る（[`cached_access_token`](../../../src/bluesky/client.rs#L254)）。複数 request が 401 を受けた場合の refresh を single-flight にする世代／mutex がなく、rotating refresh token を並行更新し、古い結果や空値を DB へ保存する競合が生じ得る。

### 方針と受け入れ条件

- [x] token accessor は空文字 fallback をせず、async read または typed error を返す。
- [x] 1 session につき refresh は single-flight で、待機要求は同じ結果を共有する。
- [x] auth generation より古い refresh 結果を memory / DB へ反映しない。
- [x] 同時 401、refresh token rotation、refresh 失敗、logout 競合のテストがある。

`BlueskyClient`のtoken snapshotはpoisoned lockからも既存値を回収し、空文字へ置換しない。401 recoveryはsession単位のgeneration gateで成功・失敗を共有し、rotating sessionのSQLite保存まで同じcritical sectionに含める。logoutはgate無効化を待ってからSQLite account rowを削除するため、進行中rotationの保存より必ず後に削除が来る。さらに[`CredentialStore`](../../../src/auth/credential_store.rs)のlogin generationがlogout/re-login後の旧sinkを拒否する。同時401、共有失敗と後続retry、logout待機、旧sessionによるstale write拒否をconcurrency testで固定した。

## DEAD-01: 旧 GPUI 構造と全体 `dead_code` 抑制を整理する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | リポジトリ衛生、コンパイラ警告 |
| 依存 | ARCH-01、DOC-01 |

### 問題と根拠

[`Cargo.toml`](../../../Cargo.toml#L62) は crate 全体で `dead_code` を許可し、旧 GPUI 時代の state module や assets が残る。[`CLAUDE.md`](../../../CLAUDE.md#L7) と [`virtual-list-implementation.md`](../../virtual-list-implementation.md) も現行 Tauri / React 実装と異なる説明を持つ。必要な compatibility code と単なる残骸をコンパイラが区別できない。

### 方針と受け入れ条件

- [x] repository-wide allow を外し、serialization / platform 条件等で必要な箇所だけ局所 allow + 理由を付ける。
- [x] `app_state.rs`、`active_account.rs`、`session_pool.rs`、旧 SVG 等は参照と履歴を確認して削除または現用途を記録する。
- [x] `cargo clippy --all-targets -- -D warnings` が通る。
- [x] 現行アーキテクチャ文書が削除対象を参照しない。
