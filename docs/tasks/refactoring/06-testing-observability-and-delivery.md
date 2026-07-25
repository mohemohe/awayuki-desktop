# 06. テスト・観測性・配布

## QUAL-01: PR 必須の品質ゲートを作る

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | CI、静的検査 |
| 依存 | なし。既存 lint debt の修正を含む |

### 問題と根拠

現行の [`cache-warmup.yml`](../../../.github/workflows/cache-warmup.yml#L3) は main push 中心で、frontend build と `cargo check` を行うが、PR で test、fmt、clippy、TypeScript 型検査を必須化しない。[`package.json`](../../../package.json#L6) に frontend の test / lint / typecheck script もない。Vite build は `tsc --noEmit` の代わりにならない。

監査時点では build、TypeScript、37 Rust tests は成功した一方、`cargo fmt --check` は 1 ファイルで失敗し、`cargo clippy --all-targets -- -D warnings` は 20 errors / 1 warning で失敗した。これは通常 build が壊れているという意味ではなく、品質ゲートとしてまだ有効化できない負債である。

### 方針

- PR workflow に frozen Bun install、`typecheck`、ESLint、frontend tests、`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、production build を置く。
- lint は一括 disable せず、大きな enum は boxing、引数過多は request/config struct、dead code は削除または局所 allow で解消する。
- release は同一 commit の品質 job 成功を必須にし、再実行で検査を迂回しない。
- OS 固有 compile/package smoke は matrix へ分離し、重い job と高速 PR feedback を両立する。

### 受け入れ条件

- [ ] pull request で上記 gate が必須になり、全て green である。
- [x] `vite build` と `tsc` の役割を script 名で分ける。
- [x] generated IPC / migration / bundle budget の未反映差分も CI が検出する。
- [x] branch protection と release job の依存が文書化される。

## QUAL-02: リスクベースの自動テスト体系を整備する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | unit、integration、E2E、fault injection |
| 依存 | QUAL-01 |

### 現状

Rust tests は 37 件あり、「テストがない」わけではない。ただし主に `tauri_commands.rs`、`timeline_service.rs`、settings、paths、preset visibility に集中し、frontend test はない。高リスクな migration upgrade、OAuth callback、IPC 応答喪失、stream backpressure、transaction 部分失敗、updater 署名、sidecar isolation、package install を覆わない。

### テスト層

| 層 | 優先ケース |
| --- | --- |
| Rust unit | domain identity、capability、YQ plan、retry policy、redaction |
| Rust integration | 全 migration path／部分適用 DB、transaction fault injection、HTTP timeout、OAuth state/PKCE、stream lag/resync |
| Frontend unit / RTL | timeline reducer、boot error、設定応答逆転、login Enter、sanitizer/link、keyboard dialog/menu |
| Contract | Rust ↔ TS args/result/error、serde casing、unknown enum、mock exhaustive mapping |
| Tauri E2E | login callback、multi-account action routing、upload/download、sidecar 権限、upgrade/restart |
| Package smoke | clean macOS / Windows / Linux で install、launch、DB migration、update/uninstall |

### 受け入れ条件

- [ ] P0/P1 タスクは failure mode を再現するテストを修正前に持つ。
- [x] test が実 token、実資格情報、外部サービスの可用性へ依存しない。
- [x] flaky retry で隠さず、fake time、wiremock、temp DB、deterministic queue を使う。
- [x] OS 固有 test の未実行／skip が release summary で分かる。

## OPS-01: UI → IPC → API → DB を同じ operation ID で観測する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | ログ、metrics、診断 |
| 依存 | ERR-01、ARCH-02 |

### 問題と根拠

時間計測ログは既に一部あるが、操作を層間で関連付ける ID がなく、queue depth、stream sequence gap、resync、sync phase、DB statement 数、cache hit を確認できない。逆に DB summary や設定読込みが error を 0 / default に変換する箇所があり、障害が「データなし」に見える。最終 stream queue の drop counter も log だけで UI recovery へつながらない。

### 方針

- operation / request ID、account の匿名化 ID、command、phase、duration、result code を structured event にする。
- startup sync、HTTP、DB transaction、stream queue、frontend reducer の span を関連付ける。
- queue depth、drop/coalesce/resync、DB busy、query rows/time、HTTP retry/rate limit、cache size の health snapshot を持つ。
- support bundle は明示操作で生成し、schema/version/config の安全な subset と直近 rolling logs を含める。
- token、password、OAuth query、本文、任意 path を default で収集しない。

### 受け入れ条件

- [x] 1 件の投稿／refresh を operation ID で UI から外部 API と commit まで追える。
- [x] slow startup が network、quote、DB lock、queue backlog のどれか判定できる。
- [x] stream lag を検知すると UI/diagnostics に resync 状態が出る。
- [x] redaction fixture と support bundle 内容の snapshot test がある。

## REL-01: 再現可能な単一 release pipeline と artifact manifest を作る

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | Release、provenance、保守性 |
| 依存 | QUAL-01、SEC-06、SEC-07 |

### 問題と根拠

[`release.yml`](../../../.github/workflows/release.yml#L1) と [`manual-build.yml`](../../../.github/workflows/manual-build.yml#L1) は macOS / Windows / Linux の build、署名、WinSparkle 取得、artifact 作成を大幅に重複する。platform script にも処理が分散し、version は `Cargo.toml`、`package.json`、Tauri conf、Info.plist、Taskfile、script に複数存在する。片方だけ security fix や asset 名を変更し得る。

### 方針

- reusable workflow と platform packaging script に集約し、tag release と manual build は同じ実装を呼ぶ。
- version は tag / 1 metadata source から導出し、全 manifest の一致を build 前に検証する。
- artifact ごとに source commit、toolchain、dependency lock、SHA-256、size、signature、SBOM、provenance を manifest 化する。
- appcast は manifest から生成し、XML schema、署名、重複 version、downgrade を検証してから publish する。
- publish に concurrency group と fetch/rebase/retry を持たせる。

### 受け入れ条件

- [x] 同じ commit の manual/tag build が同じ工程と naming 規則を使う。
- [x] manifest version 不一致や checksum 不一致で publish が停止する。
- [x] release page と appcast が同じ artifact digest を参照する。
- [x] SBOM / provenance / signatures の検証手順を利用者と保守者向けに記載する。

## REL-02: 署名 job の ref・権限・secret 境界を狭める

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | CI trust boundary |
| 依存 | なし。ref / permission 制限を先行し REL-01 へ統合 |

### 問題と根拠

[`manual-build.yml`](../../../.github/workflows/manual-build.yml#L4) は任意 branch / tag / SHA を入力として checkout でき、workflow 全体に `contents: write`、署名／公証 secret を使う job がある。Actions 実行権限を持つ主体が、保護 branch の review を経ない任意コードを公式署名する経路になり得る。

### 方針と受け入れ条件

- [x] 署名対象を protected tag または allowlisted commit に限定し、commit ancestry と tag signature を job 内で検証する。
- [ ] signing / notarization / publish は GitHub Environment の必須承認を通る。
- [x] `permissions` を job ごとに最小化し、build job は write token と signing secret を持たない。
- [x] untrusted code が secret を持つ runner で任意 script を実行しない二段構成にする。
- [x] audit log と emergency revocation/runbook がある。

## PKG-01: 3 OS の package 定義を clean environment で検証する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | Packaging、クロスプラットフォーム |
| 依存 | REL-01 |

### 問題と根拠

[`PKGBUILD`](../../../PKGBUILD#L30) は旧 GPUI 時代の依存と `cargo build` 中心の手順を残し、現在必要な Bun frontend build、WebKitGTK / GTK runtime、Tauri の `frontendDist` 前提と整合しない可能性が高い。macOS / Windows の package も通常 build 成功だけでは install、migration、updater、uninstall を証明しない。

### 方針と受け入れ条件

- [x] clean Arch container の `makepkg -s` で frontend を含む package を作成し、launch smoke が通る。
- [x] source archive、checksum、build/runtime dependency、license、desktop/icon を現行 Tauri 構成へ更新する。
- [ ] clean macOS / Windows VM でも install → launch →既存 DB upgrade → uninstall を確認する。
- [x] package にmock fixture、不要 secret、build cacheが入っていないことを検査する。DevToolsは利用者のbug report採取用に意図して含める。

2026-07-12 のローカル clean-container 検証では、`makepkg` の既定
`BUILDDIR=$PWD`により生成用`$srcdir`がRustの`./src`と衝突し、
`--cleanbuild`がソースを削除する不備を検出した。`PKGBUILD`は危険な配置を
ビルド開始前に拒否し、READMEとArch workflowは隔離`BUILDDIR`を必須化した。
ネイティブaarch64 Arch containerではfrontend 1,749 modulesとrelease binaryを
含むpackageを作成し、payload検査、pacman install、fresh DB起動、migration 019
fixtureからschema 28へのupgrade、再起動、uninstall後のDB保持まで成功した。
Arch Linux ARM repositoryに`bun` packageがないため、公式Bun 1.3.9 binaryを
SHA-256固定したローカルpacman package（`provides=('bun=1.3.9')`）として先に
導入し、その後Awayukiを依存検査を省略せず
`makepkg --syncdeps --noconfirm --cleanbuild`で作成した（binary 27,164,200 bytes、
package 7,211,400 bytes）。公式x86_64 runnerとmacOS / Windows hosted runnerの
完走は引き続き受け入れ条件として残す。

## DEP-01: Toolchain・依存 feature・更新ポリシーを固定する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 依存管理、build size |
| 依存 | QUAL-01、REL-01 |

### 問題と根拠

Rust toolchain と Bun version が workflow 上の movable な指定で、Actions も tag 参照が中心である。Cargo の Git dependencies は lockfile では commit 固定されるが manifest に意図した `rev` がなく、更新理由を追いにくい。Tokio `full`、reqwest default TLS + rustls 等の広い feature は compile time、binary size、攻撃面を増やす可能性がある。

### 方針と受け入れ条件

- [x] `rust-toolchain.toml`、Bun/package manager version、Actions commit SHA を固定する。
- [x] `--locked` / frozen install を CI と release で必須にする。
- [x] Git dependency は意図した rev または release version を manifest に明示する。
- [x] Tokio / TLS / Tauri feature を実使用に絞り、binary size、build time、platform compatibility を比較する。
- [x] Dependabot/Renovate 相当、`cargo audit`、`cargo deny`、license policy を定期実行し、更新 SLA を定める。

## DOC-01: 現行アーキテクチャと運用契約へ文書を更新する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | ドキュメント、onboarding |
| 依存 | ARCH-01、FE-07 |

### 問題と根拠

[`CLAUDE.md`](../../../CLAUDE.md#L7) は macOS / GPUI、存在しない module、テスト未整備を説明し、現行 Tauri / React / 3 OS 実装と異なる。[`virtual-list-implementation.md`](../../virtual-list-implementation.md) は gpui-component 前提だが現行は React Virtuoso である。[`yq-query-reference.md`](../../yq-query-reference.md#L35) の single-account 説明は multi-account 製品との関係が曖昧である。README の公証説明も安全性保証と受け取られ得る。

### 方針と受け入れ条件

- [x] current architecture、IPC、DB、streaming、sidecar、frontend state、3 OS build の入口文書を作る。
- [x] VirtualList 文書を Virtuoso の scroll anchor、retention、測定、性能予算へ更新する。
- [x] YQ の `from` 未実装と製品の multi-account 対応を分けて説明する。
- [x] test/build/release/security model、資格情報、保持期限、SQLite-only portabilityを記載する。
- [x] README の署名／公証を「発行者確認とプラットフォーム検査」であり完全な安全保証ではない表現にする。
- [x] 文書内の command/path/link を CI で検証し、architecture change には ADR 更新を要求する。

## BUDGET-01: 性能予算と回帰 benchmark を CI に置く

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 性能検証、SLO |
| 依存 | OPS-01 |

### 方針

固定 seed で小規模、42 万 status 相当、大規模の DB を生成し、以下を継続計測する。

- cold / warm startup の ready 時間、API calls、DB statements、peak RSS。
- local search / YQ / aggregate / notification / thread の p50 / p95、scanned rows、query plan。
- stream 100 events/s 時の queue、drop/resync、DB lag、frontend frame / commit time。
- 8 時間相当 scroll の entity/cache/timer 数と heap。
- upload/download の peak memory と throughput。
- raw / gzip / brotli bundle、Rust binary、package size、compile time。

### 受け入れ条件

- [x] benchmark dataset と command が再現可能で、資格情報や実ユーザーデータを含まない。
- [x] absolute budget と main 比の回帰閾値を設定し、noise の大きい値は trend artifact として扱う。
- [x] PR summary に before / after と実行環境を表示する。
- [ ] PERF 各タスクは対応する metric を改善し、別 metric の悪化も報告する。
