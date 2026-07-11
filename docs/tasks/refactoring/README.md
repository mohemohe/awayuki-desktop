# Awayuki for Desktop リファクタリング監査

- 最終更新: 2026-07-11
- 監査対象: `0bb67a2` (`v0.7.1`) 時点の `src/`、`frontend/src/`、`migrations/`、ビルドスクリプト、CI/CD、既存ドキュメント

## 目的

この文書群は、現状のコードを「技術的負債」「設計上の不備」「性能」「保守性・運用性」の観点で静的監査し、実装可能なタスクへ分解したバックログである。単なるファイル分割ではなく、データ破損・二重実行・信頼境界・待ち時間・変更容易性に効く順で並べた。

## 監査時のベースライン

| 項目 | 結果 | 補足 |
| --- | --- | --- |
| `bun run build` | 成功 | main JS 549.14 kB（gzip 168.62 kB）、`SqlEditor` 472.20 kB（gzip 155.02 kB）、`unicodeEmoji` 940.75 kB（gzip 63.89 kB） |
| `bunx tsc --noEmit` | 成功 | `vite build` 自体には型検査が含まれていない |
| `cargo test` | 成功 | 37 tests |
| `cargo fmt --check` | 失敗 | `src/services/streaming_service.rs` に整形差分 |
| `cargo clippy --all-targets -- -D warnings` | 失敗 | 20 errors / 1 warning。大きな enum、引数過多などを含む |
| フロントエンド test / lint | 未整備 | `package.json` に該当 script がない |

これは静的監査とローカルのビルド・テスト結果であり、実サービスに接続した長時間運転、巨大 DB、低速回線、各 OS のインストーラー／自動更新は未検証である。性能タスクは、最初に計測基盤と再現データを用意してから数値目標を確定する。

## 優先度と工数の読み方

| 記号 | 意味 |
| --- | --- |
| P0 | リリースを待たず着手する。二重実行、データ破損などの重大な正しさの問題 |
| P1 | 次の開発マイルストーンで扱う。信頼境界、主要性能、変更容易性に直接効く |
| P2 | P0/P1 の基盤後に計画実施する |
| P3 | 周辺変更時に回収する |
| S | おおむね 2 開発日以内 |
| M | おおむね 3〜7 開発日 |
| L | 1 週間超、または段階移行が必要 |

工数は順序付けのための相対値であり、見積もりの確約ではない。

## 最重要の結論

1. **全 IPC の一律リトライを止める。** 現在は投稿・投票・削除などの副作用コマンドも再送され、バックエンド成功後に応答だけ失われると二重実行になり得る。
2. **DB マイグレーションと複数ステートメント更新を原子的にする。** 現行の「重複カラム文字列ならスクリプト全体を無視」は部分適用状態を恒久化し得る。
3. **連合先 HTML、OAuth、資格情報、ファイル／WebView／更新機構の信頼境界を明示する。** デスクトップアプリでは Web 表示の問題がローカル権限境界へ近づく。
4. **起動時全件同期・引用の直列リトライ・無制限キューを非同期かつ有界にする。** 現在の構造は、アカウント数・履歴・通信遅延に比例して起動時間とメモリを悪化させる。
5. **8,201 行の IPC 層と巨大な Zustand／React コンポーネントを、型付き契約とユースケース境界から分離する。** ファイル分割だけを先行させず、安全性テストを先に置く。

## タスク一覧

| 領域 | P0 | P1 | P2 | P3 | 文書 |
| --- | --- | --- | --- | --- | --- |
| 正しさ・データ | SAFE-01, DATA-01 | DATA-02, CONF-01, ERR-01 | ASYNC-01 | — | [01-safety-and-correctness.md](01-safety-and-correctness.md) |
| セキュリティ・信頼境界 | CRED-01 | SEC-01〜SEC-07, SEC-09, SEC-10 | SEC-08 | — | [02-security-and-trust-boundaries.md](02-security-and-trust-boundaries.md) |
| バックエンド・データ設計 | — | ARCH-01, ARCH-02, ROUTE-01, DATA-03, SQL-01, AUTH-01 | ARCH-03, DEAD-01 | — | [03-backend-and-data-architecture.md](03-backend-and-data-architecture.md) |
| フロントエンド設計 | — | FE-01〜FE-05, UI-01 | FE-06〜FE-11 | — | [04-frontend-architecture.md](04-frontend-architecture.md) |
| パフォーマンス | — | PERF-01〜PERF-07, PERF-09, PERF-12 | PERF-08, PERF-10, PERF-11, PERF-13, PERF-14 | PERF-15 | [05-performance.md](05-performance.md) |
| 品質・運用・配布 | — | QUAL-01, QUAL-02, OPS-01, REL-01, REL-02, PKG-01 | DEP-01, DOC-01, BUDGET-01 | — | [06-testing-observability-and-delivery.md](06-testing-observability-and-delivery.md) |

実施順と依存関係は [07-execution-roadmap.md](07-execution-roadmap.md)、実装・検証証跡と残件は [08-implementation-report.md](08-implementation-report.md) にまとめる。

## 依頼観点との対応

| 観点 | 主なタスク群 |
| --- | --- |
| 技術的負債 | 手製 migration、SQLite-only資格情報の境界不足、CI 不在、旧 GPUI code/docs、重複 release workflow、未固定 build tool |
| 設計の不備 | 一律 IPC retry、暗黙の acting account、文字列／表示文言依存、8,201 行 command 層、巨大 store/component、信頼境界不足 |
| パフォーマンス | 全件 startup sync、直列 quote hydration、unbounded stream、個別 DB commit、LIKE 全走査、YQ 再評価、O(n²) frontend merge、無制限 cache |
| リファクタリング | 型付き IPC、use-case/repository 分離、protocol capability、entity store、feature slices/components、共通 lifecycle/error/i18n/test 基盤 |

## 横断的な完了条件

- 新しい境界には正常系だけでなく、タイムアウト、キャンセル、部分失敗、重複配送、破損データのテストがある。
- SQLite schema変更は原子的に適用し元DBを破損させない。未リリースのOS store方式からの互換移行・復旧経路は作らない。
- 性能改善は「速くなったはず」ではなく、固定データセットで before / after を記録する。
- IPC、ログ、サポート情報にアクセストークン、アプリパスワード、OAuth code、投稿本文の不要な全量を出さない。
- 一時的な互換レイヤーには削除条件と期限を付ける。

## 今回の対象外

- UI デザインの刷新や新規プロトコル機能の追加
- サーバーごとの API 仕様そのものの変更
- 計測なしの依存ライブラリ最新版追従
- 現在の保存値・外向き表示名を理由なく変更すること
