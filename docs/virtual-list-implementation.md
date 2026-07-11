# Timeline virtualization and retention

現行 frontend は `react-virtuoso` で可変高さ timeline を仮想化する。旧 GPUI の
`v_virtual_list` / item-size cache は使用しない。

## State contract

- status本体は canonical keyのentity mapで正規化する。
- columnはordered keyとpagination cursor、`hasMore`、unread、anchorだけを保持する。
- 1 columnのhard capは1,000件。画面上端から離れていても無制限保持しない。
- stream eventは40ms以内のmicro-batchでidentityごとにcoalesceする。
- trimしても末尾cursor、visible anchor、unreadを保持する。

## Virtuoso contract

- `computeItemKey` は配列indexではなくcanonical status keyを返す。
- prepend前後は最初の可視item keyとoffsetをanchorとして保持する。
- `endReached` は同じcursorの同時loadを起動せず、generationが古いresponseを破棄する。
- status edit、CW、media loadで高さが変わるため固定heightを仮定しない。
- overscanは表示の滑らかさとmedia/translation request量を同時に計測して決める。

## Performance budget

12 columns × 10,000件相当のfixtureと500件burstで、各columnがhard cap以内、同じidentityの
eventが1回へcoalesceされ、nested `findIndex` / full sortを行わないことをunit testする。
実runtimeではReact Profilerのcommit時間、stream batch p95、heap、anchorずれを記録する。

変更時は最低限、次を確認する。

```bash
bun run test -- frontend/src/store/appStore.timeline.test.ts
bun run typecheck
bun run build
```
