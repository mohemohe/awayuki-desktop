# Main WebView CSP policy

Awayukiの通常API通信、SQLite、media download、OAuthはRust側で行う。main WebViewが外部へ
直接接続する必要はなく、Tauri IPC originだけを`connect-src`へ許可する。Sidecarはmain
document内のframeではなく、IPC capabilityを持たない別native WebViewである。

| Directive | 許可 | 必要な経路 | 削除条件 |
| --- | --- | --- | --- |
| `default-src` | `'self'` | bundle内frontend | Tauri asset protocolへ完全移行した場合もdeny-default自体は残す |
| `connect-src` | `ipc:`、`http://ipc.localhost` | Tauri command/event bridge | Tauriが一方を使わなくなったことを3 OS package traceで確認したら個別に削除 |
| `img-src` | `'self'`、`http:`、`https:`、`data:`、`blob:` | avatar、custom emoji、protocol media、BlurHash、compose preview | 全remote imageをRust download/cache経由へ移し、Mastodon/Misskey/Bluesky fixtureが直接URLを返さなくなったら`http:`/`https:`を削除 |
| `media-src` | `'self'`、`http:`、`https:`、`blob:` | image/video preview、local compose preview | 全remote video/audioをRust streaming protocolへ移したら`http:`/`https:`、blob previewを廃止したら`blob:`を削除 |
| `style-src` | `'self'`、`'unsafe-inline'` | Reactの計算済みlayout/style属性 | inline styleをclass/CSS variableのnonce不要経路へ移したら`'unsafe-inline'`を削除 |
| `font-src` | `'self'` | bundle font | bundled fontを廃止した場合もdefault denyへ戻す |
| `script-src` | `'self'` | bundle内JavaScript | remote/inline/evalは追加しない |
| `base-uri`、`object-src`、`form-action`、`frame-src` | `'none'` | 不要 | 例外を追加しない。必要になった場合は別ADRと3 OS package traceを必須にする |

## Threat model and exception review

remote status HTMLがsanitizerをすり抜けても、任意fetch/WebSocket/form/frame/object/scriptで
外部へ情報を送れないことを境界とする。`img-src`と`media-src`のremote schemeはURLへのGET
自体を許すため、DOMへ渡すURLはprotocol DTOのmedia/avatar/emoji fieldだけに限定し、status
本文の任意attributeはsanitizerが除去する。credentialやSQLite内容をURLへ連結しない。

CSP例外の追加pull requestには、必要なresource type、対象provider/OS、CSP reportまたはpackage
trace、漏えい可能なデータ、より狭い代替案、削除条件をこの表または新ADRへ記録する。期限や削除
条件のないtemporary wildcardは受け入れない。

`bun run csp:check`はdeny directive、external connect禁止、remote image/media例外とこの文書の
削除条件に加え、production sourceのconsumer inventoryを`build/csp-source-inventory.json`へ出力する。
2026-07-12のinventoryでconsumerが存在しなかった`img-src asset:`、`media-src asset:`/`data:`、
`font-src data:`は削除した。実WebViewのCSP reportと3 OS media/Sidecar smokeは別のrelease証跡である。
