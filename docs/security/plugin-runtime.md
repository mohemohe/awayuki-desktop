# Plugin runtime trust boundary

Awayuki pluginはremote status contentとは異なり、ユーザーが意図して`plugins/`に置く
信頼済みの実行コードである。plugin marketplace、署名検証、permission prompt、自動updateは
version 1 APIのtrust boundaryには含まれない。入手先とsourceをreviewし、信頼できるpluginだけを
配置する。

## Exposed capabilities

pluginは次の能力を持つ。

- CreatePost、Boost、Favorite、Bookmark、DeletePostの直前と直後に、投稿や
  アカウントの情報を含むpayloadを読み書きする。
- Compose buttonのclick時に本文、CW、visibility、添付、poll、reply / quote / edit targetを
  含むdraftを読み書きする。click callback自体は投稿を開始しない。
- `fetch`でloopback / private networkを含むHTTP(S) endpointへrequestを送る。WebViewの
  `connect-src`やsame-origin policyはBoa runtimeに適用されない。Awayukiのlogin credentialを
  自動付与しないが、pluginがpayloadを
  request bodyやURLに入れることは防げない。
- `setTimeout` / `clearTimeout`、`setInterval` / `clearInterval`、`queueMicrotask`、
  Promise / async functionで後続jobを予約する。
- `console.log` / `info` / `warn` / `error`でSettingsのbounded in-memory logへ値を出力する。

Boa contextにNode.jsの`fs`、`process`、environment variable、Tauri IPC、SQLite handle、OS
credential APIは登録しない。pluginはAwayuki WebViewのDOMでも実行しない。これらの
非公開APIが無いことはnetwork漏えいやaction改変を防ぐsandboxを意味しない。
ES module loaderはplugin root内のimport先を読む。specifierの`..`正規化または絶対pathが
字句上root外へ解決するimportは拒否するが、plugin root内にユーザーが作成したsymlinkは
OSの通常のfile解決どおり追従する。リンク先がroot外でもpluginと同じ信頼済みcode / data境界に
入るため、信頼できない場所へのsymlinkをplugin directoryへ置かない。

## Installation and storage

plugin directoryは`awayuki.db`と同じstorage rootの`plugins/`である。通常のmacOS
releaseでは`~/Library/Application Support/awayuki/plugins`、Windows / Linuxのポータブル
モードでは実行ファイルの横のstorage rootになる。Settingsが表示する絶対pathを
実行中instanceの正本とする。

Awayukiはこのdirectory直下の`.js` / `.mjs`だけをentry moduleとして検出し、ファイル名
昇順でloadする。plugin sourceはAwayukiのSQLite永続状態ではない。DBをcopyしても
自動的にpluginは移行しない。ファイルを配付・backup・同期すると実行codeも配付する
ことになるため、他の設定fileより厳しく扱う。

## Failure and lifecycle properties

- module parse/evaluation、version、registrationの失敗は対象pluginを`error`にし、他のfileのloadを
  続ける。error textとconsole outputはSettingsで確認する。
- before hookのthrow、Promise rejection、object以外の返値はprovider APIを呼ぶ前に操作を
  中止する。これはプラグインが誤った対象や内容で送信するのを避けるfail-closed境界である。
- provider API成功後のafter hook失敗はログを残し、直前の有効な値を保って後続pluginを続ける。
  最終値を正規Statusへ戻せない場合はafter hook入力の元のstatusで成功を完了する。送信済みの投稿や
  actionをretry対象へ戻さない。
- compose callbackのthrow、Promise rejection、object / JSONとして扱えない返値はplugin logに残す。
  objectではあるがfield schemaが不正な返値はfrontendのapplication errorとして表示する。どちらも
  現在のdraftを変更しない。
- unloadはそのpluginのcontext、hook、未処理job/timer、compose functionを破棄し、buttonを
  Composeから除外する。reloadは新しいgenerationを作り、古いgenerationのclick結果を
  lifecycle完了後には適用しない。すでにruntimeが実行中のcallbackは、直列queue上のunload /
  reloadより先に完了することがある。メモリ上のlogやlifecycleは永続APIではない。

runtimeはPromise待機を30秒、JavaScript loopを1,000,000 iteration、console ringをpluginごとに
500件へ制限する。ただしこれはmemory quotaやnetwork process isolationではない。

組み込み`fetch`は接続10秒、redirectとresponse body読取りを含む全体25秒でtimeoutする。ただし
blocking requestの途中でAbort / unloadしても即時には割り込まず、同期JavaScriptを含む実行中のcodeを
OS threadの外から安全に即時終了させるemergency killにはならない。CPU、memory、network、
remote serviceへのside effectの設計とreviewはplugin author / installerの責任である。応答しない
pluginはAwayuki終了後にentry fileをdirectory外へ移動して無効化する。

## Review checklist

- source全体をreviewし、minified / obfuscated codeや不要なnetwork endpointが無いか。
- `fetch`へ本文、CW、accountなど外部へ出すべきでない情報を含めていないか。
- before hookが対象ID、本文、CW、visibilityを意図したように変えるか。
- after hookの変更がremote serverの再書き込みではなく、local result / cache / UIへの
  変換であることを理解しているか。
- timer、Promise、fetchにbounded completionと失敗処理があるか。
- unload / restartだけに頼らず、remote side effectを再実行しても安全な設計か。
