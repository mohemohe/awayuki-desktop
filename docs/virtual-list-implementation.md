# Timeline virtualization and retention

現行 frontend は `react-virtuoso` で可変高さ timeline を仮想化する。旧 GPUI の
`v_virtual_list` / item-size cache は使用しない。

## State contract

- status本体は canonical keyのentity mapで正規化する。
- columnはordered key、`hasMore`、unread、anchorを保持する。local paginationの次offsetは保持中のordered key数、API cursorは末尾statusから導出する。
- 明示的な追加読み込みにはglobal hard capを設けず、取得済みpageを保持する。
- `maxStatuses` は画面上端へ戻ったときの保持目標としてのみ使う。
- stream eventは40ms以内のmicro-batchでidentityごとにcoalesceする。
- trimは保持対象外になったpageを再取得できるよう、次offsetをtrim後のordered key数へ戻す。visible anchorとunreadは維持する。

## Virtuoso contract

- `computeItemKey` は配列indexではなくcanonical status keyを返す。
- prepend前後は最初の可視item keyとoffsetをanchorとして保持する。
- `endReached` は同じcursorの同時loadを起動せず、generationが古いresponseを破棄する。
- status edit、CW、media loadで高さが変わるため固定heightを仮定しない。
- overscanは表示の滑らかさとmedia/translation request量を同時に計測して決める。

## Performance budget

12 columns × 10,000件相当のfixtureと500件burstで、各columnが要求された10,000件を保持し、
同じidentityのeventが1回へcoalesceされ、nested `findIndex` / full sortを行わないことをunit testする。
実runtimeではReact Profilerのcommit時間、stream batch p95、heap、anchorずれを記録する。

変更時は最低限、次を確認する。

```bash
bun run test -- frontend/src/store/appStore.timeline.test.ts
bun run typecheck
bun run build
```
