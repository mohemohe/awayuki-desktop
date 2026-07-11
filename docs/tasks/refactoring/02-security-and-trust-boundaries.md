# 02. セキュリティと信頼境界

この章は「脆弱性が実証済み」という断定ではなく、外部サーバー、ブラウザ表示、ローカルファイル、単一 SQLite 状態、配布物の境界で現在不足している防御をタスク化する。脅威モデルを先に合意し、修正後は各 OS の実機確認を行う。

## CRED-01: DB・log・保存 directory の権限を即時補正する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P0 / S** |
| 対象 | SQLite、log、debug / portable mode |
| 依存 | なし。SEC-02 の本移行を待たない |

### 問題と根拠

監査環境の既存 DB は実際に mode `0644` だった。debug build は current working directory、portable mode は実行ファイル隣へ DB / log を置く（[`paths.rs`](../../../src/state/paths.rs#L12)）ため、共有 checkout／媒体では長期 secret を別ユーザーが読める可能性がある。

### 方針と受け入れ条件

- [ ] 起動時に DB、WAL、SHM、log を `0600`、格納 directory を `0700` 相当の OS ACL へ補正する。
- [ ] 新規作成時点から最小権限を使い、作成後 chmod まで world-readable な窓を作らない。
- [ ] debug / portable mode は保存先と secret リスクを明示し、安全な権限を設定できない媒体ではログインを警告／拒否する方針を持つ。
- [ ] log に secret がないことを redaction test で確認する。
- [ ] macOS / Linux の mode と Windows ACL を integration test する。

## SEC-01: 連合先 HTML を allowlist sanitizer に通す

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 対象 | 投稿本文、プロフィール、カスタム絵文字、リンク |
| 依存 | QUAL-02 |

### 問題と根拠

[`CustomEmoji`](../../../frontend/src/components/common/CustomEmoji.tsx#L35) は `dangerouslySetInnerHTML` を使う。前処理は [`template.innerHTML`](../../../frontend/src/components/common/CustomEmoji.tsx#L98) へ外部 HTML を読み込み、カスタム絵文字のテキストノードを置換するが、要素・属性・URL scheme を sanitize しない。CSP は多層防御にはなるが、危険な markup、追跡 URL、意図しない navigation を無害化する入力境界ではない。

### 方針

- Mastodon 系の想定 HTML subset を定義し、要素・属性・protocol の allowlist sanitizer を 1 箇所に置く。
- `href` は `http/https` 等の許可 scheme に限定し、`target` / `rel` とアプリ内／外部表示方針を付与する。
- `img` はカスタム絵文字等の既知用途に限定し、event 属性、style、SVG、`data:` 等を脅威モデルに従い除去する。
- plain text 化にも DOM の暗黙挙動ではなく、同じ正規化済み表現を利用する。

### 受け入れ条件

- [ ] `onerror`、`javascript:`、危険な SVG / style / iframe、壊れた HTML の fixture が無害化される。
- [ ] mention、hashtag、改行、カスタム絵文字など正規の投稿表示は維持される。
- [ ] 外部リンクは opener を共有せず、期待した経路だけで開く。
- [ ] sanitizer の allowlist 変更にレビュー可能なテスト差分が出る。

## SEC-02: 資格情報の SQLite-only portable contract を固定する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 対象 | access token、Bluesky session / app password、portable mode |
| 依存 | DATA-01 |

### 問題と根拠

Awayuki は、`awayuki.db` を移動するだけでログイン状態を含む全機能が portable に動くことを製品契約とする。資格情報を OS Keychain / Credential Manager / Secret Service 等へ分離したり別DB backupを自動生成するとこの契約を破り、SQLite とシステム状態の不整合も生む。DBには再利用可能なsecretが含まれるため、単一ファイルであることを前提に権限・ログ・support境界を厳格化する必要がある。

### 方針

- access token、refresh token を含む Bluesky session、app password は `login_accounts` の SQLite 列だけを正本とし、OS store、registry、別 file へ永続化しない。
- login、token rotation、logout は SQLite transaction と同じ serialized lifecycle で更新する。
- DB、WAL、SHM を private permission と restrictive umask で作成・補正する。
- ログ、Debug、panic、IPC result、support bundle で secret を redaction する。
- DB に資格情報が含まれることを README で明示する。
- streaming URL 等に protocol 上 token が必要な場合も、URL／query 全体をログへ出さず、header が使える protocol では header を優先する。
- 現行 `client_credentials` の read path は使われていないため、必要性を再確認し、不要なら secret store へ移さず table と保存処理を削除する。

### 受け入れ条件

- [ ] OS credential store / registry / 別 file への write dependency がない。
- [ ] `awayuki.db` の移動後に access token と Bluesky app password を復元できる自動テストがある。
- [ ] login、rotation、logout の部分失敗で account row と資格情報が不整合にならない。
- [ ] DBとportable modeの機密性・共有禁止、別backupを作らない契約がREADME / ADRに明記される。

## SEC-03: OAuth callback に state、PKCE、listener 所有権を導入する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 対象 | Mastodon OAuth、Misskey MiAuth |
| 依存 | ERR-01 |

### 問題と根拠

Mastodon の [`authorize_url`](../../../src/mastodon/oauth.rs#L52) は `state` と PKCE を付けない。callback server は [`wait_for_callback_any`](../../../src/auth/callback_server.rs#L20) で先着リクエストから値を取得する。さらに [`find_available_port`](../../../src/auth/callback_server.rs#L79) は bind して得た port を解放し、別タスクが再 bind するため TOCTOU がある。callback の path / method / state、有効期限、キャンセルを 1 セッションとして管理していない。

### 方針

- ランダムな single-use `state` を発行・定数時間比較し、対応サーバーでは S256 PKCE を用いる。
- 最初に bind した listener をそのまま callback task へ渡し、port 解放／再 bind をなくす。
- `/callback` の GET のみ、期待した query のみを受け付け、他要求には成功扱いを返さない。
- timeout、画面 close、再ログイン開始時の cancel と session cleanup を持たせる。
- 要求 scope / permission を実利用機能に合わせて棚卸しする。

### 受け入れ条件

- [ ] state 不一致、再利用、別 path、期限切れ、先着ノイズ要求を拒否する。
- [ ] port 競合を起こさず、listener はセッション終了まで 1 所有者が保持する。
- [ ] PKCE 対応／非対応サーバーの互換性テストがある。
- [ ] callback URL やログに code / verifier を残さない。

## SEC-04: メディア／ローカルファイル IPC を有界・ストリーム型にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 対象 | upload、drag & drop、download、一時ファイル |
| 依存 | ARCH-02 |

### 問題と根拠

ブラウザの `File` は [`fileToByteArray`](../../../frontend/src/utils/format.ts#L3) で `Array<number>` に展開され、[`ComposeArea`](../../../frontend/src/components/compose/ComposeArea.tsx#L356) から IPC へ渡る。複数の全量コピーと JSON/IPC 変換で、ファイルサイズより大幅にメモリを使う。バックエンドの [`upload_compose_media`](../../../src/tauri_commands.rs#L2775) は全データを一時ファイルへ書き、drop path 版 [`upload_compose_media_path`](../../../src/tauri_commands.rs#L2798) は渡されたパスをそのまま読む。download も [`download_media`](../../../src/tauri_commands.rs#L3544) で response 全量を `bytes()` に読み込む。共通のサイズ、MIME、timeout、path 所有権制限がない。

### 方針

- Tauri の file handle / scoped path / streaming channel を使い、byte array の IPC 搬送を廃止する。
- UI 選択または正規の drop event で発行した capability token と path を対応付け、任意 path の読取りを拒否する。
- protocol／server 制限とローカル上限の小さい方でサイズ、件数、MIME、extension を検証する。
- download は timeout、最大サイズ、incremental write、キャンセル、原子的 rename を持たせる。
- redirect 後も scheme / host / private-address 方針を再検証し、同名既存ファイルは truncate せず `create_new` または安全な別名を使う。
- 一時ファイルは成功・失敗・クラッシュ後の cleanup 規則を持つ。
- frontend の `blob:` preview は remote URL 採用時、削除時、submit、unmount、account switch で必ず revoke する。

### 受け入れ条件

- [ ] 大きなメディアでも `Array<number>` と全量 response buffer を作らない。
- [ ] 許可されていないローカルパス、上限超過、MIME 偽装、遅い／無限 response を拒否する。
- [ ] upload / download の progress と cancel が UI へ伝わる。
- [ ] 失敗後に秘密ファイルや一時ファイルが残らない。

## SEC-05: Sidecar WebView の capability と navigation policy を閉じる

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 対象 | child WebView、user CSS、open/download |
| 依存 | なし。policy を先行 |

### 問題と根拠

初期 URL は http/https に制限される一方、child WebView の navigation handler は [`tauri_commands.rs`](../../../src/tauri_commands.rs#L468) で後続 navigation を広く許可する。user CSS injection はバックエンドの [`schedule_sidecar_user_style_injection`](../../../src/tauri_commands.rs#L568) とフロントエンドの [`WorkspaceView`](../../../frontend/src/components/workspace/WorkspaceView.tsx#L328) の両方が retry を所有し、重複実行と lifecycle 漏れを招く。default capability は `windows: ["main"]` と WebView 作成／操作等の広い権限を持ち（[`capabilities/default.json`](../../../capabilities/default.json#L5)）、Windows conf は global Tauri injection も有効にする（[`tauri.windows.conf.json`](../../../tauri.windows.conf.json#L4)）。現設定に remote origin capability はないため remote page が直ちに command を呼べるとは断定しないが、main local WebView と sidecar label を明示分離するべきである。

### 方針

- Sidecar ごとに origin allowlist、navigation、popup、download、external open、clipboard、file access の方針を定義する。
- capability は main UI と sidecar を分離し、sidecar へ Tauri IPC 権限を付与しないことをテストする。
- `windows` 一括指定ではなく main の `webviews` label へ限定し、未使用 permission と global injection を削る。
- style injection と再試行は backend か frontend の一方だけを lifecycle owner にする。
- close / navigate / reload 時に timer、map、event listener を確実に解放する。

### 受け入れ条件

- [ ] 許可 origin 外への navigation、popup、local scheme、予期しない download の動作が定義され、既定拒否になる。
- [ ] sidecar ページから app command を直接呼べない。
- [ ] CSS injection が 1 lifecycle につき意図した回数だけ実行される。
- [ ] create / reload / close の反復で map と timer が増え続けない。

## SEC-06: OS側へ状態を持つ updater を全プラットフォームから廃止する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 対象 | Sparkle / WinSparkle、appcast metadata、3 OS release artifact |
| 依存 | なし |

### 問題と根拠

旧 Windows updater は署名のない enclosure を受け入れ、WinSparkle が更新状態を registry に永続化する。macOS Sparkleも最終確認時刻や自動確認設定をOS preferenceへ保存する。いずれも`awayuki.db`だけを移動すれば状態が完結し、OS側へ状態を書かないportable contractと両立しない。

### 方針

- Sparkle / WinSparkle dependency、framework、feature、FFI、Info.plist feed設定を削除し、全OSを検証済みrelease assetの手動更新だけにする。
- release manifestでpackageのhash、size、versionを検証するが、自動更新可能とは表示しない。appcastは公開metadata検証に限定し、アプリから参照しない。
- 将来の再導入にはcode signing、enclosure signature、downgrade拒否に加え、OS preference / registry等へ状態を書かないSQLite-only設計を必須とする。
- README、runbook、CI dependency checkで全OSの手動更新方針を固定する。

### 受け入れ条件

- [ ] 3 OS release graphにSparkle / WinSparkle、updater feature、OS preference / registry write経路が存在しない。
- [ ] app packageがappcastを参照せず、artifactを自動更新対象として表示しない。
- [ ] manifest不一致、artifact差替え、version不一致でpublishが停止する。
- [ ] clean 3 OS runnerでpackageの展開・起動を検証し、手動更新方針をREADMEへ明記する。

## SEC-07: ビルド時ダウンロードと依存バイナリを固定・検証する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 対象 | linuxdeploy、Git dependencies、Actions |
| 依存 | なし |

### 問題と根拠

[`build-appimage.sh`](../../../scripts/build-appimage.sh#L61) のようなbuild時取得物は、movable artifactやchecksumなしで実行すると供給元が同じでも再現性と監査性を損なう。Git dependency、Actions、packaging toolの参照も同様にimmutable versionとdigestへ固定する必要がある。

### 方針

- download は immutable version URL と SHA-256 を固定し、検証前に実行／展開しない。
- third-party binary を専用 vendor/cache step で取得し、Cargo checkout を書き換えない。
- Git dependencies は明示 `rev` または release version に固定する。
- Actions を commit SHA に pin し、SBOM、artifact manifest、provenance を REL-01 で生成する。

### 受け入れ条件

- [ ] digest 不一致なら build が停止する。
- [ ] 同じ source revision と toolchain から artifact manifest が再現できる。
- [ ] build script が dependency checkout や global cache を変更しない。
- [ ] 取得物の version、license、hash、更新手順が一覧化される。

## SEC-08: macOS entitlement を最小化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 対象 | macOS entitlements |
| 依存 | SEC-09 |

### 問題と根拠

macOS entitlements は JIT、unsigned executable memory、library validation 無効化を含む（[`Entitlements.plist`](../../../resources/Entitlements.plist#L6)）。Sparkle削除後も残る各許可について、必要性と外した実機検証が文書化されていない。

### 方針と受け入れ条件

- [ ] 各 entitlement を外した build/runtime 検証を行い、必要なものだけ理由とリスクを記録する。
- [ ] release artifact に有効な entitlement の監査 snapshot を残す。

## SEC-09: Release build から DevTools と不要な unstable feature を外す

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / S** |
| 対象 | Tauri features、window config、release artifact |
| 依存 | なし |

### 問題と根拠

[`Cargo.toml`](../../../Cargo.toml#L8) は全 build で `devtools` と `unstable` feature を有効化し、macOS / Windows を含む Tauri 設定も DevTools を有効にする。配布版で不要な内部状態／操作面と build surface を残す。

### 方針と受け入れ条件

- [ ] DevTools は debug feature / config に限定し、release binary から無効化する。
- [ ] `unstable` を利用箇所ごとに棚卸しし、不要なら外す。必要なら局所的な理由と更新監査を記録する。
- [ ] 3 OS の release artifact で DevTools shortcut / API が使えないことを smoke test する。
- [ ] debug workflow の開発体験は維持する。

## SEC-10: CSP の deny-default と通信先を最小化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 対象 | Tauri WebView CSP |
| 依存 | SEC-01、SEC-05、SEC-09 |

### 問題と根拠

現行 CSP は remote media のため広い http/https/wss と inline style を許可する（[`tauri.conf.json`](../../../tauri.conf.json#L22)）。一方、frontend の通常 API 通信は Rust 経由であり、`connect-src` は実利用より広い可能性がある。`object-src`、`base-uri`、`form-action`、`frame-src` 等の deny も明示されない。

### 方針と受け入れ条件

- [ ] DevTools / CSP report を使い、image、media、IPC、sidecar ごとに本当に必要な source を測る。
- [ ] `object-src 'none'`、`base-uri 'none'`、`form-action 'none'`、不要なら `frame-src 'none'` を明示する。
- [ ] `connect-src` を IPC と実利用先へ絞り、remote HTML injection から任意外部送信できない。
- [ ] image / video preview、custom emoji、protocol ごとの media URL、sidecar を壊さない 3 OS 回帰テストがある。
- [ ] 例外を追加するときは threat model と削除条件を記録する。
