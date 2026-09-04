# ADR-0004: Boa plugin runtime

- Status: Accepted
- Date: 2026-08-30

## Context

Awayukiの投稿・Boost・Favorite・Bookmark・削除をユーザーがECMAScriptで変換し、
Composeにカスタムbuttonを追加できるextension pointが必要である。プラグインはフロントの
WebViewで`eval`せず、provider mutationの実際の境界で共通に実行する必要がある。

ECMAScriptのPromise、`fetch`、timer、microtaskをサポートしつつ、Boa `Context`の
`Send` / `Sync`に依存せずTauri / Tokio runtimeと明確に分離する必要がある。また、
plugin sourceをDB BLOBとして管理すると、ユーザーが導入・review・無効化する実行コードと
AwayukiのSQLite-only永続状態が混在する。

## Decision

- `boa_engine` と `boa_runtime` でversion 1のECMAScript module APIを実行する。
- DBと同じstorage rootの`plugins/`直下にある`.js` / `.mjs`をファイル名昇順で
  検出する。それぞれを独立したBoa contextにES moduleとしてloadする。
- Boa contextとJavaScript functionは専用OS threadが所有する。Tauri commandはmessage channelで
  snapshot、reload、unload、hook、compose clickを要求する。
- runtimeに`console`、`fetch`、`setTimeout` / `clearTimeout`、`setInterval` / `clearInterval`、
  `queueMicrotask`を登録し、
  module evaluationとhookが返すPromiseに加え、load中contextのjob queueとtimerを継続してpumpする。
- `beforeCreatePost` / `afterCreatePost`、`beforeBoost` / `afterBoost`、
  `beforeFavorite` / `afterFavorite`、`beforeBookmark` / `afterBookmark`、
  `beforeDeletePost` / `afterDeletePost`をprovider mutationの共通境界で呼ぶ。ファイル名順に
  返値を次のhookへ渡す。Boost、Favorite、Bookmarkは解除アクションにも同じcategoryの
  hookを呼び、payload metadataが正確な操作を示す。
- before hookのthrow、rejected Promise、またはobjectでない返値はremote request前に操作を
  失敗させる。remote成功後のafter hook失敗はログへ記録し、直前の有効な値を保って後続pluginを
  続ける。最終値を正規Statusへ戻せない場合はafter hook入力の元の値を使う。外部で成功したmutationを
  retry可能な失敗に変えない。
- `registerComposeButtons` のdescriptorだけをfrontendへ渡し、functionはBoa contextに保持する。
  clickは現在のcompose draftを受け、返されたdraftでcomposeを上書きするだけで、投稿しない。
- unloadはcontext、hook、job/timer、compose functionを破棄し、button descriptorを除外する。
  reloadはgenerationを更新し、lifecycle完了後の旧generation clickを拒否する。runtimeは直列なので、
  すでに実行中のclick callbackは後続のunload / reloadより先に完了し得る。
- plugin sourceはexternally managed executable inputであり、SQLiteへ保存しない。lifecycle状態と
  bounded console logもメモリ内だけで管理する。

## Rejected alternatives

- WebView上の`eval` / dynamic script injectionはmain documentのCSP、DOM、Tauri IPCとpluginを
  同じtrust boundaryに入れるため採用しない。
- Node.js / Deno sidecarは配布runtime、subprocess、filesystem APIの攻撃面を増やすため採用しない。
- plugin sourceのSQLite BLOB保存はreview可能なfile lifecycleを失い、DB状態と実行コードを
  混同するため採用しない。
- protocol adapterごとにhookを実装する方式は同じpluginの意味がproviderごとにずれ、
  outbox retryを含む共通mutation boundaryを外すため採用しない。

## Data, compatibility, and rollback

plugin directoryが無い、または対象fileが無い場合の挙動はplugin runtime導入前と同じで、
database migrationは発生しない。APIは`export default { version: 1, ... }`を要求し、非対応versionは
そのfileだけを`error`として他のpluginをloadする。

機能rollbackはSettingsからunloadするか、Awayukiを終了して対象fileを`plugins/`から
移動する。runtime自体を差し戻してもSQLite schemaと既存データは変わらない。DBとplugin
sourceは別コンポーネントとしてbackup / transferする。

## Security

pluginは信頼できるユーザー導入codeとする。Boa contextへfilesystem、environment、
process、SQLite、Tauri IPC、OS credential APIを直接exposeしない。ただしpluginは投稿・account・
compose情報をhook payloadから読み、それを`fetch`で任意のremote endpointへ送れる。
WebView CSPはRust内のBoa通信に適用されない。詳細は[`security/plugin-runtime.md`](../security/plugin-runtime.md)
を正本とする。

## Verification strategy and remaining work

現行のfocused testはstorage locationからのplugin directory導出、direct `.js` / `.mjs`の
ファイル名順discovery、version / module errorの分離、reload / unload、console取得と500件ring上限、
compose button generation、synchronous / Promise hook、`fetch`、timer、microtask、multiple plugin
chainを確認する。Frontend testはSettingsのloaded / unloaded / errorとlog表示、reload / unload、
Composeの取得・上書き・非投稿、lifecycle完了後のstale generation破棄を確認する。

残る回帰test戦略は次のとおり。

- 各before / after mutation hookをprovider fakeで実際のCreatePost、Boost、Favorite、Bookmark、
  DeletePost境界まで通し、before failureでremote未実行、after failureでremote成功を維持すること。
- `setInterval`を含む未処理job / timerのunload、root外specifier拒否を専用runtime testで
  固定すること。plugin root内のユーザー作成symlinkを追従する挙動は
  trust-boundary documentationとして維持する。
- merge前にRust fmt / clippy / all-target test、frontend typecheck / lint / test / buildを完走すること。
