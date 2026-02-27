# awayuki

![](https://i.imgur.com/LcQnc5c.png)

軽量Mastodonクライアント。Rust + [GPUI](https://gpui.rs/)で構築。


## 必要環境

### macOS

- Rust (stable)
- Xcode.app (フルインストール、Command Line Toolsだけでは不可)

## セットアップ

### macOS

```bash
# Xcodeのパスを設定
xcode-select -s /Applications/Xcode.app/Contents/Developer

# Metal Toolchainのダウンロード（失敗する場合は素直にXcodeの設定からダウンロードすること）
xcodebuild -downloadComponent MetalToolchain
```

## ビルド・実行

```bash
# Debug build & run
cargo run

# Release build
cargo build --release

# ログレベル指定
RUST_LOG=awayuki=debug cargo run
```

## 技術スタック

| 領域 | ライブラリ |
|------|-----------|
| GUI | gpui, gpui-component |
| Async | tokio, gpui-tokio-bridge |
| HTTP | reqwest |
| WebSocket | tokio-tungstenite |
| DB | sqlx (SQLite) |
| 自動更新 | sparkle-updater (Sparkle.framework) |

## ライセンス

WTFPLv2.0
