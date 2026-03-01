# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

awayuki — macOS向け軽量Mastodonクライアント。Rust + GPUI（Zed Editor由来のGUIフレームワーク）で構築。TweetDeckライクなマルチカラムUIを持ち、Pleroma/Akkoma互換。

## Build & Run

```bash
cargo build          # Debug build
cargo build --release
cargo run            # Debug run (RUST_LOG=awayuki=debug で詳細ログ)
```

**前提条件**: フルXcode.appが必要（Command Line Toolsだけでは不可、Metal shaderコンパイルに必要）
```bash
xcode-select -s /Applications/Xcode.app/Contents/Developer
xcodebuild -downloadComponent MetalToolchain
```

テストは未整備。

## Architecture

### GPUI固有パターン
- エントリポイント: `Application::new().run(|cx: &mut App| { ... })`
- tokio連携: `gpui_tokio_bridge::init(cx)` → `Tokio::spawn(cx, async { ... })` で非同期タスク起動（`&mut Context<T>` が必要、Entity内からのみ使用可能）
- `Context::spawn` クロージャ: `async |this: WeakEntity<T>, cx: &mut AsyncApp| { ... }`
- `WeakEntity::update`: `this.update(cx, |this, cx| { ... })` — cxを直接渡す
- HTTPクライアント: `ReqwestHttpClient`がGPUIの`HttpClient` traitを実装（内部に専用tokio runtime保持）
- アセット: `CombinedAssets` — カスタムSVGアイコン優先、gpui-component-assetsにフォールバック

### モジュール構成
- **`bridge/`** — GPUI↔tokioブリッジ（runtime初期化、HTTPクライアント）
- **`mastodon/`** — Mastodon API層。`client.rs`(認証付き/未認証HTTPクライアント)、`endpoints/`(REST API)、`types/`(レスポンス型)、`streaming.rs`(WebSocket接続・自動再接続)、`oauth.rs`
- **`auth/`** — セッション管理(`SessionManager`: 複数アカウント対応)、OAuthコールバックサーバー、クレデンシャル保存
- **`services/`** — ビジネスロジック。`timeline_service`(REST取得→DB保存)、`streaming_service`(WebSocket→DB保存→GPUIパネルへブロードキャスト)
- **`db/`** — SQLite(sqlx)デュアルプール構成（writer×1, reader×CPU数）。WALモード。マイグレーションは`migrations/`に連番SQL。マイグレーションは必ず `src/db/pool.rs` の `alter_migrations` 配列に明示的に追加しなければならない。
- **`ui/`** — `workspace.rs`(メインUI・状態遷移管理)、`views/`(ログイン/設定画面)、`panels/`(タイムライン/アカウント詳細)、`components/`(ステータス表示)
- **`state/`** — `AppState`(Global: DB参照)、`WindowState`(ウィンドウ位置永続化)

### データフロー
WebSocketストリーミング → `streaming_service` → DB保存 + `futures::channel::mpsc` → 各`TimelinePanel`が受信・UI更新

### テーマ
Catppuccin Mochaベース。`main.rs`でGPUI Theme globalを直接カスタマイズ。

### 自動更新
macOS: Sparkle.framework (`sparkle-updater` crate)
