# KQ (Krile Query) — Awayuki 実装リファレンス

KQ (Krile Query) は、source と中置記法の predicate を組み合わせてタイムラインを絞り込むクエリ言語である。
Awayuki の実装は **StarryEyes KQ 互換**を基本としながら、文字列 ID、完全修飾 acct、visibility、CW、
メディア、投票、引用などの Fediverse データを扱えるように拡張している。

本実装は MIT License の StarryEyes
`a2c4c9b68287c9058d82a15cd28c6615863a626f` を調査・移植元としている。著作権表示、MIT License
本文、上流 URL は [LICENSES/StarryEyes-MIT.txt](../LICENSES/StarryEyes-MIT.txt) に収録している。

本書は Awayuki の契約を記述する。StarryEyes の古い `kql.txt` にあった構文や、Twitter でのみ成立した
取得・関係データの挙動をそのまま約束するものではない。

---

## クエリ全体の構文

```ebnf
query       := "from" source-list ["where" [expression]]
             | "where" expression
source-list := source { "," source } [","]
source      := source-name [":" source-arg { "," source-arg }]
             | source-name "(" [source-arg { "," source-arg } [","]] ")"
source-arg  := string | account | opaque-id | integer | identifier | "*"
```

代表例:

```kq
from home:"alice@example.social" where text contains "Rust" & visibility == "public"
from home:"alice@example.social",list:"alice@example.social/42" where has_media
where user.acct == "alice@example.social"
```

- `where` だけで始めると、暗黙に `from local` となる。
- `from ...` の後に `where` がなければ predicate は `()`、すなわち常に真となる。
- StarryEyes 互換の `from local where` も空 predicate `()` として受理する。一方、`where` だけの場合は
  predicate が必須である。
- 空または空白だけの query は compile error となる。
- source が複数ある場合は source branch 同士を OR で結合し、各 branch の一致条件に `where` predicate を
  AND する。正確な評価形は `OR(branch が source に一致 AND branch scope で predicate が true)` である。
- `from`、`where`、source 名、フィールド名、単語形式の演算子は大文字小文字を区別しない。
- source 引数の canonical 形式は `home:"acct"` のように `:` と引用文字列で指定する。上流資料との
  互換用に `home("acct")` 形式も受理する。本書と新規クエリでは colon 形式を推奨するが、保存済みの
  入力文字列を自動で書き換えることは約束しない。
- Awayuki 拡張として source 引数には quoted string のほか、acct、opaque ID、整数、単純 identifier、`*`
  も受理する。空白や `/` を含む provider ID は引用する。
- colon 形式で複数引数を続けられるのは quoted string、`@acct`、`#opaque-id`、integer、`*` である。
  bare identifier は最初の一引数には使えるが、後続の `,name` は次の source と区別できないため、複数の
  bare 値は引用するか `source(a,b)` の call 形式を使う。
- StarryEyes 互換として source list、call-form 引数、set literal の末尾 comma を受理する。同一 source
  branch は compile 時に重複除去する。account-bound source の引数 `*` は引数省略と同じ全 account を表し、
  同じ source call の他の account 引数より優先する。
- `from *` は `from all` と同じ意味で受理する。StarryEyes 側の `from *` の実装不具合は再現しない。
- predicate だけを `from` や `where` なしで書く形式は KQ ではない。

文字列は `"..."` で囲む。`\"` と `\\` をエスケープできる。閉じていない文字列、型が合わない演算、
未知の source・フィールドは compile error となり、実行は開始しない。

## 値と識別子

KQ の値は Boolean、符号付き整数、String、identity/set、および Missing を区別する。

| 構文 | 意味 |
|------|------|
| `"text"` | String |
| `123`, `-123` | 算術用整数 |
| `true`, `false` | Boolean |
| `(...)` | グループ化 |
| `()` | 常に真 |
| `[]`, `[a, b]` | empty / non-empty set |
| `@alice` | acct identity |
| `@alice-smith@sub.example.social` | 完全修飾 acct identity |
| `@"alice@example.social:8443"` | 記号やポートを含む完全修飾 acct identity |
| `#123` | provider opaque ID identity |
| `#"did:plc:abc..."` | 記号を含む provider opaque ID identity |
| `we`, `our`, `us`, predicate 内の `*` | Awayuki にログイン済み account の canonical acct と account ID の identity set |

Fediverse の status/account ID は数値ではなく **opaque string** として扱う。Mastodon の十進 ID、UUID、
Misskey ID、DID、AT URI を `i64` へ変換しない。互換性のため `id == 123` は ID 文脈に限り文字列
`"123"` との比較として扱うが、ID に対する `<`、`>` や算術は compile error となる。opaque ID は
case-sensitive に完全一致させる一方、acct/handle の比較は Unicode case-folding を行う。
引用しない integer literal は signed 64-bit の全範囲を受理する。範囲を超える十進 ID は
`id == "18446744073709551615"`、account ID literal なら `#"18446744073709551615"` のように引用する。

acct は provider-aware な identity である。ActivityPub のローカル account、完全修飾 acct、Bluesky の
ドットを含む handle を一律に `username@server_domain` へ書き換えない。曖昧さを避けるには、完全修飾
acct literal または `author.acct == "..."` を使う。

---

## 演算子、優先順位、結合規則

優先順位は次の表の下ほど高い。同じ優先順位の **すべての二項演算子は右結合**である。
例えば `10 - 3 - 2` は `(10 - 3) - 2` ではなく `10 - (3 - 2)`、すなわち `9` になる。

| 優先度 | 演算子 | 意味 |
|---:|---|---|
| 0 | `\|`, `\|\|`, `or` | Boolean OR |
| 1 | `&`, `&&`, `and` | Boolean AND |
| 2 | `=`, `==`, `!=` | 等値・不等値 |
| 3 | `<`, `<=`, `>`, `>=` | 数値比較 |
| 4 | `->`, `contains` | 文字列の部分一致、set の包含・共通要素判定 |
| 4 | `<-`, `in` | identity の set 所属、set の共通要素判定 |
| 4 | `startswith`, `startwith` | 文字列の前方一致 |
| 4 | `endswith`, `endwith` | 文字列の後方一致 |
| 4 | `match`, `regex` | 正規表現 |
| 5 | `+` | 数値加算、文字列結合、set 和 |
| 5 | `-` | 数値減算、set 差 |
| 6 | `*` | 数値乗算、set 積 |
| 6 | `/` | 整数除算 |
| 7 | `!`, `not` | Boolean NOT |
| 7 | 単項 `-` | 数値の符号反転 |
| 7 | `caseful` | 対象文字列を大文字小文字区別ありにする |
| 8 | 値、`(...)`、`[...]` | primary |

`&`、`|` は短絡評価する。記号形式が StarryEyes 互換 KQ の canonical 構文で、`and`、`or`、`not` は
Awayuki が受理する可読性向けの拡張である。
文字列の等値、部分一致、前方一致、後方一致は既定で大文字小文字を区別せず、`caseful` を付けた値との
比較だけ区別する。正規表現は常に大文字小文字を区別し、`caseful` の影響を受けない。右辺には固定の
quoted string literal が必要で、Rust regex の規則に従う。不正な pattern や動的 pattern は compile error
となる。大小文字を区別しない regex が必要なら pattern 内で `(?i)` を明示する。
set 同士の `==` / `!=` は定義せず compile error とする。所属・共通要素判定には `contains` / `in` を使う。

### Missing の三値論理

フィールド自体は Awayuki が対応していても、provider が値を返さない、関連する元投稿が cache にない、
viewer account が一意に決まらない場合、その値は `Missing` となる。

- `Missing == x`、`Missing != x`、数値・文字列・set 演算の結果は Missing。
- `!Missing` も Missing。したがって unscoped な `!viewer.bookmarked` で欠落 viewer state を拾うことはない。
- `false & Missing` は false、`true & Missing` は Missing。
- `true | Missing` は true、`false | Missing` は Missing。
- 最終結果が true の status だけが一致する。false と Missing は一致しない。
- 整数の overflow、0 除算、型の合わない動的値は Missing となり、scan を停止させない。
- `has_media`、`has_poll`、`has_card`、`has_cw` のような存在判定は、正常に取得できた status に
  対象データがなければ false となる。関連 status や JSON 自体を解決できない場合は Missing となる。

Awayuki のモデルに存在しない Twitter 専用フィールドは Missing へ暗黙変換せず、compile error にする。
これにより、スペルミスや移植不能なクエリが「一致 0 件」として見逃されない。

---

## FROM source

### 共通の取得境界

KQ の検索は Awayuki の **SQLite cache のみ**を読む。KQ の実行を理由に REST 検索、過去方向の
backfill、ユーザープロフィール取得、会話取得を開始しない。

KQ の compile、初回 page load、再評価そのものは常に SQLite-only である。一方、保存した KQ column を
activate すると、cache maintenance のため安全に表現できる source を Awayuki の共有 provider stream plan
へ加える。この lifecycle は KQ evaluator による remote fetch とは別である。

cache maintenance は best effort である。`local` / `all` / where-only と、remote source へ変換できない
`search` / `track` / `user` / `conversation` は各 session の通常 User stream を基準とし、明示した
`home:"acct"` はその session に絞る。`public`/`federated` は対応する ActivityPub 系 provider の
public stream、`local_public` は local-public stream、`hashtag` と `list` は対応 provider の
stream/poller を利用できる。Bluesky は hashtag/list polling に対応する一方、public と local-public
stream は提供しない。この購読は新着 cache を更新するだけで、過去データの完全性や remote 検索を
保証しない。`mentions` / `direct` / `bookmarks` / `favourites` も該当 session の User stream を使う。
`search`、`track`、`user`、`conversation` から query-specific remote acquisition は作らない。

| canonical source | 受理する別名 | 引数 | ローカルでの意味 |
|---|---|---|---|
| `local` | `local`, `all`, `*` | なし | SQLite に保存済みの全 status |
| `home` | `home` | acct/identity を省略可 | 保存済み home timeline entry |
| `list` | `list` | list ID または `acct/list-id` 必須 | 保存済み list timeline entry |
| `mentions` | `mention`, `mentions`, `reply`, `replies` | acct/identity を省略可 | 自 account 宛て mention/reply を含む保存済み status |
| `direct` | `message`, `messages`, `dm`, `dms`, `direct` | acct/identity を省略可 | 保存済みの direct visibility status |
| `search` | `search`, `find` | 検索文字列必須 | 保存済み status のローカル検索結果 |
| `track` | `track`, `stream` | 検索文字列必須 | 保存済み status 本文のローカル照合 |
| `conversation` | `conv`, `conversation`, `talk`, `tree` | status identity 必須 | cache 内でたどれる reply graph |
| `user` | `user` | acct/identity 必須 | top/wrapper author が一致する保存済み status |
| `public` | `public`, `federated` | acct/identity を省略可 | 保存済み federated/public timeline entry |
| `local_public` | `local_public`, `localpublic` | acct/identity を省略可 | 保存済み local-public timeline entry |
| `hashtag` | `hashtag`, `tag` | tag 必須 | 保存済み hashtag timeline entry |
| `bookmarks` | `bookmark`, `bookmarks`, `bookmarked` | acct/identity を省略可 | source acct の保存済み bookmark entry/state |
| `favourites` | `favourite`, `favourites`, `favorite`, `favorites`, `favs` | acct/identity を省略可 | source acct の保存済み favourite entry/state |

acct 引数を省略した account-bound source の source 一致は、ログイン済み account に保存された membership
の和集合になる。`home:"alice@example.social",home:"bob@example.net"` のように明示した source は
account ごとの branch の OR であり、active account を参照しない。viewer scope の決め方は後述のとおりで、
引数なし source の複数 account を暗黙に aggregate-any しない。

`local` は「接続先サーバーのローカル公開タイムライン」ではない。その意味には `local_public` を使う。
StarryEyes の `local:"Tab Name"` による別タブ参照と、Twitter の owner/slug 形式の list 指定は対応しない。

`list:"42"` の値は provider opaque list ID で、global ID ではなく **各ログイン account の名前空間内**で
解釈する。account を省略した KQ column では、同じ引数を list stream に対応する各 session へ適用する。
`list:"alice@example.social/42"` は、最初の `/` より前が `@` を含むか、`.` を含むなど acct-shaped な
場合だけ、その部分を source acct、残りを provider list ID として扱い、一致する session に絞る。
それ以外は `/` を含めた文字列全体が opaque ID であり、`at://did/...` を誤分割しない。無効な ID の
session が失敗しても、別 session の同名 ID と同じ list だとはみなさない。

`mentions` は保存済み status の mention/reply 情報だけを評価する。通知 event の actor、type、event time、
status を持たない follow 通知などを KQ へ公開するものではない。Awayuki KQ は現在 notification envelope
用の source や `notification.*` フィールドを持たない。

account ごとに保存された hashtag membership も source 判定に使う。`search` は本文、CW、URI/URL、author、
tag をローカル照合するが、壊れた tag JSON は tag の寄与だけを Missing とし、正常な本文などによる一致まで
抑止しない。

`user:"acct"` source は top/wrapper author を見るため、boost では booster に一致する。一方、predicate の
`user.*` は original/effective author を見る。この違いは StarryEyes KQ の source と status field の契約を
維持したものである。boost の original が cache にない場合でも wrapper/booster だけで `user` source は
一致できるが、original を必要とする predicate field は Missing となる。

### Provider ごとの mention/tag 制限

- Mastodon/Paon は mention の ID、username、acct を保存する。
- Misskey は現在 mention user ID を保存するが、acct と username へも同じ生 ID が入るため
  `to #id` は利用できても `to @acct` は信頼できない。
- Bluesky は現在 facet の mention/tag を `mentions_json` / `tags_json` へ保存しないため、AT URI の status で
  それらが空なら `to` と `tags` / `hashtags` は Missing となる。

---

## reblog、元投稿、booster、quote

評価時には一つの表示行を次の要素へ分ける。

| 要素 | 意味 |
|------|------|
| top/wrapper | timeline に届いた status。reblog/repost 自体の ID、時刻、account を持つ |
| original/subject | reblog の参照先。通常投稿では top と同じ |
| author | original/subject の投稿者 |
| booster | reblog/boost/repost/純粋 renote の top/wrapper 投稿者 |
| quote target | 引用先。original へ置き換えず、quote 関係として別に保持する |

互換フィールドの解決先は次の通り。

| フィールド | 解決先 |
|-----------|--------|
| `id`, `text`, `body`, `content`, `via`, reply、`user.*` | original/subject |
| `retweet`, `reblog`, `boost`, `renote` | top/wrapper に reblog 関係があるか |
| `retweeter.*`, `reblogger.*`, `booster.*` | booster |
| `has_media`, public counts | original/subject |
| `quote`, `quote.*` | top/subject の quote 関係と quote target |

source の解決先は field と同一ではない。

| source | 解決先 |
|--------|--------|
| `home`, `list`, `public`, `local_public`, `hashtag` | top/wrapper row に保存された timeline membership |
| `user` | top/wrapper author。boost では booster |
| `mentions`, `direct`, `search`, `track` | original/effective status の宛先・visibility・本文 |
| `bookmarks`, `favourites` | original/effective status の account-scoped viewer state |
| `conversation` | cache graph に含まれる wrapper または effective status identity |

reblog の original が cache にない場合、original を必要とする値は Missing となる。wrapper の空本文や
booster を original の代用にはしない。reblog flag と booster は引き続き評価できる。

Misskey の本文なし renote と Bluesky repost は reblog として扱う。本文付き renote/quote post は quote で
あり、legacy `retweet` は false、`quote` は true となる。

---

## status フィールドと別名

### StarryEyes 互換フィールド

| 型 | canonical | 受理する別名 | Awayuki での意味 |
|---|---|---|---|
| Boolean | `direct_message` | `dm`, `isdm`, `is_dm`, `message`, `ismessage`, `is_message`, `direct`, `is_direct`, `directmessage`, `direct_message`, `isdirectmessage`, `is_direct_message` | original の visibility が `direct` |
| Boolean | `retweet` | `rt`, `retweet`, `isretweet`, `is_retweet` | reblog wrapper の有無 |
| Boolean | `has_media` | `has_media`, `media` | original の attachment が非空 |
| Identity | `id` | `id` | original の opaque status ID |
| Identity | `in_reply_to` | `replyto`, `reply_to`, `inreplyto`, `in_reply_to`, `in_reply_to_id`, `reply.id` | original の返信先 ID |
| Set/Missing | `to` | `mention`, `mentions`, `to` | reply target account と mention acct/account ID の set |
| Integer | `favs` | `favs`, `favourite`, `favourites`, `favorite`, `favorites`, `favourer`, `favourers`, `favorer`, `favorers`, `like`, `likes`, `fav_count`, `favourites_count`, `favorites_count`, `likes_count`, `reactions_count` | original の favourite/reaction count |
| Integer | `retweets` | `rts`, `retweets`, `reblogs`, `boosts`, `renotes`, `reposts`, `retweeters`, `reblogs_count`, `boosts_count`, `renotes_count`, `reposts_count` | original の reblog/renote/repost count |
| String | `text` | `text`, `body`, `content` | original 本文を plain text 化した値 |
| String/Missing | `via` | `via`, `from`, `source`, `client`, `application`, `application_name`, `application.name` | original の application name |

StarryEyes の `favs` と `retweets` はローカルに保持した Twitter actor ID set としても使えたが、Awayuki
は favourite/reblog actor の完全な集合を永続化していない。Awayuki では上記 alias は **件数だけ**であり、
`favs contains @alice` のような set 演算は compile error となる。

単数 `retweeter` は booster object 専用である。StarryEyes では count/set alias と衝突して到達不能だった
経路を復活させない。大文字の `RETWEETER` が author に化けた上流の不具合も再現しない。

### Fediverse status 拡張

| 型 | canonical / aliases | 意味 |
|---|---|---|
| String | `content`, `text`, `body` | plain-text 本文 |
| String | `raw_content`, `raw` | provider から保存した HTML/生本文 |
| Identity | `id` | original の provider ID |
| String/Missing | `uri`, `url` | original の URI / 表示 URL |
| String | `server_domain`, `domain`, `host` | status row の保存元 server/domain |
| String/Missing | `application`, `application_name`, `application.name`, `via`, `from`, `source`, `client` | application name |
| Boolean | `reblog`, `isreblog`, `is_reblog`, `boost`, `isboost`, `is_boost`, `renote`, `isrenote`, `is_renote` | reblog wrapper の有無 |
| Boolean | `quote`, `has_quote`, `isquote`, `is_quote` | quote 関係の有無 |
| Boolean | `reply`, `is_reply` | `in_reply_to_id` の有無 |
| String | `visibility` | `public`, `unlisted`, `private`, `direct` |
| Boolean | `public`, `is_public`; `unlisted`, `is_unlisted`; `private`, `is_private`, `followers_only`; `direct`, `is_direct` | visibility の比較用 shortcut |
| String/Missing | `language`, `lang` | status の言語 |
| String | `spoiler_text`, `spoiler`, `cw` | CW 本文 |
| Boolean | `has_cw`, `has_spoiler` | CW が空でない |
| Boolean | `sensitive` | sensitive flag |
| Integer | `favs` とその上表の全 alias | 公開 favourite/reaction count |
| Integer | `retweets` とその上表の全 alias | 公開 reblog 系 count |
| Integer | `replies`, `replies_count` | 公開 reply count |
| Boolean | `edited`, `is_edited` | edited timestamp の有無 |
| String/Missing | `edited_at` | edited timestamp |
| Set/Missing | `tags`, `tag`, `hashtags`, `hashtag` | tag 名の set |

Misskey の reaction 合計は `favourites_count` に正規化される。Misskey の unknown visibility は現在
`public` へ正規化され、`local_only` は保存されない。Bluesky status の visibility は現在常に `public` で
ある。このため visibility predicate をアクセス制御の境界として使用してはいけない。

### reply と quote

| フィールド | 型 | 意味 |
|-----------|---|------|
| `reply.id`, `in_reply_to`, `in_reply_to_id`, `inreplyto`, `replyto`, `reply_to` | Identity/Missing | 返信先 status ID |
| `reply.account_id`, `in_reply_to_account`, `in_reply_to_account_id` | Identity/Missing | 返信先 account ID |
| `quote.id` | Identity/Missing | 解決済み quote target ID |
| `quote.url` | String/Missing | quote target URL |
| `quote.text` | String/Missing | 解決済み quote target の plain-text 本文 |
| `quote.author`, `quote.user`, `quote.author.acct`, `quote.user.acct` | String/Missing | 解決済み quote target の author acct |

返信 ID は server/domain と組で解決する。`conversation` は cache 内だけをたどり、別 server の同じ文字列 ID
を同一 status とみなさない。循環検出と深さ上限に達した場合、それより先は取得しない。

quote envelope だけが保存され、quoted status row を cache から解決できない場合、`has_quote` と envelope の
`quote.url` は既知になり得るが、`quote.id`、`quote.text`、`quote.author.acct` は Missing となる。

---

## account と booster フィールド

`user` は original/subject の author を表す。Fediverse 向けの明示的な別名として `author` も使える。
`retweeter`、`reblogger`、`booster` は reblog wrapper の booster を表し、通常投稿では Missing となる。
bare `renoter` も booster acct の alias だが、フィールド付きの新規クエリでは `booster.*` を使う。
これらを field なしで使うと provider-aware acct identity を返す。

### 互換 account フィールド

次の表は `user.*` と `retweeter.*` の双方に適用される。

| 型 | canonical | 受理する別名 | Awayuki での意味 |
|---|---|---|---|
| Boolean | `is_protected` | `protected`, `isprotected`, `is_protected`, `locked` | follow approval が必要か |
| Identity | `id` | `id` | server-scoped opaque account ID |
| Integer | `statuses` | `status`, `statuses`, `statuscount`, `status_count`, `statusescount`, `statuses_count` | statuses count |
| Integer | `following` | `follow`, `following`, `followings`, `followingcount`, `followingscount`, `following_count`, `followings_count`, `friend`, `friends`, `friendscount`, `friend_count`, `friends_count` | following count |
| Integer | `followers` | `follower`, `followers`, `followerscount`, `follower_count`, `followers_count` | followers count |
| String | `screen_name` | `screenname`, `screen_name` | provider-aware acct |
| String | `name` | `name`, `username`, `display_name` | display name。`user.username` の旧 KQ 挙動を維持 |
| String | `description` | `bio`, `desc`, `description`, `note` | profile note を plain text 化した値 |
| Boolean | `bot` | `bot`, `is_bot` | Bot flag（Awayuki 拡張） |
| String | `domain` | `domain`, `server_domain` | provider-aware な account origin domain（Awayuki 拡張） |

StarryEyes 互換の `user.username` は display name であり、local username ではない。曖昧さのない新規クエリ
では次の canonical field を使う。

| root | 受理するフィールド | 意味 |
|------|-----------|------|
| `author`, `booster`, `reblogger` | `id` | scoped opaque account ID |
| `author`, `booster`, `reblogger` | `username` | provider の local username/handle component |
| `author`, `booster`, `reblogger` | `acct` | provider-aware な account identity |
| `booster`, `reblogger` | `screen_name`, `screenname` | `acct` と同値 |
| `author`, `booster`, `reblogger` | `name`, `display_name` | 表示名 |
| `author`, `booster`, `reblogger` | `description`, `desc`, `bio`, `note` | plain-text profile note |
| `author`, `booster`, `reblogger` | `locked`, `protected`, `is_protected` | follow approval が必要か |
| `author`, `booster`, `reblogger` | `bot`, `is_bot` | Bot flag |
| `author`, `booster`, `reblogger` | `statuses`, `statuses_count` | statuses count |
| `author`, `booster`, `reblogger` | `following`, `following_count` | following count |
| `author`, `booster`, `reblogger` | `followers`, `followers_count` | followers count |
| `author`, `booster`, `reblogger` | `server_domain`, `domain` | provider-aware な account origin domain |

profile note は DB 上では HTML であるため、互換 `description` と canonical `note` は plain text 化して返す。
account の `domain` / `server_domain` は cache/PDS の取得先ではなく identity の origin を返す。完全修飾
ActivityPub/Misskey acct は最後の `@` より後、unqualified local acct は `DbAccount.server_domain`、`@` のない
ドット付き Bluesky handle は handle 全体を DNS identity として返す。

Misskey の欠落 count や Bluesky の basic profile は adapter で 0 へ正規化される場合があるため、account
count の 0 が provider で確認済みの真の 0 とは限らない。

### viewer state

viewer state は status の公開属性ではなく、あるログイン account から見た状態である。

| フィールド | 型 |
|-----------|---|
| `viewer.favourited`, `viewer.favorited` | Boolean/Missing |
| `viewer.reblogged`, `viewer.boosted`, `viewer.renoted` | Boolean/Missing |
| `viewer.bookmarked` | Boolean/Missing |
| `viewer.muted` | Boolean/Missing |
| `viewer.pinned` | Boolean/Missing |

viewer field は、`home:"acct"`、`bookmarks:"acct"` など **一つの source branch が一意に選んだ acct** の
`status_viewer_state` を読む。評価単位は `source branch が一致 & その branch の scope で predicate` で、
全 branch の結果を最後に OR する。したがって `home:"alice",home:"bob"` は branch ごとに別々の viewer
state を評価し、いずれか一方が true なら status は一致する。

引数なしの `home`、`public`、`local_public`、`mentions`、`direct` と、account 未指定の `list`、`hashtag`
は、その status に一致した account がちょうど一つならその acct を viewer scope にする。複数 account で
一致した場合は一つの unscoped branch にまとめ、viewer field は Missing となる。明示した複数 source 引数は
引数ごとの branch のままである。引数なし `bookmarks` / `favourites` は viewer state 自体が source 一致を
証明するため、account ごとの scoped branch になる。

scope がない `from local` / `search` / `track` / `conversation` / `user` や、一つの branch で matched acct が
一意に決まらない場合も Missing となる。active account を暗黙の viewer にせず、一つの branch 内の複数
account を aggregate-any することもない。

---

## media、poll、card

### media

| フィールド | 型 | 意味 |
|-----------|---|------|
| `has_media`, `media` | Boolean/Missing | attachment が一つ以上ある |
| `media.count`, `media_count` | Integer/Missing | attachment 数 |
| `media.types`, `media_types` | Set/Missing | `image`, `gifv`, `video`, `audio`, `unknown` の set |
| `media.descriptions`, `media_descriptions` | Set/Missing | alt text/description の set |
| `has_image`, `media.has_image` | Boolean/Missing | image attachment がある |
| `has_video`, `media.has_video` | Boolean/Missing | video または gifv attachment がある |
| `has_audio`, `media.has_audio` | Boolean/Missing | audio attachment がある |

reblog では original の media を見る。壊れた attachment JSON は空配列と同一視せず Missing とし、query
全体を crash させない。Misskey の animated image は変換結果によって `image` となる場合がある。

### poll

| フィールド | 型 |
|-----------|---|
| `has_poll`, `poll` | Boolean/Missing |
| `poll.id` | Identity/Missing |
| `poll.expired` | Boolean/Missing |
| `poll.multiple` | Boolean/Missing |
| `poll.votes_count` | Integer/Missing |
| `poll.voters_count` | Integer/Missing |
| `poll.options_count` | Integer/Missing |
| `poll.options` | Set/Missing |
| `poll.expires_at` | String/Missing |

poll がない場合、`has_poll` は false で、`poll.*` は Missing となる。Misskey で `voters_count` が得られない
場合は 0 ではなく Missing。Bluesky の現在の変換には poll がない。

`has_card` / `card` は card JSON の存在を Boolean/Missing で返す。card 内部の任意 JSON path は公開しない。

---

## 明示的に非対応の Twitter KQ 機能

以下は compile error となる。件数、viewer state、表示用の近似値へ読み替えない。

| フィールド・構文 | 非対応の理由 |
|------------------|--------------|
| `favs` / `retweets` の actor set | 完全な favourite/reblog actor 一覧を永続化していない |
| `@acct.following`, `.followers`, `.blocking` | relationship graph を永続化していない |
| `list.owner.slug` の member set | provider 共通の list member graph/owner-slug identity がない |
| `user.verified` | Mastodon の verified profile field は Twitter account verification と別概念 |
| `user.translator` | provider 共通値がない |
| `user.contributors_enabled` | provider 共通値がない |
| `user.geo_enabled` | provider 共通値がない |
| `user.favorites` | account favourite count を保存していない |
| `user.listed` | listed count を保存していない |
| `user.location` | account location を保存していない |
| `user.language` | account language を保存していない。status の `language` とは別 |
| `protocol` | scanner が `status_identities.protocol` を hydrate せず、保存元 provider からの推測も不正確 |
| `quote.state` | quote の resolution state を永続化・公開していない |
| `poll.voted`, `poll.own_votes` | poll viewer state が account-scoped table に正規化されていない |
| `local:"Tab Name"` | 名前参照と循環 invalidation の契約を Awayuki が持たない |
| notification envelope / `notification.*` | event actor・event time・statusless event の KQ context は未実装 |

Mastodon の `fields_json[].verified_at` を `user.verified` へ変換しない。必要になった場合は
`has_verified_profile_field` のような別名で追加し、Twitter 互換 alias とは分ける。

---

## 実行範囲、性能、キャンセル

KQ scanner は `created_at, server_domain, id` の安定した順序で SQLite の候補を bounded page scan し、
compiled AST の evaluator が source、original/booster、JSON、Missing を含む最終判定を行う。初期実装では
source と predicate を SQL へ push down せず、evaluator が authoritative である。

compiler が将来安全な SQL prefilter を生成した場合だけ、その条件を parameterized SQL へ適用できる。
prefilter は KQ の真の結果を取りこぼさない superset でなければならず、Missing の NOT、provider ごとに
欠落し得る値、original を未解決のまま扱う条件を無理に SQL へ変換しない。

### Resource limits

| 対象 | 上限 |
|------|------|
| query 本文 | 32 KiB (UTF-8 byte) |
| token 数 | 4,096 |
| expression nesting / 同一優先順位の operator chain | 64 |
| 一つの set literal | 1,024 items |
| source branch | 64 |
| query 全体の source 引数 | 64 |
| regex pattern | 4 KiB |
| regex 評価対象 | 1 MiB |
| plain-text 化した出力 | 1 MiB。超過部分を安全に切り詰める |
| `+` で生成する文字列 | 1 MiB |
| evaluator が読む一つの JSON projection | 1 MiB |
| compiled-query cache | 64 entries。16 KiB 以下の query だけを格納 |
| SQLite scan page | 250 statuses |
| 一 query の scan | `cache status 数 + 250` を 25,000〜2,000,000 に clamp |
| 一 query の時間 | `10 秒 + cache 100,000 statuses ごとに 1 秒`、実効 11〜25 秒 |
| conversation source | 8 |
| conversation root 解決候補 | 32 |
| 一 conversation tree | 500 statuses |
| 全 conversation source 合計 | 4,000 statuses |

query/token/nesting/set/source/regex pattern 上限は compile error、regex 評価対象・`+` の生成文字列・JSON
projection の上限超過は Missing、scan/time/conversation work 上限は typed timeout error となる。timeout
時に途中までの status を成功結果として返さない。

compile error は UTF-8 入力上の span と、1 始まりの line/column を持つ。新規 KQ column は保存前に
compile し、不正なら既存設定を置き換えない。portable DB に過去の不正 KQ が残っている場合は、その
column だけを stream plan から除外し、他 column の購読は継続する。

- 検索はページ単位で SQLite を読み、全件を一度にメモリへ載せない。
- source 引数、文字列、正規表現由来の値を SQL 文字列へ連結せず bind parameter とする。
- scanner の行 identity は `(server_domain, id)` とし、`status_identities` による canonical dedupe はしない。
  同じ canonical URI の別 cache row がそれぞれ候補になり得る。これは wrapper、booster、source membership、
  count を失わないためで、最終 UI の既存 identity/dedupe 処理とは別の境界である。
- 同じ original を指す別 booster wrapper は booster が異なるため、original ID だけで重複除去しない。
- scan row 数と実行時間には budget を設ける。超過時は部分結果を成功扱いせず、条件を絞るよう error を返す。
- timeline query の cancellation token を SQLite page 取得中と evaluator 走査中の双方で確認し、走査中は
  少なくとも 64 statuses ごとに再確認する。
- 新しい query、column の破棄、アプリ終了による cancel 後に、古い結果で UI を上書きしない。
- slow query では scan 件数、match 件数、実行時間、適用 budget を metrics/log に残す。
- query 本文は log へ出さず、engine、UTF-8 byte 長、source 数などの非秘密メタデータだけを記録する。

500 ms 以上または 10,000 statuses 以上を scan した query を slow として記録する。

初期実装では source/predicate の SQL pushdown がないため、`home:"acct"`、domain、visibility のような
選択的な条件でも scan row 数が必ず減るわけではない。`track` や `regex` に限らず、必要な match/page を得る
まで bounded cache 全体を評価し得る。source は結果 corpus の正しさのために指定し、performance hint とは
みなさない。将来 safe prefilter が生成された query だけが候補 scan をさらに縮められる。

---

## クエリ例

### StarryEyes の組み込みタブを Awayuki 向けにする

全 cache:

```kq
from all where ()
```

Home。StarryEyes の built-in は follow relation set を式に含めていたが、Awayuki では provider が保存した
home timeline entry を source とする。

```kq
from home where ()
```

特定 account の Home:

```kq
from home:"alice@example.social" where ()
```

Mentions。quote/reblog wrapper を除外し、保存済み mention/reply status だけを表示する。

```kq
from mentions where !retweet
```

自分の投稿と、自分が行った boost:

```kq
from all where user in our | retweeter in our
```

StarryEyes の Activities は `user in our & (favs > 0 | rts > 0)` だったが、Awayuki で同じ式を書くと
「自分の投稿の現在の公開 count」を表し、favourite/reblog **通知 event**にはならない。notification envelope
を KQ が公開していないため、Activities の忠実な KQ 置換は現在ない。

### Fediverse 拡張

公開かつ CW のない画像投稿:

```kq
from public where visibility == "public" & has_image & !has_cw
```

完全修飾 acct とドメインを明示:

```kq
from local where author.acct == "alice-smith@sub.example.social"
```

十進表記でも opaque ID 文脈では文字列比較:

```kq
from local where id == 109876543210987654
```

acct literal を使う場合:

```kq
from local where user in @alice-smith@sub.example.social
```

Bob が boost した Alice の投稿。`user` source は wrapper/booster、predicate の `user.acct` は original author
を参照する。

```kq
from user:"bob@example.social" where user.acct == "alice@example.social" & retweet
```

解決済み quote target の本文に `Rust` を含むもの:

```kq
from home where quote.text contains "Rust"
```

期限切れでない複数選択 poll:

```kq
from home where has_poll & poll.multiple & !poll.expired
```

明示した account でブックマーク済み:

```kq
from home:"alice@example.social" where viewer.bookmarked
```

メディア種別 set:

```kq
from local_public where media.types contains "audio"
```

conversation は cache 内限定:

```kq
from conversation:"example.social/109876543210987654" where ()
```

`server_domain/status_id` または保存済み status の URI/URL を推奨する。bare ID も互換用に受理するが、
全 server domain から同じ ID の root 候補を探すため、複数の無関係な conversation が一致し得る。

---

## YQ との違い

Awayuki は KQ と [YQ](yq-query-reference.md) を別の言語・別の互換契約として扱う。

| 観点 | KQ | YQ |
|------|----|----|
| 構文 | source + 中置演算子 | S 式 |
| 結合 | 同一優先順位は右結合 | 関数の引数順 |
| `from` | source を実際に選択し、複数 branch は OR | 現在は無視して全 status を走査 |
| 暗黙 source | `where ...` は `local` | `from` を無視するため全 status |
| reblog | original、wrapper、booster、quote を分離 | flattened DbStatus 変数中心 |
| ID | opaque string identity、acct literal | 通常の string 変数 |
| Missing | Kleene 三値論理 | yqrs の nil/evaluation 規則 |
| viewer state | 明示 source acct に scope | source acct を選ぶ FROM plan が未実装 |
| alias | StarryEyes KQ の Twitter 名を多数維持 | Yukari/yqrs の symbol/function 名 |
| remote acquisition | なし。SQLite cache のみ | なし。SQLite cache のみ |

見た目が似た alias があっても、KQ と YQ の式を機械的に貼り替えることはできない。特に KQ の `from`、
右結合、original/booster 解決、Missing と viewer scope は結果集合を変えるため、移行時に個別確認する。
