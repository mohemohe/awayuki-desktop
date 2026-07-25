# Release artifact verification

Release には各 package、同じcommitから作るdeterministic source archive、`artifact-manifest.json` を添付する。manifest は source commit、
Rust/Bun、lockfile digest、artifact size、SHA-256、署名方針を記録する。

```bash
gh release download v0.7.1 --dir artifacts
bun scripts/artifact-manifest.mjs verify artifacts \
  artifacts/artifact-manifest.json 0.7.1 <source-commit>
```

macOS packageは`Sparkle.framework`を同梱し、`codesign --verify --deep --strict`と
`spctl --assess --type open`で署名・公証を確認する。Windows packageは`awayuki.exe`と公式x64
`WinSparkle.dll`を同梱する。両OSとも公開releaseのappcastから更新を通知する。
Windows packageはコード署名が未導入
なのでmanifest上も`disabled-unsigned`とする。Linux AppImageはmanifest
のSHA-256を照合する。

署名鍵は GitHub Environment の承認付き job だけで利用し、repository、log、artifactへ
出力しない。鍵漏えい時は publish environment を停止し、該当 release と appcast を撤回、
新しい鍵で public key を更新してから既知の commit を再buildする。

CIの `package-smoke.sh` はpackageを一時directoryへ展開し、source tree、mock、credential、DB、
build cacheが混入していないことを確認してから、隔離した一時HOMEで起動する。Arch packageは
clean container上で `makepkg`、install、起動、uninstallまで検査する。
