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

- [x] 変更のない 2 回目起動で全 bookmark/favourite pages を取得しない。
- [x] 同期中断後に既取得ページを重複処理せず再開できる。
- [x] full reconciliation で remote の解除／削除を local に反映する。
- [x] 固定データで起動 API 件数、DB write 数、ready までの時間、DB 増加量を before / after 比較する。
- [ ] prune は保護 status と参照整合性を壊さない。

差分同期・reconciliation checkpointは維持する。retention sweepは実DBで単一writerを占有する可能性があるためproductionの自動実行から撤回し、現在はtest専用である。保護条件と128件上限のfixtureは残すが、writerを長時間保持しないproduction設計が確定するまでpruneを完了扱いにしない。420,000-status system fixtureの同一DB warm-start比較では、旧全page同期（home/notification各1、bookmark/favourite各8 page）がAPI 18件・write 18件・ready 4.324 ms・DB/WAL増加0 bytes、現行checkpoint同期がAPI 0件・write 0件・ready 3.175 ms・増加0 bytesだった。

## PERF-02: 引用解決を初期表示から分離する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | API latency、タイムライン応答 |
| 依存 | OPS-01 |

### 問題と根拠

quote hydration は request path 内で 1 + 2 + 4 + 8 + 15 + 30 秒程度の backoff を行い（[`timeline_service.rs`](../../../src/services/timeline_service.rs#L316)）、status ごとに lookup / fetch を逐次実行する（[`L433`](../../../src/services/timeline_service.rs#L433)）。notification refresh も 1 件ずつこの処理を待つ（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L4867)）。遅い／削除済み quote 1 件が page 全体を数十秒止め得る。

### 方針と受け入れ条件

- [x] status 本体を先に保存・返却し、quote は `pending / resolved / unavailable` として後から event 更新する。
- [x] canonical quote ID ごとのdeduplicated job、bounded concurrency、timeout、negative cacheを持つ。
- [x] pane/account close で不要 job をキャンセルし、retry は jitter と server budget を守る。
- [x] quote timeout がinitial timeline latencyに加算されないことを統合テストで示す。

Home/Public/Notificationのpage保存・startup reconciliation・streamに加え、profile pinned、AIR、thread、post/action/editからも同期quote lookupと約60秒のbackoff APIを削除した。status本体は先にbatch保存・返却し、source account内のcanonical quote identityでdeduplicateした最大128 job、全体同時4件かつsource server単位同時1件、1 attempt 5秒、最大3 attempt、jitter付きretry、5分negative cacheのbackground workerへ渡す。server semaphoreは弱参照で保持し、同一serverの複数accountで共有する一方、別serverをActive accountで狭めない。DTOは初期`pending`、SQLite/embedded quote解決後`resolved`、retry枯渇時`unavailable`を持ち、成功・失敗ともsource account付きstatus updateをbroadcastする。application層へ同期hydration callを戻す変更は`startup:check`が拒否する。column/profile pane IDをtyped IPCの`quoteConsumerId`として登録し、複数paneとaccount background ownerをgeneration付き参照数で共有する。pane closeはそのownerだけを外し、最後のownerでHTTP futureをabortする。logoutは当該source accountのconsumer jobも停止するが、Active account切替とstream購読再構成はquote jobを停止しない。scheduler非blocking、dedupe、pane/account cancel、共有owner維持、`unavailable` event、server budgetをtestした。

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

- [x] synthetic burst で queue と process memory が設定上限を超えない。
- [x] overflow 中の update/delete 後も resync で UI と DB が一致する。
- [x] 不要な public column がないとき public stream を接続しない。
- [x] reconnect storm に jitter、connect timeout、server 単位 budget が効く。

raw、persistence、quote、WebView emitに加え、notification DB/desktop side-effect handoffも64件のTokio bounded FIFOへ移行した。UI eventを先にemitしてからside-effect queueへ送る順序を保ち、通常のpending side effectが後続UI eventを止めないtestを維持する一方、SQLite writer停滞時はbounded queue overflowをlagとして記録する。writer回復・queue drain時はgenerationを進めたResyncを必ずemitし、frontendは当該sourceに関連する全Unified Home/Public/Notification columnをSQLite/API snapshotから再loadする。update/delete coalescing、overflow中のlive delivery、回復Resync、Unified reloadをtestした。stream購読集合は表示columnから算出し、Public columnがないHome/Notification構成ではActivityPub sessionにもPublic streamを追加しない。Mastodon/Misskeyは15秒connect timeoutと最大60秒の指数backoff・決定的jitter、同一source server 250ms spacingを持ち、Active accountでsourceを狭めない。seed v3の420,000-status、100 events/s×10秒system benchmarkはqueue depth 100、drop/resync 0、3,958 events/s、process peak RSS delta 12,779,520 bytesで64MiB ceilingを通過し、全受け入れ条件を満たした。

## PERF-04: Bluesky polling を差分・revision ベースにする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | API rate、WAL、イベント量 |
| 依存 | PERF-03 |

### 問題と根拠

[`bluesky/streaming.rs`](../../../src/bluesky/streaming.rs#L64) は各 tick で最新 40 件を再 emit / UPSERT し、前回 window から消えた post を逐次 GET して削除確認する。30 秒ごとの全量再処理は、1 stream あたり理論上 1 日 115,200 件分の status event を作り、window 脱落と削除も混同する。

### 方針と受け入れ条件

- [x] cursor / indexedAt / CID 等の protocol revision で新規・変更だけを emit する。
- [x] polling window からの脱落だけでは delete とみなさない。
- [x] deletion 確認は低頻度の reconciliation queue にまとめ、API budget と negative cache を使う。
- [x] 変更なし1時間fixtureでDB writesとeventsが4,800から0になる。
- [ ] viewer-specific Home/Notification page API callsを、鮮度とUnified Timelineの完全性を落とさず桁違いに減らす。
- [ ] 同一notificationを反復pollしたときの`reason_subject` call数、TTL後の再取得、negative cacheをmock XRPC fixtureで検証する。

status/notificationのobservable revisionをSHA-256 fingerprintで比較し、最新pageから外れたAT URIはdeleteせず30分間隔・最大4件のreconciliation queueで404/410を確認する。最大512件のmemory windowに加え、migration 027の`bluesky_poll_checkpoints`へaccount/stream単位のソート済みrevision baselineとqueueをJSON 1行で保存する。同一checkpointはconditional UPSERTでwrite 0件、logoutは`login_accounts` FK cascadeで当該accountだけを削除する。再起動後も既存notificationを再通知せず、新規だけをemitする。Home checkpointは`statuses`へ実在するIDだけを復元し、stream event送信後・DB保存前のcrashで未保存statusを誤って抑止しない。全状態は`awayuki.db`内だけにあり、OS store/side fileは作らない。`bun run benchmark:bluesky`の固定1時間fixture（1 Bluesky source、Unified Home、40件page、既定30秒、baseline確立後は無変更）では監査前`0bb67a2`からAPI calls 120→120、DB writes 4,800→0、events 4,800→0となった。

notification subject hydrationにはclient共有のprocess-local cacheを実装した。positive cacheは最大512件・取得時刻基準の絶対TTL 10分で、hitによって期限を延長しない。missing subjectと取得失敗は最大512件・30秒TTLのnegative cacheへ保持し、未cache URIだけを25件単位で`getPosts`する。status mutation後の`get_status`はcacheを最新viewer stateで置換し、deleteは該当entryを破棄する。cacheはprocess memoryだけにあり、SQLite以外の永続状態やOS storeを作らない。cache構造のcapacity・絶対TTL testはあるが、反復poll時のendpoint call-count testは残る。したがってwrite/eventとnotification hydration APIは削減したが、viewer-specific Home/Notification pageそのもののAPI call条件は明示的に未達である。公式AT Protocol firehose/Jetstreamはpublic repository eventであり、認証済みAppViewのviewer-specific Home/Notificationを再現しない。follow DID filterではunknown actor由来notificationやAppView選定postを欠落させるため採用せず、設定intervalの引き延ばしも鮮度契約違反としてproduction source/fixture双方で拒否する。選択的pushまたはviewer cursorがproviderから提供されるまではAPI callだけを成功扱いしない。

## PERF-05: DB 保存を batch transaction 化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | SQLite write throughput |
| 依存 | DATA-02、PERF-03 |

### 問題と根拠

1 status の保存が account、status、tag、mapping、timeline entry 等の複数 statement に分かれ（[`save_status_to_db`](../../../src/services/timeline_service.rs#L594)）、取得 loop から 1 件ずつ呼ばれる。writer connection は 1 本であり、個別 commit、lock acquisition、WAL growth が stream と startup sync を直列化する。

### 方針と受け入れ条件

- [x] status page / event micro-batch を 1 transaction と prepared statement 群で保存する。
- [x] duplicate account/tag lookup は batch 内 map と bulk upsert を使う。
- [x] transaction の最大件数／時間を制限し、UI mutation を長時間飢餓させない。
- [x] 1,000 status fixture の statements、commit 数、wall time、WAL bytes を before / after 記録する。

[`save_status_items_measured`](../../../src/services/timeline_service.rs)はpage/eventをstatus graph・viewer state・timeline entryごと同じtransactionへ保存し、transactionを件数と経過時間で分割する。account identityとtag nameはbatch内`HashSet`で重複排除し、transaction内upsertへまとめる。[`thousand_status_page_uses_bounded_batch_transactions`](../../../src/services/timeline_service.rs)は同じ1,000 statusをWAL有効・1 writerの2 DBへ保存する。一方は旧相当の1件ずつ呼出し、他方はpage batchとし、両方のstatements、commits、wall time、WAL bytesを同じ形式で出力する。status/timeline欠落なし、baseline 1,000 transaction、batch側のtransaction/statement削減、最大transaction件数、account 1回、tag 10回、statement 4,000未満を自動検証する。

## PERF-06: ローカル検索を FTS5 + keyset pagination にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | 検索、インデックス |
| 依存 | DATA-01、DATA-03、BUDGET-01 |

### 問題と根拠

[`query_search_statuses`](../../../src/tauri_commands.rs#L5365) は本文、spoiler、account、JSON 等 7 条件に `lower(...) LIKE '%term%'` を適用し、index を使えない全走査と DISTINCT temp sort を行う。42 万件規模の参考計測で、存在しない語が約 6.82 秒だった。OFFSET pagination は後ページほど不要な走査を増やす。

### 方針

- ICU4XでNFKC、Unicode case folding、辞書word segmentationを行い、非空白のpunctuation/emoji segmentも含む安全なtokenだけをFTS5 external-content tableへ格納する。productionの更新・検索経路では文字n-gramを使用しない。
- status保存transactionは同じ`awayuki.db`内のcoalescing queue更新だけに限定し、token化、FTS更新、segment mergeを低優先度の非同期indexerへ分離する。
- migration の backfill を小さいchunk + resumableにし、既存statusとlive queueのcursor/stateを同じportable DBだけに保持する。
- rank / created_at / stable ID の keyset cursor を返す。
- CJK tokenizer、絵文字、URL、mention、大小文字、削除／編集の期待挙動を fixture で確定する。

### 受け入れ条件

- [ ] 42万件fixtureと93万件実DBで、1/2文字・3文字以上それぞれのp95とfirst-result latencyが予算内になる。
- [x] edit/delete/prune 後に stale hit が残らない。
- [x] backfill中断から再開でき、live queueは8件、backfillは32件ごとにwriterを解放する。indexerは`try_acquire`成功時だけwriterを使い、CPU/mergeを含む処理時間の3倍休止する。取得後はSQLite writerをpreemptできないためmicro-chunk自体もforeground page batchより小さく保つ。
- [x] 最大8 termの候補件数と実candidate branchを10秒budget内かつstatus/account各10,000件上限でmaterializeする。8語超は明示的に拒否し、9語目以降を全status scalar scanへ戻さない。pending status/accountと移行gapはscalar適用前に各256件へ固定し、最新10,000 statusは保存済みICU tokenを再分節せず照合する。bounded sourceを`created_at / server_domain / id`順へ戻してから最終candidateを切る。
- [x] 2ページ目以降は`created_at / server_domain / id` cursorで再開し、OFFSETを0にする。
- [x] query cancellationと10秒のSQLite execution budgetをprogress handlerへ伝播する。
- [x] 絵文字、punctuation、URL、CJK、非ASCII大小文字のindexed fixtureを追加する。

migration 032はtrigram/short n-gramのstatus triggerを全て停止し、status insert/edit/deleteでは`status_search_index_queue`の1 keyをcoalesceするだけにした。migration 034はaccount専用のICU FTS/content/queue/backfillを同じ`awayuki.db`へ追加し、account検索の全件scalar走査を除去した。[`search_indexer.rs`](../../../src/services/search_indexer.rs)はstatus live、account live、account backfill、status backfillの順に処理し、writerが空いていることを確認してからICU4X処理を行い、live 8件／backfill 32件までをtransactionへ反映する。reader snapshotとwriter取得の間に更新が入った場合は128-bit generationをtransaction内で再検証し、古いtokenを破棄する。複数processのbackfillもsource field一致とcursor CAS時だけ反映する。backfill cursor、queue、個別merge debt、索引は全て`awayuki.db`内にあり、OS storeやside DBは作らない。旧n-gram FTS tableはmigration時のblocking `DROP`を避けるためdormant schemaとして残すが、検索・status writeでは参照しない。未到達・pending rowはscalar評価前に各256件の`MATERIALIZED` windowへ固定し、前景で全cacheをICU分節しない。明示的なcache全消去は旧payloadもFTS5の公開操作だけで削除するが、file pageの物理回収をinteractive writerへ戻さないためoffline容量回収は残件とする。

migration 033はcache全消去transaction中だけindex/counter triggerを停止し、ICU FTSの`delete-all`、旧n-gram payload、content/queue、counterとbackfill stateを原子的に消去・リセットする。通常status delete 100万件をqueue 100万件とcounter UPDATE 100万回へ増幅しない。

migration 035は既存ICU postingsを残したままstatus/account backfill cursorだけをO(1)でresetし、旧indexに無かったpunctuation/emoji segmentを低優先度workerで更新する。起動時にcache全件の削除やqueue insertを行わないため、このtokenizer拡張自体が単一writerを長時間占有しない。

[`icu_search.rs`](../../../src/db/icu_search.rs)はICU4X 2.1.1のNFKC、Unicode case folding、dictionary segmentationをindex/queryで共有する。word-like tokenに加えて非空白punctuation/emoji segmentもUTF-8 hexの単一FTS tokenへ符号化するため、`can't`、URL、CJKを`unicode61`が再分割せず、非word検索も全件scalarへ落ちない。検索は最大8 termのstatus/account FTS候補と実branchを各10,000件上限でmaterializeし、account-only matchは専用account ICU FTSから`idx_statuses_account`でstatusへ展開する。pendingとbackfill gapだけは`awayuki_icu_match`を各256件内で使う。最新10,000 statusは`awayuki_icu_index_match`が保存済みtokenをallocationなしで照合し、全bounded sourceをrecency順にしてから上限を適用する。FTSのautomerge/crisis mergeはinteractive pathで無効化し、同じportable DBへstatus/account個別の有限merge試行creditを記録して、queueの空き時間に小さいmanual mergeを反復する。mergeが現在no-opでもcreditを消費するため、永久に1秒周期でwriterを取得しない。検索はanalytics WAL reader上で動き、pool acquireとcandidate countを含む全SQLへCancellationTokenと10秒deadlineを伝える。42万件fixtureと実93万件DBのp95再計測が残るため、PERF-06は部分完了とする。

## PERF-07: YQ を query 単位で compile し、predicate pushdown する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | filter engine、pagination |
| 依存 | BUDGET-01 |

### 問題と根拠

[`query_yq_statuses`](../../../src/tauri_commands.rs#L5238) は statuses を 250 件ずつ Rust へ読み、必要件数を得るまで先頭から評価する。次ページも match OFFSET 分を再評価するため後ページほど二次的に悪化する。filter は status ごとに Context と regex cache を作る（[`yq_filter.rs`](../../../src/services/yq_filter.rs#L159)）ため、同じ正規表現も row ごとに compile され得る。

### 方針と受け入れ条件

- [x] parser AST から query plan を 1 回生成し、context / regex / literal を query session で再利用する。
- [x] time、account、visibility、tag、application 等の安全に表現できる predicate は SQL に pushdown する。
- [x] 残りだけ Rust evaluator に渡し、stable cursor で次ページを再開する。
- [x] time budget / cancel / scanned count を持ち、遅い query を UI に示す。
- [x] 選択性の高低と regex を含む benchmark で scanned rows、allocations、page latency を比較する。
- [x] DB commit前に届くlive stream eventをcache visibilityの根拠にせず、SQLite保存成功後だけCustom/YQをinvalidateする。
- [ ] insert/updateを追跡するSQLite local change revisionと、matchから外れたidentityのreconciliationを実装し、安全なdelta refreshを可能にする。

YQはrequestごとに一度compileし、HTML-safe text contains、visibility、account等をSQL prefilterへ落とし、残りだけ共有EvaluationCache付きEvaluatorで処理する。keyset cursorと規模連動25,000〜2,000,000 row / 15〜120秒budgetを維持しつつ、frontend AbortSignalのoperation IDからCancellationTokenをSQL fetch、account hydration、64件単位evaluationへ明示伝播する。成功時はscanned/matched/duration/budget metricsを返し、500msまたは10,000 scanned rows以上なら`timeline-query-metrics` eventでStatus Barへ件数と時間を表示する。10,000-status release benchmarkではquery-session reuseが96,549 allocations / 1,673,256 bytes、旧相当per-status Context生成が963,644 / 94,240,468 bytesだった。

2026-07-12の実DB（935,974 statuses）では、frontendがdelta cursorを「最後に走査したstatus」ではなく「最後にmatchしたstatus」から作っていたため、0件refreshが同じ範囲を反復した。ただし`created_at` tupleもstatusのinsert/updateを表すchange cursorではなく、過去時刻で到着する連合statusや既存statusの条件出入りを欠落させる。したがって不正なdelta自体をproduction経路から外し、post-commit/coalesced YQ refreshは結果全体を置換する。migration 030はYQの安定走査順序と一致する`(created_at DESC, server_domain DESC, id DESC)` indexを追加する。

stream status/delete、resolved quote、notificationはSQLite保存成功後に別event `timeline-cache-committed`をemitする。これはtimeline streamのgeneration/sequenceには含めず、frontendはcommit前のlive eventからCustom/YQ queryを開始しない。post/edit/delete/action/voteとprovider-backed manual refresh/load-moreもcommand成功後に同じcoordinatorをinvalidateする。YQ refreshはcoalesce後に全結果を置換するため、過去時刻で到着するstatus、editによる条件出入り、deleteを欠落させない。将来deltaを戻す条件は、insert/updateをともに追跡するSQLite local change revisionと非match identityのreconciliationであり、それまでは完全性を優先する。

## PERF-08: 通知・thread・aggregate query の N+1 と全体集計をなくす

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | SQLite read path |
| 依存 | DATA-03、BUDGET-01 |

### 問題と根拠

notification は 1 行ごとに actor account、status、status account、quote を個別 query する（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L5520)）。thread も parent/child ごとの読取りを繰り返す。一方、同ファイルには bulk view context helper が既にある（[`L5867`](../../../src/tauri_commands.rs#L5867)）。status bar は 15 秒ごとに全体 COUNT を呼び、aggregate timeline は大きな集合へ window rank を計算してから LIMIT する。

### 方針と受け入れ条件

- [x] notification / thread を bulk context、JOIN / recursive CTE、IN batch のいずれかへ統一する。
- [x] page size を増やしても SQL statement 数が比例増加しない。
- [x] status count は write-time counter または invalidation cache で更新し、15 秒全 COUNT をやめる。
- [x] aggregate の canonical mapping と index を見直し、42 万件相当で EXPLAIN と p95 を記録する。

## PERF-09: フロントの timeline を有界・O(n)・micro-batch 更新にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | UI memory、stream frame time |
| 依存 | FE-02、PERF-03 |

### 問題と根拠

stream event は全 column array を走査し（[`appStore.ts`](../../../frontend/src/store/appStore.ts#L1179)）、merge は `some` / `map` / `filter + findIndex` / sort / date parse を繰り返す（[`L1491`](../../../frontend/src/store/appStore.ts#L1491)、[`L1662`](../../../frontend/src/store/appStore.ts#L1662)）。一部 column は新規・更新・削除のたびに全 query を再 load する。追加読み込みとstream prependへ同じ保持上限を適用すると、明示的に取得した次pageまで破棄され、pagination cursorが前進しない。

### 方針

- FE-02 の entity map + ordered key index で dedupe/update を O(1)〜O(n) にする。
- stream events を animation frame / 16〜50 ms の小さな batch で reducer へ渡し、同じ identity を coalesce する。
- `maxStatuses` はnear-top復帰時のtrimに限定し、明示的に追加読み込みしたpageにはglobal hard capを適用しない。
- far-anchor中のstream prependはdeferred keyとして保持し、visible anchorとpagination cursorを変更しない。
- custom/YQ/thread は影響する entity / column だけ invalidation し、可能な filter は client delta 評価する。

### 受け入れ条件

- [x] 多 column × 1 万 status × burst fixtureで、要求したstatusを破棄せず128 MiBのfixture heap budgetと50 ms batch p95を満たす。
- [x] 1 event ごとの nested `findIndex` と全 sort がなく、frame p95 を計測する。
- [x] trim後もscroll anchorと未読が保たれ、次offsetはtrim後の保持件数へ戻る。
- [x] custom/YQ の再実行回数が event 数 × column 数にならない。
- [x] live event単独ではCustom/YQ queryを開始せず、SQLite commit signal後だけ開始する。
- [x] Custom/YQの同時実行数は1で、hidden/far-anchor columnを自動refreshしない。
- [x] streamとpost/edit/delete/action/vote、provider refresh/load-moreのDB確定後にCustom/YQをinvalidateする。
- [ ] 90万件以上の実DBと継続streamで、query回数とmain-process CPUがbudget内に収まることをpackaged appで再計測する。

statusはcanonical entity mapとcolumn別ordered keyへ正規化した。`maxStatuses`はnear-top復帰時のtrim目標であり、明示的なload-more pageへglobal hard capは適用しない。stream eventは`requestAnimationFrame`または40msでmicro-batch化し、同一identityのnew/update/deleteをcoalesceする。reducerは同一batchの挿入をcolumnごとにstable mergeし、全配列copyとmembership再構築をeventごとではなくcolumnごと1回に抑える。12 columnへ各10,000 statusを投入するBun fixtureは、各columnが要求された10,000件を保持したまま128 MiBのfixture peak heap budgetと50 ms batch p95を検証する。このheap budgetは固定fixtureのallocation gateであり、status保持数の上限ではない。far anchor時はvisible keyを変更せず、deferred keyと未読だけを更新する。trim後はlocal offsetを保持件数へ戻し、破棄したpageを次回load-moreで再取得できる。

旧80-event同期burst testは40msを跨ぐ継続streamを再現せず、実環境では23:39〜23:41 JSTの3分間だけでCustom 78回・YQ 26回、query duration合計169.354秒/実時間180秒となり、main process約200%を消費した。全`newStatus`がhiddenを含む全Custom/YQを再loadし、query中のeventを完了直後にpending replayする正帰還が原因だった。

`timeline-stream-event`はUXのためSQLite persistenceより先にWebViewへ届く。このlive eventはUnified Home/Public/Notificationへ即時反映し、Custom/YQでは未読だけを更新するが、queryのdirty化や実行権限には使わない。64件／40msで分割した各SQLite transactionのcommit直後に送る`timeline-cache-committed`がglobal Custom/YQ columnをdirty化する正本である。通知はbackend single-flightでbounded WebView queue待ちから切り離し、queue満杯でも待機taskを1つに制限する。status actionやnotificationの複数commitも各段階で通知し、後続失敗・response cancelで先行commitを見失わない。provider refreshはHTTP fetch中とpage境界でcancelし、fetch成功後のSQLite transactionだけを完了まで保護する。command成功後の保守的な同等通知は同じcoalesce waveへ吸収する。各paneで選択中かつnear-topのcolumnだけを2秒coalesce・wave完了後30秒cooldownで直列refreshし、hidden/scrolled columnはdirtyと未読を保持して選択／near-top復帰時に一度追従する。query中の追加commitはversionでdirtyを保持する。invalid SQL/timeoutは同じversionを周期再試行せず、次のcommitまたは明示activationまでdirtyのまま止める。通常のUnified Home/Public/Notification reducerはこのcoordinatorを通さず即時反映を維持する。transaction commit数と通知数、部分失敗、満杯queueで240通知を1件へまとめること、hidden/far-anchor、smooth scroll-to-top、失敗versionを回帰testで固定したが、同じ93万件DBでのpackaged再計測が終わるまでPERF-09全体は部分完了とする。

## PERF-10: UI cache と media retry を LRU / single-flight 化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 長時間メモリ、ネットワーク |
| 依存 | BUDGET-01 |

### 問題と根拠

module-global の translation cache（[`TimelineArea.tsx`](../../../frontend/src/components/timeline/TimelineArea.tsx#L62)）と blurhash data URL cache（[`blurhash.ts`](../../../frontend/src/utils/blurhash.ts#L4)）には上限・TTL・account cleanup がない。avatar / emoji / media ごとの retry hook は同じ失敗 URL でも各 component が timer と request を持つ。auto translate も overscan 内の複数 status から同時 IPC を発行し得る。

### 方針と受け入れ条件

- [x] cache ごとに byte / item 上限、LRU、TTL、content hash、account/logout invalidation を定義する。
- [x] URL load は single-flight + negative cache + jitter backoff で共有し、viewport 外は停止する。
- [x] translation は visible priority、同時 2〜4、cancel、engine/content 世代を持つ queue にする。
- [x] 8 時間相当の synthetic scroll で cache item と timer が上限を超えない。

translationは500 item/2 MiB/1時間、blurhashは256 item/8 MiB/30分、media retryはin-flight 256/1分とnegative 512/5分のweighted LRU上限を持つ。translation keyはstatus identity・target・engine・content hash、blurhashはhashと寸法、mediaはURLを用いる。login/logout時のaccount-scoped cleanupは[`appStore.ts`](../../../frontend/src/store/appStore.ts)から3 cacheを一括clearし、translation clearをtestした。media retryは[`mediaRetryCoordinator.ts`](../../../frontend/src/utils/mediaRetryCoordinator.ts)でURL単位のsingle-flight、negative cache、stable jitterを共有し、componentごとのAbortSignal consumerを追跡する。virtualized media/custom emojiがunmountし、最後のconsumerが消えた未開始timerは中止する。翻訳IPCは[`translationScheduler.ts`](../../../frontend/src/features/timeline/translationScheduler.ts)のvisible-priority queueへ集約し、同時3件、同一generationのsingle-flight、consumer lease単位のcancelを実装した。auto translateは[`TimelineStatusContent.tsx`](../../../frontend/src/features/timeline/TimelineStatusContent.tsx)のIntersectionObserverでviewport近傍に入ったstatusだけをqueueへ入れ、manual操作を高優先度にする。content/engine変更・unmount後の完了はgenerationで破棄する。bounded concurrency、優先順、single-flight、queued cancel、共有consumer、offscreen media cancelを各testで固定した。[`cacheLifetime.test.ts`](../../../frontend/src/utils/cacheLifetime.test.ts)はfake clockで30秒刻み・8時間分のvirtual scroll churnを再現し、全cacheのitem/weight上限、in-flight消滅、timer 0を検証する。

## PERF-11: feature 単位 code split と bundle budget を導入する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 初回ロード、配布サイズ |
| 依存 | FE-07、FE-10、BUDGET-01 |

### 問題と根拠

Settings、Login、Media overlay 等は [`App.tsx`](../../../frontend/src/components/App.tsx#L5) から eager import される。監査時の main JS は 549.14 kB（gzip 168.62 kB）だった。SqlEditor は `React.lazy`、unicode emoji catalog は利用時 dynamic import で既に別 chunk 化されており、この分割は維持すべきである。警告閾値 1,000 kB だけでは回帰を検出しにくく、production main chunk には static mock も含まれる。

### 方針と受け入れ条件

- [x] 既存の SqlEditor / emoji catalog 分割を維持し、Settings、Login、Media overlay を利用時 dynamic import する。
- [x] FE-10 により mock fixture を production graph から除外する。
- [x] bundle analyzer の artifact と raw / gzip / brotli budget を CI に保存する。
- [x] cold start の JS parse/evaluate、first interactive、memory を before / after 記録する。

初期static module graphの評価終了、最後のinitial script responseとの差分、Navigation Timingの`domInteractive`、初回React commit、snapshotが描画された後の2回目animation frame、WebViewが公開するJS heap used/limitを[`startupMetrics.ts`](../../../frontend/src/utils/startupMetrics.ts)で計測する。値はメモリ上support bundleと明示実行したbenchmark artifactだけへ匿名の整数metricsとして含め、SQLite・OS store・side fileへ保存しない。`AWAYUKI_BENCHMARK_MODE=startup bun scripts/benchmark-webview.mjs`は監査前`0bb67a2`と現在版を同じmacOS host、隔離`HOME`のApplication Support DB、同じ可視化手順で比較した。initial JS raw/gzipは550.82/168.63 kBから469.03/148.50 kB（-14.85/-11.94%）、可視化後interactiveは376から372ms（-1.06%）だった。一方、単発cold runのmodule evaluateは314から339ms（+7.96%）、初回commitは321から346ms（+7.79%）、main process peak RSSは112,181,248から121,044,992 bytes（+7.90%）へ悪化したため、bundle削減だけをstartup改善と誤表示しない。WKWebViewはJS heap値を公開せず0だったのでprocess RSSをmemory比較の正本とする。raw `firstInteractiveMs`にはwindow occlusion時間が入るため、fixtureが別記するvisibility waitを差し引いた値だけを比較する。

package-only WebView security fixture追加後のcurrent initial JSは
469.24/148.61 kB raw/gzip（baseline比-14.81/-11.87%）となった。fixture本体は
2.81/1.35 kBのlazy chunkで、通常initial graphのactivation listener増分は
0.21/0.11 kB（+0.04%/+0.07%）。このtest overheadもbundle metricから除外しない。

## PERF-12: HTTP client policy を共有し、timeout・上限・キャンセルを統一する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | ネットワーク可用性、resource 制御 |
| 依存 | ERR-01、OPS-01 |

### 問題と根拠

Mastodon と Misskey client は明示 timeout のない reqwest Client を構築する（[`mastodon/client.rs`](../../../src/mastodon/client.rs#L47)、[`misskey/client.rs`](../../../src/misskey/client.rs#L28)）。Bluesky refresh や media download にも ad-hoc `Client::new()` があり、connect、request、idle、response body、redirect、retry が用途ごとに揃わない。停止した server が background startup sync や IPC を無期限に待たせ得る。

### 方針と受け入れ条件

- [x] `HttpClientFactory` に connect/request/idle timeout、redirect、UA、proxy/TLS、body上限を定義する。
- [x] normal API、upload、download、stream connect で必要な差分だけ明示 override する。
- [x] retry は idempotent request、rate-limit header、jitter、server budget を守る。
- [x] UI / account / app shutdown の cancellation が HTTP request まで伝播する。
- [x] hanging server、slow body、redirect loop、巨大 error body の integration test がある。

通常API、server-kind probe、downloadは[`api/http.rs`](../../../src/api/http.rs)の共有builderから構築し、probeだけ8秒、downloadだけ5分とredirect再検証を明示的に上書きする。`api/detect.rs`のad-hoc clientと無制限`text()`も共有policy・有界bodyへ移行した。hanging server、slow chunk body、redirect loop、Content-Length超過をローカルTCP fixtureで検証する。[`api/retry.rs`](../../../src/api/retry.rs)はprotocol-neutralなTimeout/Transport/RateLimitedだけを最大3 attemptで再送し、`Retry-After`、最大60秒、operation固有の決定的jitter、同一server 250ms spacing、最大256 serverの共有budgetを守る。home/public/notificationを含むread facadeだけへ適用し、post/boost/favourite等のmutationは自動再送しない。quote lookupは専用のbounded retry/negative cacheと多重化しない。timeline/autocomplete/profile、login、media upload/downloadはoperation tokenとの`tokio::select!`でreqwest future/response streamまでcancelし、frontend resource cancelをbackend operation IDへ接続する。post/status action/vote/edit/delete/followも`MutationLifecycle`とstatus mutationのoperation IDをbackend `mutation_operations` managerへ渡し、account scope変更は`cancel_mutation_operation`を送る。provider dispatch後のcancelは成功/失敗を推測せずUIを`uncertain`に保ち、自動retryしない。Tauriの`ExitRequested`は全login/media/timeline/mutation/quote tokenとstream taskを停止する。pending provider futureがcancel時にdropされるtestを固定した。

## PERF-13: フロントエンドの要求集中と描画を計測して有界化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | request scheduling、React rendering |
| 依存 | ASYNC-01、BUDGET-01 |

### 問題と根拠

login / switch / save / startup 後に全 column を無制御 `Promise.all` refresh する箇所がある。autocomplete は入力ごとに IPC を発行し、古い結果を捨てても処理自体は cancel しない。profile pane は profile 完了後に複数 timeline fetch を始めるため余分な RTT があり、大きい profile list を仮想化しない。各 StatusItem / Avatar の多数の Zustand subscription、scroll event ごとの store `set` も負荷仮説だが、現時点では profiler 証拠が不足している。

### 方針と受け入れ条件

- [x] visible pane 優先の request scheduler と用途別同時数を持ち、旧 generation を cancel する。
- [x] autocomplete は debounce、最小長、結果上限、事前 index、virtual grid を使う。
- [x] 独立な profile / pinned / posts / media request は bounded queue 上で並行開始し、list は共通 virtualization を使う。
- [x] React Profiler で stream 100 events/s、scroll、profile open の commit 数／時間を記録する。
- [x] 計測で有効な箇所だけ `React.memo`、shallow selector、pane-level context、virtualization を適用する。
- [x] scroll state は値が変化したときだけ dispatch する。

frontend readはtimeline 4、profile 3、autocomplete 2のlane別上限を持つpriority schedulerへ通し、resource keyごとのgenerationとAbortSignalで旧requestをcancelする。Custom SQL/YQはreader poolを小さくする代わりに、重い全走査を同時にtemp-sortしてCPU/IOを競合させないanalytics laneと上記dirty coordinatorへ分離した。SQLite reader pool自体は500接続のlazy共有poolを維持し、通常timeline/profile/searchの並列性を狭めない。visible load/refreshはbackgroundより高優先度で、pane close・logoutはprefix/all cancelを行う。mention/hashtag autocompleteは250ms debounce、2文字以上、backend/clientとも8件上限、emojiは一度だけloadしたcustom/Unicode catalogからclient filterし、最大8件のARIA listboxなのでvirtualizationを要しない。profile identity/pinned/posts/mediaは待ち合わせ前に同時scheduleする。プロフィール画像・自己紹介・field・集計・Posts/Media tabは共通Virtuosoのscroll-away Headerへ置き、statusと同じ1つのscroll ownerを使う。投稿0件でもEmptyPlaceholder内で同じ構造を保ち、scroll-to-topは先頭statusではなくprofile最上部へ戻す。near-top stateはbooleanが変化したときだけZustandへdispatchする。[`renderMetrics.ts`](../../../frontend/src/utils/renderMetrics.ts)はstream batchとscroll eventの次commitをscenario attributionし、profile Profilerも含め各240 sampleに制限する。production ReactでProfiler callbackが無効でもfixtureの`useLayoutEffect`が同じ固定scenarioへcommit時間を記録する。macOS packaged WebViewでは100 events/s×10秒のstream commit/frame p95が10/33ms、scroll frame p95 66ms、profile open commit/frameが8/4msだった。共通Virtuosoが可視rowだけをmountし、stream frameが50ms予算内だったため、計測根拠のない追加`React.memo`やselector再編は行わない。support bundleとartifactは整数集計だけを含み、pane/column/account/status IDを出さず永続化しない。

## PERF-14: ログ hot path と定期診断 I/O を制御する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | I/O、ディスク、CPU |
| 依存 | OPS-01 |

### 問題と根拠

backend file log は単一同期 mutex writer で rotation がなく（[`state/logging.rs`](../../../src/state/logging.rs#L14)）、stream parse error で raw message を記録する箇所がある。frontend は console を Tauri log plugin へ転送し、各 IPC の start/success を記録するため、release で詳細ログを有効にすると hot path の I/O と秘匿情報リスクが増える。

### 方針と受け入れ条件

- [x] non-blocking rolling appender、size/time rotation、世代／総量上限を導入する。
- [x] release log level、sampling、batching を定義し、stream/IPC hot path の payload 全文を記録しない。
- [x] secret、OAuth query、投稿／通知本文、ローカルパスの redaction test がある。
- [x] runtime log / support bundle / crash dump は `.gitignore` と package exclusion の対象になる。
- [x] logging on/off で stream throughput と p95 latency の差を計測する。

16,384件のsynthetic redacted stream eventをruntimeと同じ1/16 sampling、redaction、
2,048件bounded queue、file writerへ通すrelease fixtureを追加した。macOSローカル計測は
logging off/onのproducer p95が0.000042/0.001708 ms、on throughputが
1,242,801 events/s、drop 0件だった。CIは同じfixtureを420k dataset jobで実行し、
`logging-benchmark.json`を保存する。絶対budgetはproducer p95 5 ms未満、throughput
100 events/s以上、drop 0件であり、disk drain完了までをthroughputへ含める。

backend file logは2,048件のbounded `sync_channel`へ`try_send`し、専用threadが64 KiB bufferで書く。1 record 256 KiB、1 file 5 MiBまたは24時間、3世代の上限を持ち、queue overflowはdiagnosticsのdropped countへ加算する。release既定はinfoでfile log自体opt-in、debug/traceを明示した場合もstream/IPC/tauri-command/UI timeline hot pathは決定的に1/16 samplingする。frontend console forwardingはproductionのlog/debugを捨て、100ms・50件・16 KiBでbatch化する。backend/frontend両方でtoken/password/OAuth code/state/Bearer/Authorization、content/spoiler/post/notification/status text、Unix/Windows local pathをredactし、stream parse errorはpayload byte数と型だけを記録する。size/age/generationとredaction fixtureをtestし、`.gitignore`およびmacOS/AppImage/Windowsの明示package scriptにdiagnostic artifact名が入らないことをportable-state gateで検査する。logging off/onの同一burstはrelease fixtureとperformance CI artifactで継続比較する。

## PERF-15: 小規模 resource の生成を計測して coalesce する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P3 / S** |
| 種別 | connection、timer、短命 task |
| 依存 | OPS-01、BUDGET-01 |

### 問題と根拠

SQLite reader pool は CPU parallelism と同数の connection を作る（[`db/pool.rs`](../../../src/db/pool.rs#L30)）が、desktop SQLite の負荷は disk / WAL / single writer に制約され、多コアほど有利とは限らない。window move / resize の保存 debounce は event ごとに sleep task を作り、generation で最後の write だけを残す（[`tauri_commands.rs`](../../../src/tauri_commands.rs#L1311)）。正しさの問題ではないが、長い drag や多コア端末で不要 resource を増やす。

### 方針と受け入れ条件

- [x] reader poolを実クライアントの並列要求で詰まらない接続数にし、acquire waitとmemoryを検証する。
- [x] window state は watch channel + resettable timer 等の 1 task に coalesce し、close 時に非同期 flush する。
- [x] sidecar layout/style 等、同種の event-per-task 実装も lifecycle owner の 1 timer に統合する。
- [x] 変更前後の task / connection 数を計測し、差が無い場合は複雑化しない。

2/4接続という小さいpartitionは、Unified timeline・profile4資源・SQL/YQ・streamingが同時に動く実クライアントでpool timeoutを起こした。通常/analyticsを同じlazy WAL reader poolへ統合し、上限を500にした。SQLxの`u32::MAX`はpool内部で約100GBの仮想メモリを予約するため採用しない。500は要求時だけ接続を開き、通常負荷で一桁capを作らず、内部予約も現実的な大きさに保つ。reader benchmarkは比較artifactとして残すが、小規模synthetic queryの最小RSSだけでruntime capを2へ強制しない。window stateの1 worker/debounce、Sidecar layout/style timerのcoalesceは維持する。
