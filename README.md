<div align="center">
  <img src="https://i.imgur.com/NPHWZ2Y.png" height="128">
  <h1>Awayuki</h1>
</div>

![](https://i.imgur.com/GdHx6N5.png)

Krile 2, [Krile STARRYEYES](https://github.com/karno/StarryEyes) ライクな [Mastodon](https://github.com/mastodon/mastodon) / [Paon](https://github.com/mstdn-plusminus-io/paon) クライアント。  
SQL または [YQ](https://github.com/shibafu528/Yukari/wiki/Yukari-Query) でカスタムタイムラインを作成可能。

設定でデッキ表示とシングル1行表示を切り替えることができます。

| ![](https://i.imgur.com/tCbTMCl.png) | <img src="https://i.imgur.com/67vQT4z.png" height="480"> |
| --- | --- |

## ダウンロード

[GitHub Releases](https://github.com/mohemohe/awayuki-desktop/releases) から最新版をダウンロードしてください。  
Windows, macOS (Apple Silicon) で動作します。

macOS版はAppleによって公証されており、悪質なアプリケーションではないことが証明されています。  
Windows版は証明書が高いため、コード署名を行っていません。起動時に警告が表示される場合があります。

## ビルド

### macOS

- Rust (stable)
- Xcode.app (フルインストール、Command Line Toolsだけでは不可)

```bash
# Xcodeのパスを設定
xcode-select -s /Applications/Xcode.app/Contents/Developer

# Metal Toolchainのダウンロード（失敗する場合は素直にXcodeの設定からダウンロードすること）
xcodebuild -downloadComponent MetalToolchain

# Release build
cargo build --release
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
