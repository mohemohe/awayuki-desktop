import React from "react";
import { createPortal } from "react-dom";
import { VirtuosoGrid } from "react-virtuoso";
import { ChevronLeft, ChevronRight } from "lucide-react";
import {
  indexedCustomEmojis,
  normalizeEmojiSearchText,
} from "../../domain/composeAutocomplete";
import { t, translateKnownMessage } from "../../i18n";
import type {
  CustomEmojiSummary,
} from "../../types/app";
import type {
  UnicodeEmojiCategory,
  UnicodeEmojiItem,
} from "../../constants/unicodeEmoji";
import { RetriedCustomEmojiImage } from "../../components/common/CustomEmoji";

export function ComposeEmojiPicker({
  anchorRef,
  triggerRef,
  customEmojis,
  unicodeEmojiCategories,
  onPickEmoji,
  onClose,
}: {
  anchorRef: React.RefObject<HTMLElement | null>;
  triggerRef: React.RefObject<HTMLElement | null>;
  customEmojis: CustomEmojiSummary[];
  unicodeEmojiCategories: UnicodeEmojiCategory[];
  onPickEmoji: (emoji: string) => void;
  onClose: () => void;
}) {
  const pickerRef = React.useRef<HTMLDivElement>(null);
  const [query, setQuery] = React.useState("");
  const normalizedQuery = normalizeEmojiSearchText(query);
  const customGroups = React.useMemo(() => {
    const map = new Map<string, CustomEmojiSummary[]>();
    for (const emoji of customEmojis) {
      const category = emoji.category?.trim() || "Custom";
      const items = map.get(category) ?? [];
      items.push(emoji);
      map.set(category, items);
    }
    return [...map.entries()].map(([name, emojis]) => ({
      name,
      icon: emojis[0]?.url,
      iconEmoji: emojis[0],
      emojis,
    }));
  }, [customEmojis]);
  const categories = React.useMemo(
    () => [...unicodeEmojiCategories, ...customGroups],
    [customGroups, unicodeEmojiCategories],
  );
  const [activeCategory, setActiveCategory] = React.useState(
    categories[0]?.name ?? "Smileys & Emotion",
  );
  const visibleCategoryCount = 6;
  const [categoryOffset, setCategoryOffset] = React.useState(0);
  React.useEffect(() => {
    const maxOffset = Math.max(0, categories.length - visibleCategoryCount);
    setCategoryOffset((offset) => Math.min(offset, maxOffset));
    if (!categories.some((category) => category.name === activeCategory)) {
      setActiveCategory(categories[0]?.name ?? "");
    }
  }, [activeCategory, categories]);
  const active =
    categories.find((category) => category.name === activeCategory) ??
    categories[0];
  const visibleCategories = categories.slice(
    categoryOffset,
    categoryOffset + visibleCategoryCount,
  );
  const visibleEmojis = React.useMemo(() => {
    if (normalizedQuery) {
      const unicodeEmojis = unicodeEmojiCategories.flatMap((category) =>
        category.emojis.filter((emoji) =>
          emoji.searchText.includes(normalizedQuery),
        ),
      );
      const customEmojiResults = indexedCustomEmojis(customEmojis)
        .filter((entry) => entry.searchText.includes(normalizedQuery))
        .map((entry) => entry.emoji);
      return [...unicodeEmojis, ...customEmojiResults];
    }

    return active && "emojis" in active ? active.emojis : [];
  }, [
    active,
    customEmojis,
    normalizedQuery,
    unicodeEmojiCategories,
  ]);
  const activeLabel = normalizedQuery
    ? t("Search results")
    : active?.name
      ? translateKnownMessage(active.name)
      : "";

  const [position, setPosition] = React.useState<{
    left: number;
    top: number;
  } | null>(null);
  React.useLayoutEffect(() => {
    const updatePosition = () => {
      const anchor = anchorRef.current;
      if (!anchor) return;
      const rect = anchor.getBoundingClientRect();
      const pickerWidth = Math.min(365, Math.max(0, window.innerWidth - 16));
      setPosition({
        left: Math.max(8, Math.min(rect.left + 8, window.innerWidth - pickerWidth - 8)),
        top: rect.bottom + 4,
      });
    };

    updatePosition();
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [anchorRef]);

  React.useEffect(() => {
    const closeOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (
        triggerRef.current?.contains(target) ||
        pickerRef.current?.contains(target)
      ) {
        return;
      }
      onClose();
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer, true);
    return () =>
      document.removeEventListener("pointerdown", closeOnOutsidePointer, true);
  }, [onClose, triggerRef]);

  if (!position) return null;

  return createPortal(
    <div
      ref={pickerRef}
      className="fixed z-40 w-[min(365px,calc(100vw-16px))] rounded-md border border-surface0 bg-base-100 p-3 text-sm text-text shadow-xl"
      style={position}
    >
      <div className="mb-2 grid h-8 grid-cols-[28px_minmax(0,1fr)_28px] items-center gap-1">
        <button
          className="btn btn-ghost btn-xs h-7 min-h-7 px-1"
          onClick={() => setCategoryOffset((offset) => Math.max(0, offset - 1))}
          disabled={categoryOffset === 0}
          title={t("Previous emoji categories")}
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
        <div className="flex min-w-0 items-center justify-center gap-2 overflow-hidden">
          {visibleCategories.map((category) => (
            <button
              key={category.name}
              className={`grid h-8 min-w-10 place-items-center rounded-full px-2 text-lg ${active?.name === category.name ? "bg-blue text-crust" : "hover:bg-surface0"}`}
              onClick={() => setActiveCategory(category.name)}
              title={translateKnownMessage(category.name)}
            >
              {"iconEmoji" in category && category.iconEmoji ? (
                <RetriedCustomEmojiImage
                  emoji={category.iconEmoji}
                  alt=""
                  title={translateKnownMessage(category.name)}
                  className="h-5 w-5 object-contain"
                />
              ) : (
                category.icon
              )}
            </button>
          ))}
        </div>
        <button
          className="btn btn-ghost btn-xs h-7 min-h-7 px-1"
          onClick={() =>
            setCategoryOffset((offset) =>
              Math.min(
                Math.max(0, categories.length - visibleCategoryCount),
                offset + 1,
              ),
            )
          }
          disabled={categoryOffset + visibleCategoryCount >= categories.length}
          title={t("Next emoji categories")}
        >
          <ChevronRight className="h-4 w-4" />
        </button>
      </div>
      <input
        className="input input-bordered input-sm mb-3 h-8 min-h-8 w-full border-surface0 bg-base-100 text-sm"
        placeholder={t("Search emoji...")}
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <div className="mb-2 text-xs text-subtext0">
        {activeLabel}
      </div>
      <VirtuosoGrid
        className="overflow-x-hidden pr-1"
        style={{ height: 240 }}
        data={visibleEmojis}
        components={{ List: EmojiGridList, Item: EmojiGridItem }}
        itemContent={(index, emoji) =>
          isUnicodeEmojiItem(emoji) ? (
            <button
              key={`${emoji.codePointsHex.join("-")}-${index}`}
              className="grid h-7 w-7 place-items-center overflow-hidden rounded text-xl hover:bg-surface0"
              onClick={() => onPickEmoji(emoji.emoji)}
              title={emoji.name}
            >
              {emoji.emoji}
            </button>
          ) : (
            <button
              key={emoji.shortcode}
              className="grid h-7 w-7 place-items-center overflow-hidden rounded hover:bg-surface0"
              onClick={() => onPickEmoji(`:${emoji.shortcode}:`)}
              title={`:${emoji.shortcode}:`}
            >
              <RetriedCustomEmojiImage
                emoji={emoji}
                alt={emoji.shortcode}
                title={`:${emoji.shortcode}:`}
                className="max-h-6 max-w-6 object-contain"
              />
            </button>
          )
        }
      />
    </div>,
    document.body,
  );
}

const EmojiGridList = React.forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(function EmojiGridList(props, ref) {
  return <div {...props} ref={ref} className="grid grid-cols-9 gap-2" />;
});

function EmojiGridItem(props: React.HTMLAttributes<HTMLDivElement>) {
  return <div {...props} className="h-7 w-7" />;
}

function isUnicodeEmojiItem(
  emoji: UnicodeEmojiItem | CustomEmojiSummary,
): emoji is UnicodeEmojiItem {
  return "emoji" in emoji;
}
