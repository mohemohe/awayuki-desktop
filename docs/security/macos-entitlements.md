# macOS entitlements

Awayuki の配布版は Hardened Runtime を有効にし、`resources/Entitlements.plist` の
次の二つだけを許可する。

| entitlement | 理由 | 削除条件 |
| --- | --- | --- |
| `com.apple.security.cs.allow-jit` | WKWebView の JavaScript 実行を Hardened Runtime 下で維持するため。macOS の署名済み smoke test で起動、ログイン、timeline、sidecar を確認する。 | 対応する WebKit / Tauri が JIT entitlement なしで同じ smoke test を通ることを確認できた時。 |
| `com.apple.security.network.client` | Mastodon、Misskey、Bluesky API と remote media への outbound 通信に使用する。 | 通信を全て別プロセスへ分離した時。 |

`allow-unsigned-executable-memory` と `disable-library-validation` は、より広い実行・動的
library 読み込みを許すため削除した。埋め込み framework と helper は app と同じ signing
identity で署名し、`codesign --deep --strict` で検証する。

Release job は署名後に entitlement を `build/macos-entitlements.plist` へ抽出し、静的な
allowlist と一致することを検証する。この snapshot は artifact manifest と共に保存する。
許可を追加する変更では、必要な runtime 操作、脅威、削除条件をこの表へ追記する。
