# YQ (Yukari Query) — Awayuki 実装リファレンス

YQ (Yukari Query) は [Yukari for Android](https://github.com/shibafu528/Yukari) 由来の S 式ベースのフィルタ言語である。
Awayuki では公式 Rust 実装 [yqrs](https://github.com/shibafu528/yqrs) をフィルタエンジンとして使用し、
DB に保存済みの Mastodon ステータスをインメモリでフィルタリングする。

本ドキュメントでは、Awayuki の SQLite データ構造と YQ クエリ構文の間で **検索できること・できないこと** を整理する。

---

## クエリ構文

```
[from <source>] [where] <S-expression>
```

- `from` 句と `where` 句はどちらも省略可能。
- 式だけを書いた場合（例: `(contains text "keyword")`）、Awayuki が自動的に `where` を補完する。
- `from` 句を省略した場合、暗黙的に `from all` として扱われる。

---

## FROM 句 — データソース指定

### 元仕様 (Yukari for Android)

| ソース | 説明 | 引数 |
|--------|------|------|
| `all` / `*` / `local` / `stream` | 全ストリーム受信ツイート | なし |
| `home:"screenName"` | ホームタイムライン | アカウント名 |
| `mention:"screenName"` | メンションタイムライン | アカウント名 |
| `user:"target"` | 特定ユーザーのツイート | ユーザー名 |

Yukari はマルチアカウント対応であり、`home:"account_a"` と `home:"account_b"` のように
異なるアカウントのタイムラインをソースとして区別できた。

### Awayuki の現状

**`from` 句は現在無視される。** 常に `statuses` テーブル全体を対象としてフィルタリングを行う。

理由:
- Awayuki製品自体はmulti-account対応だが、YQの`from` sourceを`timeline_entries.account_acct`へ変換するquery planが未実装である。
- `where` 句で `(= domain "example.com")` のような条件を書くことで、同等のフィルタリングは可能。

### 将来の FROM source 対応に向けて

`timeline_entries` テーブルには `account_acct` カラムがあり、どのアカウントのどのタイムラインに属するエントリかを記録している。`from home:"user@example.com"` をこの列とのJOINへ安全にpushdownするまでは、`from`をaccount分離機能として扱ってはいけない。

---

## WHERE 句 — 検索可能な変数

### ステータスフィールド

| 変数名 | 型 | 説明 | 検索可否 |
|--------|-----|------|----------|
| `text` / `content` | string | 投稿本文 (HTML タグ除去済み) | **可** |
| `raw_content` | string | 投稿本文 (生 HTML) | **可** |
| `visibility` | string | `public` / `unlisted` / `private` / `direct` | **可** |
| `language` / `lang` | string / nil | 言語コード (`ja`, `en` など) | **可** |
| `spoiler_text` / `cw` | string / nil | CW テキスト (空なら nil) | **可** |
| `sensitive` | t / nil | センシティブフラグ | **可** |
| `favourites_count` / `fav_count` | integer | お気に入り数 | **可** |
| `reblogs_count` / `boost_count` | integer | ブースト数 | **可** |
| `replies_count` | integer | リプライ数 | **可** |
| `bookmarked` | t / nil | ブックマーク済み | **可** |
| `favourited` / `faved` | t / nil | お気に入り済み | **可** |
| `reblogged` / `boosted` | t / nil | ブースト済み | **可** |
| `muted` | t / nil | ミュート済み | **可** |
| `pinned` | t / nil | ピン留め | **可** |
| `in_reply_to_id` | string / nil | リプライ先ステータス ID | **可** |
| `is_reply` | t / nil | リプライかどうか | **可** |
| `is_reblog` / `is_boost` | t / nil | ブーストかどうか | **可** |
| `has_media` | t / nil | メディア添付の有無 | **可** |
| `has_poll` | t / nil | 投票の有無 | **可** |
| `has_card` | t / nil | リンクカードの有無 | **可** |
| `has_cw` | t / nil | CW の有無 | **可** |
| `server_domain` / `domain` | string | サーバードメイン | **可** |

### アカウントフィールド

| 変数名 | 型 | 説明 | 検索可否 |
|--------|-----|------|----------|
| `user` / `username` | string | ユーザー名 (ローカル名) | **可** |
| `acct` | string | acct (`user` or `user@domain`) | **可** |
| `display_name` | string | 表示名 | **可** |
| `bot` | t / nil | Bot フラグ | **可** |
| `locked` | t / nil | 鍵アカウント | **可** |

### DB に存在するが YQ 変数として未公開のフィールド

以下は `statuses` / `accounts` テーブルに保存されているが、現在の VariableProvider では変数として公開していない。

| DB カラム | テーブル | 未公開の理由 |
|-----------|---------|-------------|
| `id` | statuses | ステータスIDでのフィルタは実用性が低い |
| `uri` / `url` | statuses | URI でのフィルタは実用性が低い |
| `created_at` / `edited_at` | statuses | 日時比較の演算子が yqrs に未実装 (将来的に `>`, `<` 追加で対応可能) |
| `account_id` | statuses | 内部IDであり、`acct` で代替可能 |
| `in_reply_to_account_id` | statuses | 公開可能だが需要を見て判断 |
| `reblog_of_id` | statuses | `is_reblog` で代替可能 |
| `poll_json` | statuses | JSON 文字列。構造化検索には yqrs の拡張が必要 |
| `card_json` | statuses | 同上 |
| `mentions_json` | statuses | 同上。特定ユーザーへのメンション検索に有用だが JSON パースが必要 |
| `tags_json` | statuses | 同上。特定ハッシュタグ検索に有用だが JSON パースが必要 |
| `emojis_json` | statuses | 同上 |
| `media_attachments_json` | statuses | 同上。メディアタイプ別検索に有用だが JSON パースが必要 |
| `fetched_at` | statuses | 内部管理用 |
| `note` | accounts | アカウントプロフィール本文 |
| `avatar` / `avatar_static` / `header` | accounts | 画像 URL |
| `followers_count` / `following_count` / `statuses_count` | accounts | 公開可能だがフィルタ用途は限定的 |
| `created_at` / `fetched_at` | accounts | 日時比較未対応 |
| `fields_json` / `emojis_json` | accounts | JSON 構造化検索が必要 |

---

## 演算子・関数

### yqrs ビルトイン

| 関数 | エイリアス | 説明 |
|------|-----------|------|
| `and` | `&` | 論理 AND (短絡評価) |
| `or` | `\|` | 論理 OR (短絡評価) |
| `not` | `!` | 論理 NOT |
| `equals` | `eq`, `=`, `==` | 等値比較 |
| `noteq` | `neq`, `!=`, `/=` | 不等値比較 |
| `contains` | `in` | 部分文字列マッチ / リスト要素検索 |
| `list` | — | リスト構築 |
| `quote` | — | 式のクォート |
| `+` `-` `*` `/` `%` | `mod` | 算術演算 |

### Awayuki 独自追加

| 関数 | 説明 |
|------|------|
| `regex` | 正規表現マッチ。`(regex text "pattern")` 形式。Rust の regex crate を使用。 |

### 元仕様にあるが yqrs に未実装の機能

| 機能 | 状況 |
|------|------|
| `>`, `<`, `>=`, `<=` (大小比較) | yqrs にビルトインなし。日時・数値の範囲検索に必要。 |

---

## 元仕様 (Yukari / Twitter) との主な差異

### 1. マルチアカウントと FROM 句

Yukari は複数の Twitter/Mastodon アカウントを同時に扱え、`from home:"account_a",mention:"account_b"` のように
アカウントごとのタイムラインを横断検索できた。

Awayuki は現時点でシングルアカウント運用のため、`from` 句のソース指定・アカウント引数は無視される。
DB 構造 (`login_accounts`, `timeline_entries`) にはマルチアカウントの基盤があり、
将来対応時に `from` 句のサポートを追加できる。

### 2. フィールド名の違い

| Yukari (Twitter) | Awayuki (Mastodon) | 備考 |
|------------------|--------------------|------|
| `?text` | `text` / `content` | Awayuki では `?` プレフィックス不要 |
| `?source` | — | Mastodon API にはクライアント名フィールドがない |
| — | `visibility` | Mastodon 固有 (Twitter には相当するものがない) |
| — | `spoiler_text` / `cw` | Mastodon 固有の CW 機能 |
| — | `sensitive` | Mastodon 固有のセンシティブフラグ |
| — | `language` / `lang` | Mastodon 固有の言語タグ |
| — | `bookmarked` | Mastodon 固有のブックマーク |
| — | `is_reblog` / `is_boost` | Twitter のリツイート相当 |
| — | `has_poll` / `has_card` | Mastodon 固有 |
| — | `server_domain` / `domain` | 連合を意識したフィールド |

### 3. ストリーミングソースの違い

Yukari の `from stream` / `from local` は Twitter の UserStream や Streaming API に対応していた。
Awayuki では WebSocket ストリーミングで受信した全イベントがリアルタイムに YQ 評価されるが、
ソースの種別 (`from` 句) による選別は行わない。

### 4. 検索対象のスコープ

- **Yukari**: ストリーミングで受信中のツイートをリアルタイムフィルタ (メモリ上)。過去のツイートは対象外。
- **Awayuki**: DB に保存済みの全ステータスが対象。加えてストリーミング受信時のリアルタイムフィルタも動作する。DB ベースのため、過去に取得したステータスも検索可能。

---

## クエリ例

```lisp
;; 本文に "Rust" を含むステータス
(contains text "Rust")

;; 公開投稿のみ
(= visibility "public")

;; CW 付きのステータス
has_cw

;; Bot 以外のユーザーの投稿
(not bot)

;; ブースト数が 10 以上 (数値比較は equals のみ)
;; 注: >, < は未対応のため完全な範囲指定はできない

;; 特定ユーザーの投稿
(= acct "user@example.com")

;; 正規表現による検索
(regex text "(Rust|Go|Python)")

;; 複合条件: メディア付きの公開投稿で、リプライではないもの
(and (= visibility "public") has_media (not is_reply))

;; 特定ドメインからの投稿
(= domain "mastodon.social")
```
