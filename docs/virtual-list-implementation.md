# VirtualList 実装ガイド

gpui-component の `v_virtual_list` を使った仮想スクロールの実装パターン。`TimelinePanel` での採用事例をもとに、設計判断と注意点をまとめる。

## 背景

Mastodon の投稿一覧を `div().children(全件)` で描画していたが、全アイテムを毎フレームレンダリングするため性能上の問題があった。`v_virtual_list` は可視範囲のアイテムのみレンダリングすることで、アイテム数に依存しない描画性能を実現する。

## VirtualList の API

```rust
use gpui_component::{v_virtual_list, VirtualListScrollHandle};

v_virtual_list(
    view: Entity<V>,                    // パネルの Entity
    id: impl Into<ElementId>,           // 一意な ID
    item_sizes: Rc<Vec<Size<Pixels>>>,  // 各アイテムの事前計算サイズ
    f: impl 'static + Fn(&mut V, Range<usize>, &mut Window, &mut Context<V>) -> Vec<R>,
) -> VirtualList
```

### 重要な制約

- **高さの事前計算が必須**: `item_sizes` で各アイテムの高さを `Rc<Vec<Size<Pixels>>>` として渡す必要がある
- **アイテムは指定した高さに制約される**: VirtualList 内部で `AvailableSpace::Definite(item_sizes[ix])` が適用される。高さが不正確だとクリッピング（小さすぎ）や余白（大きすぎ）が発生する
- **レンダーコールバックは `'static`**: キャプチャする値はすべて `'static` でなければならない。`Arc<dyn Fn(...)>` でコールバックを包む

### VirtualListScrollHandle

`ScrollHandle` のラッパー。`Deref<Target=ScrollHandle>` を実装しているため、`set_offset()` や `bounds()` がそのまま使える。加えて `scroll_to_item(ix, strategy)` でアイテム単位のスクロールが可能。

`ScrollbarHandle` trait も実装しているため、`vertical_scrollbar(&handle)` に渡せる。

## オフスクリーン高さ測定

可変高さのアイテム（テキスト量、メディア、CW 展開状態で高さが変動する Mastodon 投稿）に対して、`AnyElement::layout_as_root()` を使ったオフスクリーン測定で正確な高さをキャッシュする。

### 測定の原理

```rust
let mut element = render_status_item(status, expanded, None, ..., window, cx);
let measured = element.layout_as_root(
    size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
    window, cx,
);
// measured.height が自然な高さ
```

- `AvailableSpace::Definite(width)` で幅を固定し、`MinContent` で高さを自動計算させる
- `layout_as_root` はレイアウト計算のみで描画は行わない（VirtualList 自体の `measure_item()` と同じパターン）
- 測定用エレメントはコールバック `None` で生成する（イベントハンドラ不要、軽量）

### コールバック None の安全性

`render_status_item()` のコールバック引数はすべて `Option` 型。`None` を渡すと `on_click` などのイベントハンドラが付かないが、レイアウトに影響するスタイル（サイズ、パディング、フレックス）は同一なので、測定結果は正確。

### Element ID の衝突

測定用と実際の描画用で同じ Element ID（`"avatar-{id}"` 等）が使われるが、VirtualList 自体が内部の `measure_item()` で同パターン（生成→測定→破棄→再生成）を使っており、GPUI はこれを正しく処理する。

## カラム幅の検出

高さ測定にはアイテムの描画幅が必要だが、`render()` 時点ではレイアウト前のためパネル幅が不明。前フレームの `ScrollHandle::bounds()` を利用する。

### ScrollHandle::bounds() vs content_size()

| メソッド | 返す値 | 用途 |
|---------|-------|------|
| `bounds()` | スクロールコンテナの**ビューポート bounds** | パネル幅の取得に使用 |
| `content_size()` | 仮想コンテンツの**全体サイズ**（全アイテム合計高さ × 最初のアイテム幅） | スクロールバーの比率計算用 |

**`content_size().width` はパネル幅ではない**。最初のアイテムを `MinContent` で測定した自然幅が返るため、パネル幅として使うと誤差が生じる。

### 幅の遷移とキャッシュの無効化

```
フレーム 1: bounds() = 0（前フレームに VirtualList なし）
            → フォールバック 350px で測定

フレーム 2: bounds() = 実際のパネル幅
            → last_measured_width が None → Some に遷移
            → キャッシュ全クリア → 正しい幅で再測定
```

**重要**: `last_measured_width` が `None` から `Some` に遷移するとき、フォールバック幅で測定されたキャッシュを必ずクリアする。これを忘れると、350px で測定された不正確な高さが残り、余白やクリッピングが発生する。

```rust
let should_invalidate = match self.last_measured_width {
    None => true,  // フォールバック測定のキャッシュを無効化
    Some(prev_width) => /* 幅が変わったか判定 */,
};
```

## 高さキャッシュの管理

`HashMap<String, Pixels>` でステータス ID → 測定済み高さを保持する。

### キャッシュキー

CW（Content Warning）の展開状態で高さが変わるため、キーに展開状態を含める:

```rust
fn height_cache_key(&self, id: &str) -> String {
    if self.expanded_cw.contains(id) {
        format!("{}-expanded", id)
    } else {
        id.to_string()
    }
}
```

### 無効化が必要なケース

| イベント | 対応 |
|---------|------|
| CW トグル | 該当 ID のキャッシュを削除 |
| ステータス更新 (`StatusUpdate`) | 該当 ID のキャッシュを削除 |
| ステータス削除 (`DeleteStatus`) | 該当 ID のキャッシュを削除 |
| パネル幅の変更 | キャッシュ全クリア |
| `None` → `Some` 幅遷移 | キャッシュ全クリア |

### クリーンアップ

`statuses.truncate()` で削除されたステータスのキャッシュエントリが残るため、`rebuild_item_sizes()` の後にクリーンアップする:

```rust
fn cleanup_height_cache(&mut self) {
    let valid_ids: HashSet<&str> = self.statuses.iter().map(|s| s.id.as_str()).collect();
    self.height_cache.retain(|key, _| {
        let base_id = key.strip_suffix("-expanded").unwrap_or(key);
        valid_ids.contains(base_id)
    });
}
```

## render() の構成

```rust
fn render(&mut self, window, cx) -> impl IntoElement {
    // 1. 幅の変更検出（前フレームの bounds から）
    // 2. 高さの測定（未キャッシュ分のみ）
    // 3. item_sizes の再構築
    // 4. コールバック群の構築（Arc<dyn Fn(...)>）
    // 5. v_virtual_list の構築
    // 6. Loading / Empty 状態の排他的配置
}
```

### VirtualList とスクロールバーの配置

```rust
div()
    .size_full()
    .vertical_scrollbar(&self.scroll_handle)  // 外側にスクロールバー
    .child(
        v_virtual_list(...)
            .track_scroll(&self.scroll_handle)  // VirtualList にスクロールを紐付け
            .flex_1()
    )
```

- VirtualList 内部で `overflow_scroll()` が設定されるため、外側に `overflow_y_scroll()` は**不要**
- `vertical_scrollbar` は外側のコンテナに配置し、VirtualList の `scroll_handle` を渡す

### Loading / Empty 状態

VirtualList は `ParentElement` を実装しないため `.when()` による条件付き子要素が使えない。VirtualList と Loading/Empty は排他的に配置する:

```rust
if has_statuses {
    container = container.child(virtual_list);
}
if show_loading {
    container = container.child(loading_indicator);
}
if show_empty {
    container = container.child(empty_message);
}
```
