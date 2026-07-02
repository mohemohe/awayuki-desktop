<div align="center">
  <img src="https://i.imgur.com/NPHWZ2Y.png" height="128">
  <h1>Awayuki</h1>
</div>

![](https://i.imgur.com/GdHx6N5.png)

Krile 2, [Krile STARRYEYES](https://github.com/karno/StarryEyes) ライクな [Mastodon](https://github.com/mastodon/mastodon) / [Paon](https://github.com/mstdn-plusminus-io/paon) / [Misskey](https://github.com/misskey-dev/misskey) / [Bluesky](https://bsky.app/) クライアント。  
SQL または [YQ](https://github.com/shibafu528/Yukari/wiki/Yukari-Query) でカスタムタイムラインを作成可能。

設定でデッキ表示とシングル1行表示を切り替えることができます。

| ![](https://i.imgur.com/tCbTMCl.png) | <img src="https://i.imgur.com/67vQT4z.png" height="480"> |
| --- | --- |

## ダウンロード

[GitHub Releases](https://github.com/mohemohe/awayuki-desktop/releases) から最新版をダウンロードしてください。  
Windows, macOS (Apple Silicon), Linux で動作します。

macOS版はAppleによって公証されており、悪質なアプリケーションではないことが証明されています。  
Windows版は証明書が高いため、コード署名を行っていません。起動時に警告が表示される場合があります。  
Linux版はAppImage形式で提供しており、ほとんどのディストリビューションで動作します。

## ポータブルモード

実行ファイルと同じディレクトリに `PORTABLE` という名前のファイルがある場合、SQLiteのキャッシュDB `awayuki.db` とログファイル `awayuki.log` は実行ファイルと同じディレクトリから読み書きします。`PORTABLE` ファイルの中身は問いません。

`PORTABLE` がない場合は、従来どおりOS標準のアプリケーションデータディレクトリを使用します。

- Windows / Linux: `awayuki.exe` やAppImageなど、起動する実行ファイルと同じディレクトリに `PORTABLE` を置いてください。
- macOS: `/Applications/PORTABLE` や `Awayuki.app` と同じディレクトリの `PORTABLE` は参照しません。

## ビルド

### macOS

- Rust (stable)
- Bun
- Xcode.app (フルインストール、Command Line Toolsだけでは不可)

```bash
# Xcodeのパスを設定
xcode-select -s /Applications/Xcode.app/Contents/Developer

# Metal Toolchainのダウンロード（失敗する場合は素直にXcodeの設定からダウンロードすること）
xcodebuild -downloadComponent MetalToolchain

# Frontend assets
bun install
bun run build

# Release app bundle
./scripts/build-app-bundle.sh
```

### Arch Linux

```bash
git clone https://github.com/mohemohe/awayuki-desktop.git
cd awayuki-desktop
makepkg -s # または makepkg -si
```

## 技術スタック

| 領域 | ライブラリ |
|------|-----------|
| Desktop Shell | Tauri 2 |
| Frontend | React, TypeScript, Vite |
| UI | Tailwind CSS, DaisyUI, lucide-react |
| Timeline Virtualization | react-virtuoso |
| State Management | Zustand |
| Backend | Rust |
| Async | tokio, futures |
| HTTP / WebSocket | reqwest, tokio-tungstenite |
| DB | sqlx (SQLite) |
| Yukari Query | yqrs |
| Bluesky / AT Protocol | bsky-sdk, atrium-api |
| Logging | tracing, tauri-plugin-log |
| 自動更新 | sparkle-updater (Sparkle.framework) |

## ライセンス

WTFPLv2.0
