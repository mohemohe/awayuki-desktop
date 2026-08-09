import type {
  UnicodeEmojiCategory,
  UnicodeEmojiItem,
} from "../constants/unicodeEmoji";
import type { CustomEmojiSummary } from "../types/app";

export type ComposeAutocompleteKind = "mention" | "hashtag" | "emoji";

export const emojiNoWidthSpace = "\u200B";

export function appendEmojiNoWidthSpace(value: string) {
  return value.endsWith(emojiNoWidthSpace)
    ? value
    : `${value}${emojiNoWidthSpace}`;
}

export type ComposeAutocompleteMatch = {
  kind: ComposeAutocompleteKind;
  query: string;
  start: number;
  end: number;
};

export type ComposeAutocompleteItem = {
  value: string;
  label: string;
  insertText?: string;
  description?: string;
  avatar?: string;
  emoji?: CustomEmojiSummary;
  unicodeEmoji?: UnicodeEmojiItem;
};

export type ComposeAutocompleteState = ComposeAutocompleteMatch & {
  items: ComposeAutocompleteItem[];
  selectedIndex: number;
  loading: boolean;
};

type IndexedCustomEmoji = {
  emoji: CustomEmojiSummary;
  normalizedShortcode: string;
  searchText: string;
};

const autocompleteBoundaryChars = new Set([
  " ",
  "\n",
  "\t",
  emojiNoWidthSpace,
  "(",
  ")",
  "[",
  "]",
  "{",
  "}",
  "<",
  ">",
  '"',
  "'",
  "`",
]);

const customEmojiAutocompleteIndexes = new WeakMap<
  CustomEmojiSummary[],
  IndexedCustomEmoji[]
>();

export function detectComposeAutocomplete(
  text: string,
  caret: number,
): ComposeAutocompleteMatch | null {
  let start = caret;
  while (
    start > 0 &&
    !autocompleteBoundaryChars.has(text[start - 1] ?? "")
  ) {
    start -= 1;
  }
  const token = text.slice(start, caret);
  const emojiMarkerIndex = token.lastIndexOf(":");
  if (emojiMarkerIndex >= 0) {
    if (token.slice(0, emojiMarkerIndex).includes(":")) return null;
    const query = token.slice(emojiMarkerIndex + 1);
    if (query.length > 80 || !/^[\w+-]*$/.test(query)) return null;
    return {
      kind: "emoji",
      query,
      start: start + emojiMarkerIndex,
      end: caret,
    };
  }
  if (token.length < 2) return null;
  const marker = token[0];
  if (marker !== "@" && marker !== "#") return null;
  const query = token.slice(1).trim();
  if (!query || query.length > 80) return null;
  return {
    kind: marker === "@" ? "mention" : "hashtag",
    query,
    start,
    end: caret,
  };
}

export function uniqueAutocompleteItems(
  kind: ComposeAutocompleteKind,
  items: ComposeAutocompleteItem[],
) {
  const seen = new Set<string>();
  return items.filter((item) => {
    const key = item.value
      .trim()
      .replace(
        kind === "mention" ? /^@/ : kind === "hashtag" ? /^#/ : /^:|:$/g,
        "",
      )
      .toLowerCase();
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function indexedCustomEmojis(emojis: CustomEmojiSummary[]) {
  const cached = customEmojiAutocompleteIndexes.get(emojis);
  if (cached) return cached;
  const index = emojis.map((emoji) => ({
    emoji,
    normalizedShortcode: emoji.shortcode.toLowerCase(),
    searchText: normalizeEmojiSearchText(
      `${emoji.shortcode} ${emoji.category ?? ""}`,
    ),
  }));
  customEmojiAutocompleteIndexes.set(emojis, index);
  return index;
}

export function emojiAutocompleteItems(
  emojis: CustomEmojiSummary[],
  unicodeEmojiCategories: UnicodeEmojiCategory[],
  query: string,
) {
  const normalizedQuery = normalizeEmojiSearchText(query);
  const normalizedShortcodeQuery = query.toLowerCase();
  const customItems = indexedCustomEmojis(emojis)
    .filter((entry) => {
      if (!normalizedQuery) return true;
      return entry.searchText.includes(normalizedQuery);
    })
    .sort((a, b) => {
      if (!normalizedQuery) {
        return a.normalizedShortcode.localeCompare(b.normalizedShortcode);
      }
      const aStarts = a.normalizedShortcode.startsWith(normalizedShortcodeQuery);
      const bStarts = b.normalizedShortcode.startsWith(normalizedShortcodeQuery);
      if (aStarts !== bStarts) return aStarts ? -1 : 1;
      return a.normalizedShortcode.localeCompare(b.normalizedShortcode);
    })
    .map(({ emoji }) => ({
      value: emoji.shortcode,
      label: `:${emoji.shortcode}:`,
      insertText: appendEmojiNoWidthSpace(`:${emoji.shortcode}:`),
      description: emoji.category ?? undefined,
      emoji,
    }));
  const unicodeItems = unicodeEmojiCategories
    .flatMap((category) => category.emojis)
    .filter((emoji) => {
      if (!normalizedQuery) return true;
      return emoji.searchText.includes(normalizedQuery);
    })
    .sort((a, b) => {
      if (!normalizedQuery) return a.name.localeCompare(b.name);
      const aName = normalizeEmojiSearchText(a.name);
      const bName = normalizeEmojiSearchText(b.name);
      const aStarts = aName.startsWith(normalizedQuery);
      const bStarts = bName.startsWith(normalizedQuery);
      if (aStarts !== bStarts) return aStarts ? -1 : 1;
      return a.name.localeCompare(b.name);
    })
    .map((emoji) => ({
      value: emoji.emoji,
      label: emoji.emoji,
      insertText: appendEmojiNoWidthSpace(emoji.emoji),
      description: emoji.name,
      unicodeEmoji: emoji,
    }));
  const customLimit = unicodeItems.length > 0 ? 4 : 8;
  const mergedItems = [
    ...customItems.slice(0, customLimit),
    ...unicodeItems.slice(0, 8 - Math.min(customItems.length, customLimit)),
  ];
  if (mergedItems.length < 8) {
    mergedItems.push(...customItems.slice(customLimit, 8));
  }
  return uniqueAutocompleteItems("emoji", mergedItems).slice(0, 8);
}

export function normalizeEmojiSearchText(value: string) {
  return value
    .toLowerCase()
    .replace(/[:_-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}
