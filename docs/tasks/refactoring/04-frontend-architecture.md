# 04. フロントエンド設計

## FE-01: 起動処理を明示的な状態機械にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | 起動、エラー復旧 |
| 依存 | ERR-01、OPS-01 |

### 問題と根拠

snapshot 取得失敗時、store は [`error` を設定](../../../frontend/src/store/appStore.ts#L218)するが、[`App`](../../../frontend/src/components/App.tsx#L71) は snapshot がない間 loading 画面で早期 return する。エラーや再試行 UI まで到達せず、利用者には永久 spinner に見える。render error を受け止める Error Boundary もない。

### 方針と受け入れ条件

- [ ] `idle / loading / ready / error / recovering` の boot state と段階名を持つ。
- [ ] DB、設定、account restore、event listener 登録のどこで失敗したか safe error と再試行を表示する。
- [ ] recovery 可能な段階だけを再実行し、二重 listener / startup sync を作らない。
- [ ] React Error Boundary と診断情報コピー導線を持ち、初期化失敗テストがある。

## FE-02: status entity を正規化し、全 mutation を単一 pipeline に統合する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / L** |
| 種別 | 状態モデル、マルチサーバー |
| 依存 | ROUTE-01、ARCH-02 |

### 問題と根拠

favorite / boost の store [`action`](../../../frontend/src/store/appStore.ts#L1000) は操作元 column を更新する一方、edit / poll / media overlay は独自に全 column を走査する（[`appStore.ts`](../../../frontend/src/store/appStore.ts#L1712)、[`MediaPreviewOverlay`](../../../frontend/src/components/media/MediaPreviewOverlay.tsx#L146)）。同じ status の反映規則が複数実装へ分裂している。現在も `statusIdentity` は URI または server + ID を使うが、更新経路ごとに identity helper の使用が揃っていない。

### 方針

- `entities: Map<StatusKey, TimelineStatus>` と column ごとの `StatusKey[]` に正規化する。
- `StatusKey` は ROUTE-01 の protocol / server / canonical identity から生成する。
- post / edit / delete / vote / favorite / boost / stream / media overlay の result を純粋な 1 reducer へ通す。
- optimistic update は operation ID、before image、confirmed/uncertain/failed を持つ。

### 受け入れ条件

- [ ] 同じ status は複数 column／overlay で 1 entity を参照し、更新が一貫する。
- [ ] 異なる server の同一 ID を誤更新しない fixture がある。
- [ ] reblog / quote / notification 内の入れ子 status も同じ identity 規則で更新される。
- [ ] mutation failure / response loss 時に rollback または uncertain 表示が決定的である。

## FE-03: 設定編集を draft + 直列保存にする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | 設定 UX、競合状態 |
| 依存 | CONF-01、ASYNC-01 |

### 問題と根拠

プリセットキーワード等は [`SettingsView`](../../../frontend/src/components/settings/SettingsView.tsx#L650) の入力 1 文字ごとに設定を IPC 保存し、[`saveSetting`](../../../frontend/src/store/appStore.ts#L1150) が返した snapshot を反映する。複数要求の完了順は保証されないため、古い応答が新しい入力を巻き戻せる。DB write とログも入力頻度で増える。

### 方針と受け入れ条件

- [ ] UI draft と last-saved value を分け、明示保存または 300〜500 ms 程度の debounce で送る。
- [ ] 同一設定 resource の mutation を直列化し、generation より古い応答を反映しない。
- [ ] `setPanes(current => ...)` 等の state updater 内から別 state setter を呼ばず、pane/tab/selection を純粋な 1 reducer transition にする。
- [ ] 保存中、保存済み、失敗、競合の状態が見える。
- [ ] 高速入力、応答逆転、画面 close、account switch のテストがある。

## FE-04: Sidecar を専用 lifecycle manager で管理する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | WebView、非同期 resource |
| 依存 | SEC-05、ASYNC-01 |

### 問題と根拠

[`WorkspaceView`](../../../frontend/src/components/workspace/WorkspaceView.tsx#L146) は create 直後の show / hide / position / size を fire-and-forget で呼ぶ箇所があり、rAF からの sync に cancellation / mutex がない。user style は同ファイル [`L328`](../../../frontend/src/components/workspace/WorkspaceView.tsx#L328) と backend の両方が複数 retry を予定し、削除済み sidecar の timer が残り得る。正規化、幅計算、設定反映も App / Settings / Workspace に重複する。

### 方針と受け入れ条件

- [ ] `SidecarLifecycleManager` が create / ready / visible / navigating / closing / failed と generation を所有する。
- [ ] すべての Promise、rAF、timer、event listener を捕捉し、close / reload / remove で cancel する。
- [ ] layout は最新世代だけが適用し、同期は sidecar 単位に直列化／coalesce する。
- [ ] style injection は 1 所有者、成功時停止する backoff、close 時 cleanup になる。
- [ ] schema、URL normalizer、幅計算を `domain/sidecar` に一元化する。

## FE-05: Protocol ごとにログイン form の submit intent を分ける

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / S** |
| 種別 | 入力 semantics、誤操作防止 |
| 依存 | QUAL-02 |

### 問題と根拠

[`LoginView`](../../../frontend/src/components/auth/LoginView.tsx#L63) の form submit は Mastodon login を実行するため、Bluesky password 欄で Enter を押しても意図した app-password login にならない。

### 方針と受け入れ条件

- [ ] protocol ごとに form と submit intent を分け、Enter / button click が同じ操作になる。
- [ ] password manager / browser autofill と button disabled 状態を protocol ごとに維持する。
- [ ] keyboard submit と click の回帰テストがある。

## UI-01: 変更操作と confirmation dialog の lifecycle を共通化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P1 / M** |
| 種別 | mutation UI、dialog lifecycle |
| 依存 | SAFE-01、ERR-01、ASYNC-01 |

### 問題と根拠

vacuum、cache clear、external open 等で pending disable、confirmation、成功／失敗表示の扱いが揃っていない。確認 dialog の resolver は module-global singleton で（[`appStore.ts`](../../../frontend/src/store/appStore.ts#L164)）、次の確認要求が前の Promise を暗黙に false 解決する。所有者、queue、unmount cleanup がなく、並行操作とテスト分離に弱い。

### 方針と受け入れ条件

- [ ] 共通 mutation helper が pending、重複 click、confirmation、success/error toast を扱う。
- [ ] confirmation は dialog ID 付き queue とし、cancel/unmount 時に対応する Promise だけを解決する。
- [ ] 破壊操作と非冪等操作は進行中に再実行できず、SAFE-01 の uncertain 状態を表示できる。
- [ ] 並行要求、二重 click、unmount、response loss の回帰テストがある。

## FE-06: Zustand store を domain slice と純粋 reducer へ分ける

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / L** |
| 種別 | 状態管理、テスト容易性 |
| 依存 | FE-01〜FE-03、ASYNC-01 |

### 問題と根拠

[`appStore.ts`](../../../frontend/src/store/appStore.ts#L1) は 1,828 行あり、session、server cache、timeline algorithm、compose、dynamic pane、overlay、confirmation、API mutation、DOM 寄りの状態を 1 store に持つ。単一の `error?: string` を多くの非同期操作が上書きし、無関係な成功が別操作の error を消し得る。dynamic pane を開く処理にも大量の「検索→focus→作成→load→scroll」重複がある。

### 方針

- `session`、`timelineEntities`、`panes`、`compose`、`settingsDraft`、`overlays`、`notifications` slice に分ける。
- state transition は純粋 reducer、外部処理は use-case action / query service に分ける。
- error / pending は global 1 個ではなく operation/resource key ごとに管理する。
- dynamic pane は descriptor を受ける `openOrFocusDynamicPane` へ集約する。

### 受け入れ条件

- [ ] pure reducer を Tauri mock なしで unit test できる。
- [ ] slice 間更新は明示 action だけを通り、循環 import がない。
- [ ] unrelated operation が error / pending を上書きしない。
- [ ] selector の public contract を保ちながら段階移行できる。

## FE-07: 巨大 React コンポーネントを feature 境界で分離する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / L** |
| 種別 | コンポーネント設計、変更容易性 |
| 依存 | FE-06 |

### 問題と根拠

- [`TimelineArea.tsx`](../../../frontend/src/components/timeline/TimelineArea.tsx#L1): 2,233 行。pane、virtual list、profile、status renderer、translation、poll、notification 等。
- [`SettingsView.tsx`](../../../frontend/src/components/settings/SettingsView.tsx#L1): 2,207 行。設定 schema、draft、pane editor、DB maintenance 等。
- [`ComposeArea.tsx`](../../../frontend/src/components/compose/ComposeArea.tsx#L1): 1,793 行。投稿 draft、media、autocomplete、emoji picker、visibility 等。

行数自体ではなく、データ取得、domain rule、非同期 lifecycle、DOM、表示が同じ component にあることが問題である。

### 方針と受け入れ条件

- [ ] Timeline は `pane controller / list / status variants / profile / translation / notification` に責務分割する。
- [ ] Settings は typed section descriptor、draft reducer、maintenance actions に分ける。
- [ ] Compose は draft reducer、media queue、autocomplete service、picker、form view に分ける。
- [ ] 分割後に prop drilling を巨大 context へ置換せず、狭い selector / hook を使う。
- [ ] 主要 feature の component test と Story / fixture 相当を持つ。

## FE-08: Timeline type を descriptor registry に一元化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 拡張性、backend capability |
| 依存 | ARCH-02、ARCH-03 |

### 問題と根拠

timeline type の label、parameter、filter、stream、refetch、pagination 対応は [`utils/columns.ts`](../../../frontend/src/utils/columns.ts#L8)、store、settings、timeline view の if / 配列へ分散する。新しい種類や protocol 制約を加えると、表示はあるが load できない等の漏れが生じやすい。

### 方針と受け入れ条件

- [ ] typed `TimelineDescriptor` に label key、load strategy、pagination、stream policy、filter、parameter editor、capability 条件を集約する。
- [ ] backend snapshot の capability と組み合わせ、未対応 type を作成できない。
- [ ] 全 type が必要 metadata を持つことを exhaustive check する。
- [ ] 保存済み文字列値は migration なしに変更せず、未知の将来値を安全に表示する。

## FE-09: 表示文言をロジックから分離し、i18n key を型付けする

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | i18n、semantic DTO |
| 依存 | ARCH-02、ERR-01 |

### 問題と根拠

[`i18n.ts`](../../../frontend/src/i18n.ts#L5) は英語原文を key とし、未登録時も原文へ fallback するため欠落を CI で検出できない。boost 判定やアイコン／時刻ロジックが `notificationLabel` の `"boosted"` 等の表示文字列を解釈する箇所もある（[`TimelineArea`](../../../frontend/src/components/timeline/TimelineArea.tsx#L2215)）。翻訳変更が機能変更になっている。

### 方針と受け入れ条件

- [ ] `timeline.empty` 等の typed message ID と EN / JA 辞書を使い、missing key を CI error にする。
- [ ] notification DTO に kind、actor、timestamp 等を持たせ、表示 label を条件分岐に使わない。
- [ ] locale は候補順に最初の対応 locale を選び、実行中変更と Intl formatter を支える。
- [ ] raw backend error と未翻訳 literal を user-facing UI に残さない。

## FE-10: mock adapter を production bundle から外し、契約を厳密化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / M** |
| 種別 | 開発環境、bundle、contract test |
| 依存 | ARCH-02、QUAL-02 |

### 問題と根拠

[`tauri.ts`](../../../frontend/src/api/tauri.ts#L2) は mock を静的 import するため、production の main chunk に大量の fixture と `placehold.co` URL が実際に含まれる。mock は巨大な文字列分岐で module singleton を mutate し、未知 command を [`undefined as T`](../../../frontend/src/api/mock.ts#L330) で成功扱いするため contract drift を隠す。

### 方針と受け入れ条件

- [ ] dev/browser mode のときだけ dynamic import し、production bundle に mock fixture が入らない。
- [ ] generated command map の exhaustive handler を実装し、未知／未実装 command は即座に失敗する。
- [ ] fixture factory と reset API で test ごとに状態を分離する。
- [ ] CI が production asset に mock marker がないことを検査する。

## FE-11: dialog / menu / tabs / autocomplete のアクセシビリティ基盤を共通化する

| 項目 | 値 |
| --- | --- |
| 優先度 / 工数 | **P2 / L** |
| 種別 | UI primitives、キーボード操作 |
| 依存 | QUAL-02、FE-07 |

### 問題と根拠

media overlay に dialog / focus trap、post menu に menu role / keyboard navigation / focus restore、timeline tab に tab semantics がない。compose の media reorder handle は focus できない `div`、autocomplete は候補を screen reader へ伝えない。個別対応では keyboard と focus lifecycle が再び分裂する。

### 方針と受け入れ条件

- [ ] accessible dialog、menu、tabs、listbox、confirmation の共通 primitive を採用／実装する。
- [ ] open 時の初期 focus、Escape、矢印、close 後の focus restore が自動テストされる。
- [ ] media reorder を keyboard で実行し、結果を通知する。
- [ ] axe と RTL keyboard test を PR gate に加え、reduced-motion と text selection も監査する。
