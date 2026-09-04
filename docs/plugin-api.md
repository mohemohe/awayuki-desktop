# Awayuki plugin API version 1

Awayukiはstorage root直下の`plugins/`にある`.js` / `.mjs`をES moduleとして
[Boa](https://github.com/boa-dev/boa)で実行する。pluginはdefault exportに`version: 1`と
hook / compose buttonを持つobjectを返す。CommonJSの`module.exports` やフロントエンドの
global registrationは使わない。

## Install and lifecycle

plugin directoryは常に`awayuki.db`と同じdirectoryの`plugins/`である。

| 実行形態 | plugin directory |
| --- | --- |
| macOSの通常release | `~/Library/Application Support/awayuki/plugins` |
| Windows / Linuxの通常release | OS標準のAwayuki data directory内の`plugins/` |
| Windows / Linuxのポータブルモード | 実行ファイルの横の`awayuki.db`と同じdirectoryの`plugins/` |
| debug / local run | そのrunが表示するDB storage directory内の`plugins/` |

Settingsに表示される絶対pathが実行中instanceの正本である。directory直下の
`.js` / `.mjs`だけがentryで、ファイル名の辞書順でloadされる。multiple pluginに
同じhookがある場合もこの順序で、前のhookの返値が次のhookの引数になる。
plugin IDは拡張子を含むentryのファイル名そのものであり、IPCではopaqueな値として扱う。
entry moduleはplugin root内のfileをES module importできる。specifierの`..`正規化または
絶対pathが字句上plugin root外へ解決するimportは拒否される。ただしユーザーがplugin root内に
作成したsymlinkはloaderが追従するため、そのリンク先もpluginと同じ信頼境界として扱う。

`設定 > プラグイン` はdiscovered entryを`loaded` / `unloaded` / `error`で表示し、
pluginごとのconsole log、unload、reloadを提供する。unloadはhookとCompose buttonを外し、
reloadはfileを再読み込みして新しいgenerationを登録する。古いgenerationの非同期
Compose結果はlifecycle完了後には適用されない。runtimeはpluginごとに直列実行するため、
unload / reloadより先に受付済みのcallbackは、そのlifecycle処理より先に完了することがある。
`Reload all`はdirectoryを再scanするため、起動後に追加・削除したentryも一覧へ反映する。
console bufferはpluginごとに新しい500件だけをメモリに保持し、永続化しない。

## Minimal sample

[`examples/plugins/sample.mjs`](examples/plugins/sample.mjs)をplugin directoryへcopyすると使用できる。

```js
export default {
  version: 1,
  beforeCreatePost: (obj) => {
    if (obj.visibility === "public" && obj.text.includes("内緒")) {
      obj.visibility = "unlisted";
    }
    return obj;
  },
  registerComposeButtons: [
    {
      icon: "🥹​",
      onClick: (obj) => {
        obj.cw_title = "ぴえん";
        return obj;
      },
    }
  ],
};
```

`beforeCreatePost`は「内緒」を含むpublic投稿をunlistedに変える。Compose buttonは
CW欄を「ぴえん」に上書きするだけで投稿を開始しない。各callbackは引数を直接変更
できるが、変更後のobjectを必ず返す。`async` functionまたはPromiseを返してもよい。

## Runtime APIs

module evaluationとcallbackでは、standard ECMAScriptに加えて主に次を使用できる。

- Promise、`async` / `await`
- `console.log`、`console.info`、`console.warn`、`console.error`
- `fetch`
- `AbortController` / `AbortSignal`、`URL` / `URLSearchParams`
- `setTimeout`、`clearTimeout`、`setInterval`、`clearInterval`
- `queueMicrotask`
- `TextEncoder` / `TextDecoder`、`structuredClone`、`atob` / `btoa`

Awayukiはload中contextのBoa job queueとtimerをpumpするため、hookが返すPromise、
`fetch`、timerから予約したmicrotaskも実行される。`fetch`にWebView CSP / same-originは
適用されない。Node.jsの`fs`、`process`、`Buffer`、Tauri APIは提供しない。
安全な導入とnetworkの注意点は[plugin runtime trust boundary](security/plugin-runtime.md)を参照する。

module evaluationとcallbackが返すPromiseの待機上限は30秒で、JavaScript loopには1,000,000 iterationの
上限がある。組み込み`fetch`は接続10秒、redirectとresponse body読取りを含む全体25秒でtimeoutするが、
blocking requestの途中でAbort / unloadしても即時には割り込まない。これらは一般的なmemory quotaではない。

## Mutation hooks

| before hook | after hook | 対象 |
| --- | --- | --- |
| `beforeCreatePost` | `afterCreatePost` | 新規投稿の実配信。outboxへ入れた時点ではない |
| `beforeBoost` | `afterBoost` | Boost / Boost解除 (`reblog` / `unreblog`) |
| `beforeFavorite` | `afterFavorite` | Favorite / Favorite解除 (`favourite` / `unfavourite`) |
| `beforeBookmark` | `afterBookmark` | Bookmark / Bookmark解除 (`bookmark` / `unbookmark`) |
| `beforeDeletePost` | `afterDeletePost` | 投稿の削除 |

hookはfunction `(object) => object | Promise<object>` である。before hookの有効な返値が
provider APIで実行するrequest / targetを変える。after hookはremote操作成功後に呼び、
actionに応じて返値がcache、command result、Compose outbox success event、UIに渡る。
after hookはremote serverの完了済みobjectをもう一度更新するAPIではない。

### CreatePost payload

`beforeCreatePost`はIPCの`PostRequest`をcamelCaseで受け取り、本文の正規aliasとして
`text`を持つ。`status`はpayloadには含まれず、返値の`text`が実際の投稿本文になる。

| field | type | 意味 |
| --- | --- | --- |
| `text` | `string` | 投稿本文 |
| `visibility` | `"public" \| "unlisted" \| "private" \| "direct" \| null` | 投稿の公開範囲 |
| `spoilerText` | `string \| null` | CW見出し |
| `sensitive` | `boolean \| null` | 添付mediaのsensitive表示 |
| `mediaIds` | `string[] \| null` | upload済みmedia ID |
| `inReplyToId` | `string \| null` | reply先のprovider-local ID |
| `inReplyToIdentity` | `object \| null` | reply先のprovider-neutral identity |
| `quoteId` | `string \| null` | quote先のprovider-local ID |
| `quoteIdentity` | `object \| null` | quote先のprovider-neutral identity |
| `poll` | `{ options: string[], multiple: boolean, expiresIn: number } \| null` | poll |
| `actingAccountAcct` | `string` | 操作account。読み取り専用 |
| `operationId` | `string \| null` | outbox / cancellation用identifier。読み取り専用 |
| `_awayukiAction` | `"create"` | hook category metadata。読み取り専用 |
| `_awayukiActingAccountAcct` | `string` | 操作account metadata。読み取り専用 |

`actingAccountAcct`、`operationId`、`_awayukiAction`、`_awayukiActingAccountAcct`は
プラグインの返値から復元されるため、書き換えても実際のactorやoperation identityは変わらない。
`visibility`は本文presetを適用した後の実効値としてbefore hookへ渡る。hookが返した値が最終的な
provider requestになり、`null`を返した場合はprovider側のdefaultへ委ねる。

`afterCreatePost`はprovider成功後の正規化されたStatus objectと、同じ`create` metadataを受け取る。
outboxは実配信の各attemptでbefore hookを実行する。after hookはproviderが成功したattemptだけで実行し、
その返値が成功snapshotと`compose-outbox-updated`のstatusになる。plugin authorはretryの可能性を
考慮し、before hookのside effectをidempotentにする。

### Status payload

CreatePostのafter hookとその他のbefore / after hookは、providerから正規化された共通Statusを
snake_caseで受け取る。主なfieldは次のとおり。

```text
id, uri, url, created_at, edited_at, account, content, visibility, sensitive,
spoiler_text, media_attachments, mentions, tags, emojis, reblogs_count,
favourites_count, replies_count, in_reply_to_id, in_reply_to_account_id,
reblog, language, pinned, favourited, reblogged, muted, bookmarked, poll,
card, application, quote_id, quote, quote_original_url, pleroma
```

`account`と入れ子のStatusも同じsnake_caseである。media attachmentのkindは`type`、
cardのkindも`type`としてserializeされる。nullable / optionalのfieldはproviderやactionにより
`null`または既定defaultになる。unknown fieldはStatusへ戻す際に無視される。

すべてのStatus payloadには次のmetadataが加わる。

| field | 意味 |
| --- | --- |
| `_awayukiAction` | `create`、`reblog`、`unreblog`、`favourite`、`unfavourite`、`bookmark`、`unbookmark`、`delete`のいずれか |
| `_awayukiActingAccountAcct` | 操作するlogin accountのacct |

解除も同じhook categoryを使うため、pluginは`_awayukiAction`で分岐する。これらのmetadataは
返値をStatusへ戻す際に無視され、actorやactionは変更できない。action / deleteのbefore hookで
`id`を変更するとproviderへの対象IDが変わる。after hookの変更はそのstatusがcache / UIで
使われる方法を変える。DeletePostのafter hookは削除前のsnapshotに対して実行されるが、
成功済みのremote削除を取り消さない。DeletePostはstatusをIPCへ返さず、返値の`id`は元のidentityに
加えてlocal cacheから取り除く対象として使われる。

### Hook error semantics

- before hookがthrowする、Promiseがrejectする、またはobject以外を返すと、providerを呼ばず
  操作は失敗する。
- provider成功後のafter hookがthrowするかPromiseがrejectすると、エラーをplugin logへ残し、
  直前の有効な値を保って後続pluginを続ける。最終値を正規Statusへdeserializeできない、または
  `id`が空の場合はafter hook入力の元Statusを使う。Create / actionではproviderの返値、Deleteでは
  before hook変換後の削除前snapshotである。remote successを失敗に変えない。
- hook定義がないpluginはそのcategoryで何も変更しない。

## Compose buttons

`registerComposeButtons`はbutton descriptorの配列である。

```js
registerComposeButtons: [
  {
    icon: "✨",
    onClick: async (compose) => {
      compose.text = `[${new Date().toISOString()}] ${compose.text}`;
      return compose;
    },
  },
],
```

| descriptor field | type | 意味 |
| --- | --- | --- |
| `icon` | `string` | Composeにtextとして描画するicon |
| `label` | `string \| undefined` | accessibility labelとtooltipに使う任意の短い名前 |
| `onClick` | `(compose) => compose \| Promise<compose>` | click callback |

callbackの入力と返値は次のsnake_case compose draftである。

| field | type | 意味 |
| --- | --- | --- |
| `text` | `string` | 投稿本文 |
| `cw_enabled` | `boolean` | CW入力の有効状態 |
| `cw_title` | `string` | CW見出し |
| `visibility` | `"public" \| "unlisted" \| "private" \| "direct"` | Composeに表示し、次の送信で使う明示visibility |
| `sensitive` | `boolean` | mediaのsensitive状態 |
| `media_ids` | `string[]` | upload済みmedia ID |
| `poll` | `{ options: string[], multiple: boolean, expires_in: number } \| null` | poll draft |
| `target` | `{ kind: "reply" \| "quote" \| "edit", status: object } \| null` | 現在のCompose target |

camelCaseの`cwEnabled`、`cwTitle`、`mediaIds`とpollの`expiresIn`も互換aliasとして受け付けるが、
version 1の正規field名は表のsnake_caseである。
`target.status`だけはfrontendの`TimelineStatus`をそのまま受け渡すため、`statusIdentity`、
`originalStatusId`、`serverDomain`などのcamelCaseであり、mutation hookのsnake_case Statusとは別契約である。

`onClick`は引数の一部を変えて同じobjectを返すか、変更するfieldだけのpartial objectを返す。
正常な返値は現在のComposeに上書きされるだけで、`enqueue_post_status`やprovider APIを
呼ばない。実際の投稿は引き続きユーザーがComposeの送信操作で行う。throw、Promise rejection、
object / JSONとして扱えない返値はplugin logへ記録し、Composeを変更しない。objectではあるが
field schemaが不正な返値はfrontendのapplication errorとして表示し、同様にComposeを変更しない。

`cw_enabled`が入力時の値のままで、callbackが空ではない別の`cw_title`へ変更した場合は、
CW見出しの変更を画面と送信内容へ反映するためAwayukiがCW入力を有効にする。したがって上記の
sampleのように`cw_title`だけを変更しても「ぴえん」が表示される。明示的な
`cw_enabled: true`も従来どおり利用できる。

`media_ids`は現在添付済みのIDだけを並べ替える、または取り除くことができ、未知のIDを追加して
uploadを迂回することはできない。`poll: null`はpollを解除し、`target: null`は現在のreply / quote /
edit targetを解除する。

media upload中はCompose buttonを実行できない。async callbackの待機中に本文、CW、poll、visibility、
target、または添付ID・順序が変わった場合、古い入力全体で新しい編集を上書きしないよう返値を破棄し、
application errorとして表示する。

Composeのvisibility presetがプラグインの返値と競合する場合は、プラグインが返した
明示`visibility`がpresetより優先される。その後ユーザーがvisibilityを手動変更した場合は手動値が
次の送信値になる。accountまたはCompose targetが外部から切り替わるとplugin overrideを破棄する。
edit targetではprovider契約に従い、元の投稿のvisibilityを維持する。
pluginがreply targetを返した場合も通常のReply操作と同じくtarget statusのvisibilityを初期値にし、
target解除時に直前の選択へ戻す。edit targetはtarget statusのvisibilityで表示と送信を固定する。
Awayukiが対応しないvisibilityを持つreply / edit target返値はCompose全体へ適用せず拒否する。
