# Release runbook

## Repository controls

- `main` は pull request、`Quality Gate`、`Dependency Audit`、review を必須にする。
- `v*` tag は maintainer の署名付き annotated tag のみ許可する。
- `production-signing` と `production-publishing` GitHub Environment は別の必須承認者を持つ。
- build job は read-only token、publish job だけが `contents: write` を持つ。
- macOSはsecretなしでbundleを作るjobと、固定pathだけを署名・公証するjobを分ける。署名jobはcheckoutやrepository script、生成appを実行せず、keychain削除後の別runnerでlaunch smokeを行う。
- manual production build は `main` の HEAD だけを対象にする。

## Release

1. `Cargo.toml`、`package.json`、`tauri.conf.json` を同じ SemVer に更新する。
2. `bun run version:check` と quality gate を通す。
3. `git tag -s vX.Y.Z -m "Awayuki vX.Y.Z"` で署名し、tag を push する。
4. `Release` workflow の source verification、quality、共通 `build-artifacts.yml` による3 OS build、署名、公証を確認する。
5. macOS DMG、Windows ZIP、Linux AppImage の clean-runner package smoke と、clean Arch container の `makepkg` install / launch / uninstall が成功したことを確認する。
6. draft release の package、deterministic source archive、SPDX SBOM、artifact manifest、provenance を照合して公開する。
7. 公開後のappcast metadata jobがmanifestのdigest / size / version、EdDSA signature、downgradeを検証したことを確認する。アプリ自身はこのfeedを読まず、更新案内はGitHub Releasesで行う。

tag release と manual production build は、source selection だけが異なり、quality、platform packaging、artifact名、package smoke、manifest、SBOM、attestation は同じ reusable workflow と script を使う。manual build も `main` の immutable commit 以外は署名できない。

## Package smoke contract

共通build workflowはpublish可能になる前にmacOS、Windows、Linuxの全reportを要求する。
各OSは隔離した`HOME` / XDG / AppDataで次を同じ順序で実行する。

1. packageをinstallまたは展開し、fresh profileで起動する。
2. 現行SQLite migrationのcommit完了を待つ。
3. `awayuki.db`だけをsynthetic migration-019 DBへ差し替える。
4. 全migration、設定とSQLite内login rowの保持を検証し、もう一度再起動する。
5. install payloadを削除し、binaryの消失とuser data側`awayuki.db`の保持を検証する。

fixtureは別DBのbackup / recovery copyを拒否し、package scanは同梱DB、source、mock、
secret、build cacheを拒否する。OS credential storeは使わず、migration前backupも作らない。
各OSのJSONは`package-smoke-summary`で必須集約するため、未実行OSはskip扱いではなく
workflow failureになる。summaryはrelease artifactとworkflow summaryへ添付する。clean Arch
`makepkg`でもdistro packageに対してfresh / legacy / restart / uninstallを同様に検証する。

## Emergency stop and key rotation

`production-signing` / `production-publishing` Environment を停止し、影響する draft / release と
appcast entry を非公開にする。漏えい疑いの secret を失効し、新しい鍵を Environment に登録する。
appcast署名鍵はmetadata検証専用であり、アプリ内updaterの鍵ではない。Sparkle / WinSparkleは
OS preference / registry依存ごと削除済みで、SQLite-only contractを満たす代替設計が採択されない限り再導入しない。

監査では workflow run、environment approval、source commit、tag signature、artifact manifest、
GitHub release の asset digest を同じ release ID と共に保存する。
