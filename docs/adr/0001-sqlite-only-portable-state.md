# ADR-0001: SQLite-only portable state

- Status: Accepted
- Date: 2026-07-11

## Context

Awayukiは`awayuki.db`を移動するだけでログイン状態を含む全機能を移行できることを製品契約とする。
OS credential storeへ資格情報を分離すると、DBだけの移動では復元できず、DB rowとOS itemの
lifecycle不整合も発生する。

## Decision

資格情報、設定、column、cacheを含むAwayukiの永続状態はSQLiteだけへ保存する。Keychain、
Windows Credential Manager、Linux Secret Service、registry、別のsecret fileへ永続化しない。
token rotationとlogoutはSQLite transactionを中心に直列化する。
OS store方式は未リリースなので、その方式からSQLiteへ戻す移行・復旧codeも追加しない。
`PORTABLE`マーカーによる実行ファイル隣接保存はWindows / Linuxだけの仕様とする。macOSでは
マーカーを探索せず、release buildは通常OS標準のApplication Support配下を使用する。
OS data directoryを取得できない場合は、既存の`$HOME/.awayuki`、続いてcurrent directoryへの
fallbackを維持するが、`PORTABLE`マーカーで保存先を切り替えない。
macOSのSparkleとWindowsのWinSparkleは更新通知のために使用する。OS preferenceやregistryへ
保存される更新確認日時・通知設定はAwayukiのユーザーデータではなく、`awayuki.db`の移動による
ログイン状態・設定・Timelineの移行要件には含めない。

Boa pluginのECMAScript sourceは、ユーザーが別途導入する実行コードであり、
Awayukiが生成・保存する永続状態ではない。`plugins/` は配置場所を一意にするため
DBと同じstorage rootの下に置くが、source本体、load/unload状態、console logを
SQLiteの代替永続storeとして使わない。DBだけを移動する契約は引き続きログイン状態、
設定、Timelineを対象とし、同じextension環境も必要な利用者はplugin sourceを別にcopyする。
この例外の詳細は[ADR-0004](0004-boa-plugin-runtime.md)で定める。

DB、WAL、SHMはprivate permissionで作成し、log/IPC/support bundleではtoken、password、
OAuth code、本文、任意pathをredactする。別DB copyや自動backupは作らない。READMEはDBに
再利用可能な資格情報が含まれることを明示する。

## Consequences

単一file portabilityを維持できる一方、DBを取得した主体は資格情報を再利用できる。OS user境界、
disk encryption、安全なtransferが防御となる。databaseをsupport artifactへ自動添付しない。

## Verification

- DB fileを移動・再openしてaccess tokenとBluesky app passwordを復元するtest。
- login / rotation / logout transaction test。
- OS credential-store dependencyがmanifest/lockfileへ入らない検査。
- private file mode、redaction、別永続fileを作らないcontract test。
- macOS buildでは`PORTABLE`マーカーを探索せず、Windows / Linuxだけがマーカーを解釈するtest。
- plugin source、plugin lifecycle、console bufferがSQLite以外のAwayuki永続状態にならないcontract test。
