# Dependency update policy

Rust、Bun、GitHub Actions と build 時に実行する外部 binary は immutable version または
commit SHA と digest で固定する。`Cargo.lock` と `bun.lock` は application artifact の一部
として review し、CI / release は `--locked` / `--frozen-lockfile` を常に使う。

Dependabot と `Dependency Audit` workflow は毎週実行する。対応期限は次の通りとする。

| 種別 | 対応期限 |
| --- | --- |
| exploit が確認済み、または資格情報・更新経路に影響する critical | 24 時間以内に配布停止判断、72 時間以内に修正版または緩和策 |
| high advisory | 7 日以内 |
| medium / low advisory | 30 日以内、または理由を添えて `deny.toml` に期限付き例外 |
| 通常更新 | 月次 review |

例外には advisory ID、影響評価、代替防御、担当、削除期限を記載する。`cargo audit` と
`cargo deny` は advisory、license、重複、wildcard、registry / Git source を検査する。
Git dependency の rev 更新では upstream diff と license を review する。

## Feature surface metrics

`bun run dependency:metrics` は `Cargo.toml`、locked Cargo metadata、enabled feature
tree を `build/dependency-metrics.json` へ出力する。監査開始時点の比較可能な direct
feature surface は [`dependency-features-before.json`](../baselines/dependency-features-before.json)
に固定し、CI artifact は `before`、`current`、`delta` を同じ JSON に持つ。

現在のgateは利用者のbug report採取に必要なTauri `devtools`を必須とし、Tokio `full`、
reqwest default features、revのないGit dependency、Sparkle / WinSparkle dependencyの
再導入を拒否する。expanded feature
graph の行数と resolved package 数も trend として保存する。compile time、Rust binary、
Linux package の同一 runner 比較は performance workflow が担当し、feature 削減が別の
size / build metric を悪化させていないかを同時に確認する。
