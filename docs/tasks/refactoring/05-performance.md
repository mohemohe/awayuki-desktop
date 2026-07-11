# 05. パフォーマンス改善

## 観測ベースライン

監査時に利用可能だった実運用 DB スナップショットは約 341 MB、423,222 statuses、32,818 timeline entries、6,508 accounts、5,477 tags だった。warm/cold cache を統制しない参考計測では、存在しない語のローカル検索が約 6.82 秒、status 件数集計が約 0.84 秒、aggregate home query が約 0.91 秒だった。

この DB は migration 018 より前のスナップショットであり、値は正式な benchmark ではない。ただしデータ量に比例する全走査と無制限保持が、既に利用者規模で顕在化する根拠にはなる。以下は BUDGET-01 の固定 dataset で再計測する。

## PERF-01: 起動同期を差分化し、DB 保持ポリシーを導入する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | 起動時間、API 使用量、DB サイズ |
| 依存 | DATA-01〜DATA-03、BUDGET-01 |

### 問題と根拠

session 復元後に走る background startup content sync は全 account を直列処理し（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L4137)）、home / public / notifications に加え、bookmarks と favourites を毎起動 pagination 終了まで全件取得する（[`L4253`](../../../src/tauri_commands.rs#L4253)、[`L4328`](../../../src/tauri_commands.rs#L4328)）。remote から消えた bookmark/favourite の local reconciliation も明確でない。`max_statuses` は画面保持には使われても、DB の定期 prune には使われず、履歴と起動負荷が増え続ける。同期は background でも API / DB / stream と競合し、完了後に全 column refresh を誘発する。

### 方針

- account / timeline ごとに cursor、high-water mark、last successful sync、ETag 等を保存し、通常起動は差分だけ取得する。
- bookmark / favourite は増分 event がない protocol 向けに、頻度を落とした full reconciliation と途中再開 cursor を用意する。
- startup phase を独立させ、1 timeline の失敗で同 account の全 phase を skip しない。
- status retention を age / DB size / timeline type で設定し、bookmark、favourite、thread、draft 等の保護条件を定める。
- orphan tag/account/media metadata の prune、incremental vacuum / WAL checkpoint を idle maintenance として実行する。

### 受け入れ条件

- [ ] 変更のない 2 回目起動で全 bookmark/favourite pages を取得しない。
- [ ] 同期中断後に既取得ページを重複処理せず再開できる。
- [ ] full reconciliation で remote の解除／削除を local に反映する。
- [ ] 固定データで起動 API 件数、DB write 数、ready までの時間、DB 増加量を before / after 比較する。
- [ ] prune は保護 status と参照整合性を壊さない。

## PERF-02: 引用解決を初期表示から分離する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | API latency、タイムライン応答 |
| 依存 | OPS-01 |

### 問題と根拠

quote hydration は request path 内で 1 + 2 + 4 + 8 + 15 + 30 秒程度の backoff を行い（[`timeline_service.rs`](../../../src/services/timeline_service.rs#L316)）、status ごとに lookup / fetch を逐次実行する（[`L433`](../../../src/services/timeline_service.rs#L433)）。notification refresh も 1 件ずつこの処理を待つ（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L4867)）。遅い／削除済み quote 1 件が page 全体を数十秒止め得る。

### 方針と受け入れ条件

- [ ] status 本体を先に保存・返却し、quote は `pending / resolved / unavailable` として後から event 更新する。
- [ ] canonical quote ID ごとの deduplicated job、bounded concurrency、timeout、negative cache を持つ。
- [ ] pane/account close で不要 job をキャンセルし、retry は jitter と server budget を守る。
- [ ] quote timeout が initial timeline latency に加算されないことを統合テストで示す。

## PERF-03: Streaming を有界キュー、coalescing、再同期付きにする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | メモリ、整合性、接続数 |
| 依存 | FE-02、OPS-01 |

### 問題と根拠

raw stream は Tokio の [`unbounded_channel`](../../../src/services/streaming_service.rs#L148)、panel bridge は futures の unbounded channel（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L4493)）を使う。下流は単一 SQLite writer と quote/API 処理なので、burst 時に上流メモリが制限されない。最終 emit queue は bounded だが、満杯時は event を捨てて log count を増やすだけで（[`L4656`](../../../src/tauri_commands.rs#L4656)）、update/delete 消失後の UI 再同期契約がない。

さらに account ごとに column の有無を問わず User / Public / Notification を開始し、protocol によっては同じ channel を複数 socket/task が購読する。

### 方針

- 全段を bounded にし、status identity ごとに new/update を coalesce、delete と resync marker を優先する。
- event に monotonic sequence / generation を付け、gap・overflow・reconnect 後は snapshot/delta resync する。
- queue depth、oldest age、dropped/coalesced count、DB latency を metrics にする。
- server/account connection を multiplex し、表示中／必要な column の購読集合から stream を動的管理する。
- 大きな event payload は `Arc` / `Box` / ID envelope で clone を抑える。

### 受け入れ条件

- [ ] synthetic burst で queue と process memory が設定上限を超えない。
- [ ] overflow 中の update/delete 後も resync で UI と DB が一致する。
- [ ] 不要な public column がないとき public stream を接続しない。
- [ ] reconnect storm に jitter、connect timeout、server 単位 budget が効く。

## PERF-04: Bluesky polling を差分・revision ベースにする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | API rate、WAL、イベント量 |
| 依存 | PERF-03 |

### 問題と根拠

[`bluesky/streaming.rs`](../../../src/bluesky/streaming.rs#L64) は各 tick で最新 40 件を再 emit / UPSERT し、前回 window から消えた post を逐次 GET して削除確認する。30 秒ごとの全量再処理は、1 stream あたり理論上 1 日 115,200 件分の status event を作り、window 脱落と削除も混同する。

### 方針と受け入れ条件

- [ ] cursor / indexedAt / CID 等の protocol revision で新規・変更だけを emit する。
- [ ] polling window からの脱落だけでは delete とみなさない。
- [ ] deletion 確認は低頻度の reconciliation queue にまとめ、API budget と negative cache を使う。
- [ ] 変更なし 1 時間の API calls、DB writes、events が現行より桁違いに減ることを計測する。

## PERF-05: DB 保存を batch transaction 化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | SQLite write throughput |
| 依存 | DATA-02、PERF-03 |

### 問題と根拠

1 status の保存が account、status、tag、mapping、timeline entry 等の複数 statement に分かれ（[`save_status_to_db`](../../../src/services/timeline_service.rs#L594)）、取得 loop から 1 件ずつ呼ばれる。writer connection は 1 本であり、個別 commit、lock acquisition、WAL growth が stream と startup sync を直列化する。

### 方針と受け入れ条件

- [ ] status page / event micro-batch を 1 transaction と prepared statement 群で保存する。
- [ ] duplicate account/tag lookup は batch 内 map と bulk upsert を使う。
- [ ] transaction の最大件数／時間を制限し、UI mutation を長時間飢餓させない。
- [ ] 1,000 status fixture の statements、commit 数、wall time、WAL bytes を before / after 記録する。

## PERF-06: ローカル検索を FTS5 + keyset pagination にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | 検索、インデックス |
| 依存 | DATA-01、DATA-03、BUDGET-01 |

### 問題と根拠

[`query_search_statuses`](../../../src/tauri_commands.rs#L5365) は本文、spoiler、account、JSON 等 7 条件に `lower(...) LIKE '%term%'` を適用し、index を使えない全走査と DISTINCT temp sort を行う。42 万件規模の参考計測で、存在しない語が約 6.82 秒だった。OFFSET pagination は後ページほど不要な走査を増やす。

### 方針

- FTS5 external-content/contentless table に sanitize 済み plain text、CW、account、tags、application 等を格納する。
- migration の backfill を chunk + resumable にし、通常更新は batch transaction と同期する。
- rank / created_at / stable ID の keyset cursor を返す。
- CJK tokenizer、絵文字、URL、mention、大小文字、削除／編集の期待挙動を fixture で確定する。

### 受け入れ条件

- [ ] 42 万件相当 fixture で代表 query の p95 と first-result latency が予算内になる。
- [ ] edit/delete/prune 後に stale hit が残らない。
- [ ] backfill 中断から再開でき、通常起動を長時間 block しない。
- [ ] 旧 LIKE path は migration 完了後に削除される。

## PERF-07: YQ を query 単位で compile し、predicate pushdown する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | filter engine、pagination |
| 依存 | BUDGET-01 |

### 問題と根拠

[`query_yq_statuses`](../../../src/tauri_commands.rs#L5238) は statuses を 250 件ずつ Rust へ読み、必要件数を得るまで先頭から評価する。次ページも match OFFSET 分を再評価するため後ページほど二次的に悪化する。filter は status ごとに Context と regex cache を作る（[`yq_filter.rs`](../../../src/services/yq_filter.rs#L159)）ため、同じ正規表現も row ごとに compile され得る。

### 方針と受け入れ条件

- [ ] parser AST から query plan を 1 回生成し、context / regex / literal を query session で再利用する。
- [ ] time、account、visibility、tag、application 等の安全に表現できる predicate は SQL に pushdown する。
- [ ] 残りだけ Rust evaluator に渡し、stable cursor で次ページを再開する。
- [ ] time budget / cancel / scanned count を持ち、遅い query を UI に示す。
- [ ] 選択性の高低と regex を含む benchmark で scanned rows、allocations、page latency を比較する。

## PERF-08: 通知・thread・aggregate query の N+1 と全体集計をなくす

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | SQLite read path |
| 依存 | DATA-03、BUDGET-01 |

### 問題と根拠

notification は 1 行ごとに actor account、status、status account、quote を個別 query する（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L5520)）。thread も parent/child ごとの読取りを繰り返す。一方、同ファイルには bulk view context helper が既にある（[`L5867`](../../../src/tauri_commands.rs#L5867)）。status bar は 15 秒ごとに全体 COUNT を呼び、aggregate timeline は大きな集合へ window rank を計算してから LIMIT する。

### 方針と受け入れ条件

- [ ] notification / thread を bulk context、JOIN / recursive CTE、IN batch のいずれかへ統一する。
- [ ] page size を増やしても SQL statement 数が比例増加しない。
- [ ] status count は write-time counter または invalidation cache で更新し、15 秒全 COUNT をやめる。
- [ ] aggregate の canonical mapping と index を見直し、42 万件相当で EXPLAIN と p95 を記録する。

## PERF-09: フロントの timeline を有界・O(n)・micro-batch 更新にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | UI memory、stream frame time |
| 依存 | FE-02、PERF-03 |

### 問題と根拠

stream event は全 column array を走査し（[`appStore.ts`](../../../frontend/src/store/appStore.ts#L1179)）、merge は `some` / `map` / `filter + findIndex` / sort / date parse を繰り返す（[`L1491`](../../../frontend/src/store/appStore.ts#L1491)、[`L1662`](../../../frontend/src/store/appStore.ts#L1662)）。一部 column は新規・更新・削除のたびに全 query を再 load する。スクロール中は保持上限を `Number.MAX_SAFE_INTEGER` にする箇所があり、長時間 top に戻らないと配列が増え続ける。ユーザー指定 `maxStatuses` に hard cap もない。

### 方針

- FE-02 の entity map + ordered key index で dedupe/update を O(1)〜O(n) にする。
- stream events を animation frame / 16〜50 ms の小さな batch で reducer へ渡し、同じ identity を coalesce する。
- visible anchor を保つ sliding window / ring buffer と hard maximum を使い、near-top に依存した無制限保持をやめる。
- custom/YQ/thread は影響する entity / column だけ invalidation し、可能な filter は client delta 評価する。

### 受け入れ条件

- [ ] 多 column × 1 万 status × burst fixture で heap が上限内に収まる。
- [ ] 1 event ごとの nested `findIndex` と全 sort がなく、frame p95 を計測する。
- [ ] trim 後も scroll anchor、未読、pagination cursor が保たれる。
- [ ] custom/YQ の再実行回数が event 数 × column 数にならない。

## PERF-10: UI cache と media retry を LRU / single-flight 化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 長時間メモリ、ネットワーク |
| 依存 | BUDGET-01 |

### 問題と根拠

module-global の translation cache（[`TimelineArea.tsx`](../../../frontend/src/components/timeline/TimelineArea.tsx#L62)）と blurhash data URL cache（[`blurhash.ts`](../../../frontend/src/utils/blurhash.ts#L4)）には上限・TTL・account cleanup がない。avatar / emoji / media ごとの retry hook は同じ失敗 URL でも各 component が timer と request を持つ。auto translate も overscan 内の複数 status から同時 IPC を発行し得る。

### 方針と受け入れ条件

- [ ] cache ごとに byte / item 上限、LRU、TTL、content hash、account/logout invalidation を定義する。
- [ ] URL load は single-flight + negative cache + jitter backoff で共有し、viewport 外は停止する。
- [ ] translation は visible priority、同時 2〜4、cancel、engine/content 世代を持つ queue にする。
- [ ] 8 時間相当の synthetic scroll で cache item と timer が上限を超えない。

## PERF-11: feature 単位 code split と bundle budget を導入する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 初回ロード、配布サイズ |
| 依存 | FE-07、FE-10、BUDGET-01 |

### 問題と根拠

Settings、Login、Media overlay 等は [`App.tsx`](../../../frontend/src/components/App.tsx#L5) から eager import される。監査時の main JS は 549.14 kB（gzip 168.62 kB）だった。SqlEditor は `React.lazy`、unicode emoji catalog は利用時 dynamic import で既に別 chunk 化されており、この分割は維持すべきである。警告閾値 1,000 kB だけでは回帰を検出しにくく、production main chunk には static mock も含まれる。

### 方針と受け入れ条件

- [ ] 既存の SqlEditor / emoji catalog 分割を維持し、Settings、Login、Media overlay を利用時 dynamic import する。
- [ ] FE-10 により mock fixture を production graph から除外する。
- [ ] bundle analyzer の artifact と raw / gzip / brotli budget を CI に保存する。
- [ ] cold start の JS parse/evaluate、first interactive、memory を before / after 記録する。

## PERF-12: HTTP client policy を共有し、timeout・上限・キャンセルを統一する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | ネットワーク可用性、resource 制御 |
| 依存 | ERR-01、OPS-01 |

### 問題と根拠

Mastodon と Misskey client は明示 timeout のない reqwest Client を構築する（[`mastodon/client.rs`](../../../src/mastodon/client.rs#L47)、[`misskey/client.rs`](../../../src/misskey/client.rs#L28)）。Bluesky refresh や media download にも ad-hoc `Client::new()` があり、connect、request、idle、response body、redirect、retry が用途ごとに揃わない。停止した server が background startup sync や IPC を無期限に待たせ得る。

### 方針と受け入れ条件

- [ ] `HttpClientFactory` に connect/request/idle timeout、redirect、UA、proxy/TLS、body上限を定義する。
- [ ] normal API、upload、download、stream connect で必要な差分だけ明示 override する。
- [ ] retry は idempotent request、rate-limit header、jitter、server budget を守る。
- [ ] UI / account / app shutdown の cancellation が HTTP request まで伝播する。
- [ ] hanging server、slow body、redirect loop、巨大 error body の integration test がある。

## PERF-13: フロントエンドの要求集中と描画を計測して有界化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | request scheduling、React rendering |
| 依存 | ASYNC-01、BUDGET-01 |

### 問題と根拠

login / switch / save / startup 後に全 column を無制御 `Promise.all` refresh する箇所がある。autocomplete は入力ごとに IPC を発行し、古い結果を捨てても処理自体は cancel しない。profile pane は profile 完了後に複数 timeline fetch を始めるため余分な RTT があり、大きい profile list を仮想化しない。各 StatusItem / Avatar の多数の Zustand subscription、scroll event ごとの store `set` も負荷仮説だが、現時点では profiler 証拠が不足している。

### 方針と受け入れ条件

- [ ] visible pane 優先の request scheduler と用途別同時数を持ち、旧 generation を cancel する。
- [ ] autocomplete は debounce、最小長、結果上限、事前 index、virtual grid を使う。
- [ ] 独立な profile / pinned / posts / media request は bounded queue 上で並行開始し、list は共通 virtualization を使う。
- [ ] React Profiler で stream 100 events/s、scroll、profile open の commit 数／時間を記録する。
- [ ] 計測で有効な箇所だけ `React.memo`、shallow selector、pane-level context、virtualization を適用する。
- [ ] scroll state は値が変化したときだけ dispatch する。

## PERF-14: ログ hot path と定期診断 I/O を制御する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | I/O、ディスク、CPU |
| 依存 | OPS-01 |

### 問題と根拠

backend file log は単一同期 mutex writer で rotation がなく（[`state/logging.rs`](../../../src/state/logging.rs#L14)）、stream parse error で raw message を記録する箇所がある。frontend は console を Tauri log plugin へ転送し、各 IPC の start/success を記録するため、release で詳細ログを有効にすると hot path の I/O と秘匿情報リスクが増える。

### 方針と受け入れ条件

- [ ] non-blocking rolling appender、size/time rotation、世代／総量上限を導入する。
- [ ] release log level、sampling、batching を定義し、stream/IPC hot path の payload 全文を記録しない。
- [ ] secret、OAuth query、投稿／通知本文、ローカルパスの redaction test がある。
- [ ] runtime log / support bundle / crash dump は `.gitignore` と package exclusion の対象になる。
- [ ] logging on/off で stream throughput と p95 latency の差を計測する。

## PERF-15: 小規模 resource の生成を計測して coalesce する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P3 / S** |
| 種別 | connection、timer、短命 task |
| 依存 | OPS-01、BUDGET-01 |

### 問題と根拠

SQLite reader pool は CPU parallelism と同数の connection を作る（[`db/pool.rs`](../../../src/db/pool.rs#L30)）が、desktop SQLite の負荷は disk / WAL / single writer に制約され、多コアほど有利とは限らない。window move / resize の保存 debounce は event ごとに sleep task を作り、generation で最後の write だけを残す（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L1311)）。正しさの問題ではないが、長い drag や多コア端末で不要 resource を増やす。

### 方針と受け入れ条件

- [ ] reader 2〜4 と CPU 数設定を DB fixture で比較し、acquire wait / query p95 / RSS の最良点に上限を置く。
- [ ] window state は watch channel + resettable timer 等の 1 task に coalesce し、close 時に非同期 flush する。
- [ ] sidecar layout/style 等、同種の event-per-task 実装も lifecycle owner の 1 timer に統合する。
- [ ] 変更前後の task / connection 数を計測し、差が無い場合は複雑化しない。
