# 01. 正しさ・データ保全

## SAFE-01: IPC の再試行を副作用安全にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P0 / M** |
| 種別 | 正しさ、API 契約 |
| 依存 | なし |

### 問題と根拠

[`invokeCommand`](../../../frontend/src/api/tauri.ts#L8) は、一時的と判定した IPC エラーに対してコマンド種別を問わず 75 ms 後に同じ要求を再送する。一方、同じ入口から [`post_status`](../../../frontend/src/store/appStore.ts#L939)、アクション、投票、編集、削除、設定保存、アカウント切替なども呼ばれる。

バックエンドで処理が完了し、結果配送だけ失敗した場合、再送は投稿・投票などを二重実行する。クライアント側は「失敗したか不明」という状態を「未実行」と同一視しており、冪等性キーもない。

### 方針

- まず自動再試行を読み取り専用コマンドに限定し、変更系は盲目的に再送しない。
- コマンド記述子に `read` / `idempotent-write` / `non-idempotent-write` とタイムアウト方針を持たせる。
- provider が idempotency key を支える操作は operation ID を外部 API まで渡し、バックエンド／DB の operation ledger から同じ結果を返す。
- provider が exactly-once を支えない操作ではローカル台帳だけで外部副作用を保証できないため、自動再送せず「処理結果不明」を UI 状態として表現し、再取得による照合か明示的な再実行を選べるようにする。
- この分類を ARCH-02 の生成型 IPC 契約へ統合する。

### 受け入れ条件

- [ ] 非冪等な変更コマンドは、配送エラーだけでは自動再送されない。
- [ ] 読み取りコマンドの再試行回数・対象エラー・待機時間が明示されている。
- [ ] provider が冪等性を保証する変更系は、同じ operation ID を複数回受けても外部 API と DB の副作用が 1 回になる。
- [ ] provider が保証しない変更系は exactly-once と誤表示せず、応答喪失時に `uncertain` と reconciliation 導線を返す。
- [ ] 「バックエンド成功、応答喪失」を模擬した自動テストがある。
- [ ] ログで attempt と operation ID を追えるが、本文や資格情報は含まない。

## DATA-01: バージョン付き・原子的な DB マイグレーションへ移行する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P0 / L** |
| 種別 | データ保全、起動処理 |
| 依存 | QUAL-02 の移行テストを同時実施 |

### 問題と根拠

[`run_migrations`](../../../src/db/pool.rs#L50) は SQL ファイルを手動配列で実行し、ALTER 系はエラーメッセージに `duplicate column name` が含まれればスクリプト全体を無視する。適用バージョン・チェックサム・トランザクションがない。

たとえば [`008_add_credentials.sql`](../../../migrations/008_add_credentials.sql#L1) は ALTER と CREATE TABLE を同じファイルに含む。ALTER だけ適用済みでテーブル作成前に終了した DB は、再起動時に重複カラムでファイル全体が無視され、欠けたテーブルが修復されない。同様に複数 ALTER を含む移行も部分適用され得る。`pane_index` の補正 UPDATE も毎起動実行される。

### 方針

- `sqlx::migrate!` 相当の履歴テーブル、単調増加バージョン、チェックサム検証へ移行する。
- 1 マイグレーションを 1 トランザクションで適用し、各スキーマ操作を再開可能な単位に分ける。
- 初回切替時に現行スキーマを検査し、既知の部分適用状態を修復してからベースラインを記録する。
- ディスク不足・破損は変更前のintegrity checkとtransaction rollbackで安全に停止し、別DB backupは作らない。
- 全既存バージョンと代表的な部分適用 DB を fixture 化する。

### 受け入れ条件

- [ ] 適用済みバージョンとチェックサムが DB に記録される。
- [ ] 途中失敗した移行は全体がロールバックされ、次回起動で安全に再試行できる。
- [ ] `001` から最新版まで、各リリース相当 DB からのアップグレードテストが通る。
- [ ] 008 / 012 などの部分適用 fixture が完全な最新版へ修復される。
- [ ] 失敗時は同じDBのtransactionがrollbackされ、別のbackup/recovery fileを残さない。

## DATA-02: 複数ステートメント更新をユースケース単位で原子的にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | データ整合性、トランザクション設計 |
| 依存 | DATA-01 |

### 問題と根拠

- [`set_active_account`](../../../src/db/queries/settings.rs#L118) は全アカウントを非 active にした後、対象を active にする別 UPDATE を実行する。
- [`switch_active_account`](../../../src/tauri_commands.rs#L2963) は対象 session / DB row の存在を先に確定せず、memory 側 `set_active` の成否も整合性判定に使わない。
- [`logout`](../../../src/tauri_commands.rs#L2975) はフォールバック選択、関連データの付替え／削除、active 更新を別々に実行する。
- [`save_columns`](../../../src/tauri_commands.rs#L3234) は既存構成を削除してから 1 件ずつ挿入するため、途中失敗でレイアウト全体を失い得る。
- ステータス保存も、本文・アカウント・タグ・timeline entry が複数の独立書き込みに分かれている（[`save_status_to_db`](../../../src/services/timeline_service.rs#L594)）。

### 方針

DB query 関数へ `&mut Transaction<'_, Sqlite>` を渡せる形を用意し、logout、active account 切替、column 全量保存、status + tags + timeline entry、cache clear を各 1 トランザクションにする。外部 API 呼び出しはトランザクション外で行い、DB commit 後にだけメモリ状態とストリームを更新する。

### 受け入れ条件

- [ ] 上記ユースケースの各ステートメント間に fault injection しても、before / after のどちらか一方だけが観測される。
- [ ] active account は常に 0 または 1 件という不変条件を持ち、必要なら DB 制約でも保証する。
- [ ] 存在しない／削除済み acct への切替は DB と memory を変更せず typed error になる。
- [ ] column 保存失敗で既存レイアウトを失わない。
- [ ] commit 失敗時に UI の楽観状態を確定扱いしない。

## CONF-01: 設定を型付き・バージョン付きスキーマで管理する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | 設定契約、データ移行 |
| 依存 | ARCH-02 |

### 問題と根拠

[`save_settings`](../../../src/tauri_commands.rs#L3007) は一部キー以外を任意 JSON として保存する。読み込み側の [`load_setting`](../../../src/tauri_commands.rs#L4021) はデシリアライズ失敗を `unwrap_or_default()` で隠し、壊れた値を DB に残したままデフォルトへ戻す。設定追加・名称変更・型変更に移行契約がない。

### 方針

- 設定キー、Rust/TypeScript 型、既定値、検証、スキーマバージョンを 1 つの registry に集約する。
- 書き込み時に全キーを検証し、未知キーは明示的に拒否する。
- 読み込み破損はログ／UI で通知し、元の値を退避してから復旧する。
- 型変更はバージョン間 migration と round-trip test を必須にする。

### 受け入れ条件

- [ ] 未知キー、型違い、範囲外値を保存できない。
- [ ] 破損 JSON は黙って既定値にならず、復旧と診断が可能である。
- [ ] Rust と TypeScript の設定型が同じ生成元または contract test で同期される。
- [ ] 既存 DB の値を維持した migration test がある。

## ERR-01: IPC エラーを構造化し、内部情報と UI 表示を分離する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | エラー設計、i18n、観測性 |
| 依存 | なし |

### 問題と根拠

多数の command が `Result<_, String>` と `.map_err(|e| e.to_string())` を使い、ネットワーク、認証、入力、DB、内部バグの区別を失う。フロントエンドは raw message を toast に出す箇所があり、再試行可能性を判断できず、英語の内部情報を利用者へ露出し得る。

### 方針

`AppError { code, message_key, safe_details, retryable, request_id }` を IPC 境界の共通 envelope とする。原因 chain は redaction 済みログへ残し、UI は安定した code と翻訳 key を扱う。外部 API adapter から command 境界まで typed error を保つ。

### 受け入れ条件

- [ ] 認証切れ、rate limit、timeout、validation、DB busy、internal を機械判定できる。
- [ ] UI 表示に token、SQL、ローカルパス、内部 stack が混入しない。
- [ ] SAFE-01 の再試行可否が文字列部分一致ではなく error code と command policy で決まる。
- [ ] request ID で UI 操作からバックエンドログまで追跡できる。

## ASYNC-01: 非同期操作に世代・キャンセル・確定状態を持たせる

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 競合状態、UI 状態機械 |
| 依存 | FE-01 |

### 問題と根拠

column load などは in-flight map で重複を抑えるが、ペインを閉じる／アカウントを切り替える／query を変更する間に旧要求が完了すると、古い結果で新しい状態を上書きできる。profile、autocomplete、ページングにも共通のキャンセル／世代契約がない。

### 方針と受け入れ条件

- [ ] resource key ごとに generation または operation ID を持ち、旧世代の結果を捨てる。
- [ ] 可能な操作は `AbortSignal` でバックエンド／HTTP までキャンセルする。
- [ ] `idle/loading/refreshing/succeeded/failed/cancelled/uncertain` を明示し、単一の boolean で表現しない。
- [ ] pane close、query 変更、account switch の競合テストがある。
