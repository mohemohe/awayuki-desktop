# ADR-0001: SQLite-only portable state

- Status: Accepted
- Date: 2026-07-11

## Context

Awayukiは`awayuki.db`を移動するだけでログイン状態を含む全機能を移行できることを製品契約とする。
OS credential storeへ資格情報を分離すると、DBだけの移動では復元できず、DB rowとOS itemの
lifecycle不整合も発生する。

## Decision

資格情報、設定、column、cacheを含む永続状態はSQLiteだけへ保存する。Keychain、Windows
Credential Manager、Linux Secret Service、registry、別のsecret fileへ永続化しない。
token rotationとlogoutはSQLite transactionを中心に直列化する。
OS store方式は未リリースなので、その方式からSQLiteへ戻す移行・復旧codeも追加しない。
Sparkle / WinSparkleもOS preferenceやregistryへ状態を持つため使用せず、更新は全OSで手動とする。

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
