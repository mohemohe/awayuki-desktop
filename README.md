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

macOS版は開発者署名とAppleの公証を行っています。これは発行者とAppleの自動検査を確認する仕組みであり、アプリの完全な安全性を保証するものではありません。

自動更新frameworkはOS側へ設定を永続化するため全OSで削除済みです。更新はGitHub Releasesから手動で行います。Windows版は現在コード署名を行っていないため、起動時に警告が表示される場合があります。
Linux版はAppImage形式で提供しており、ほとんどのディストリビューションで動作します。

配布版でもDevToolsを意図的に有効化しています。利用者からconsole/network情報を含む
bug reportを受け取るための診断機能であり、release buildから削除しません。

## ポータブルモード

実行ファイルと同じディレクトリに `PORTABLE` という名前のファイルがある場合、SQLite DB `awayuki.db` と任意の診断ログ `awayuki.log` は実行ファイルと同じディレクトリから読み書きします。ログは動作状態ではなく、削除・未移動でも機能やログイン状態へ影響しません。`PORTABLE` ファイルの中身は問いません。

`PORTABLE` がない場合は、従来どおりOS標準のアプリケーションデータディレクトリを使用します。

ログイン資格情報を含む永続データは `awayuki.db` のみに保存され、OS の Keychain、Credential Manager、Secret Service、registry などには保存しません。別DBへの自動バックアップや、未リリースのOSストア方式からの移行・復旧経路も作りません。そのため `awayuki.db` を対応するデータディレクトリへ移動すれば、ログイン状態を含めて移行できます。DB には再利用可能な資格情報が含まれるため、共有・公開せず安全に保管してください。

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
BUILDDIR="$PWD/build/makepkg" makepkg -s
# インストールまで行う場合:
BUILDDIR="$PWD/build/makepkg" makepkg -si
```

`BUILDDIR` は必須です。未指定時の `makepkg --cleanbuild` が生成用の
`$srcdir` と Rust の `./src` を同じ場所として扱い、ソースを削除するためです。

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
| 更新 | GitHub Releasesからの手動更新（全OS） |

## ライセンス

WTFPLv2.0
