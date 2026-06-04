import fullEmojiList from "full-emoji-list";

export type UnicodeEmojiItem = {
  emoji: string;
  name: string;
  group: string;
  subGroup: string;
  searchText: string;
  codePointsHex: string[];
};

export type UnicodeEmojiCategory = {
  name: string;
  icon: string;
  emojis: UnicodeEmojiItem[];
};

const emojiCategoryIcons: Record<string, string> = {
  "Smileys & Emotion": "😀",
  "People & Body": "👋",
  Component: "🏻",
  "Animals & Nature": "🐻",
  "Food & Drink": "🍔",
  "Travel & Places": "✈️",
  Activities: "⚽",
  Objects: "💡",
  Symbols: "❤️",
  Flags: "🏳️",
};

const normalizeEmojiSearchText = (value: string) =>
  value
    .toLowerCase()
    .replace(/[:_-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();

export const unicodeEmojiCategories: UnicodeEmojiCategory[] = (() => {
  const categories = new Map<string, UnicodeEmojiItem[]>();

  for (const item of fullEmojiList) {
    if (!item.Emoji) continue;

    const name = item.Name ?? "";
    const group = item.Group ?? "Other";
    const subGroup = item.SubGroup ?? "";
    const codePointsHex = item.CodePointsHex ?? [];
    const searchText = normalizeEmojiSearchText(
      [item.Emoji, name, group, subGroup, ...codePointsHex].join(" "),
    );
    const emoji: UnicodeEmojiItem = {
      emoji: item.Emoji,
      name,
      group,
      subGroup,
      searchText,
      codePointsHex,
    };

    const emojis = categories.get(group) ?? [];
    emojis.push(emoji);
    categories.set(group, emojis);
  }

  return [...categories.entries()].map(([name, emojis]) => ({
    name,
    icon: emojiCategoryIcons[name] ?? emojis[0]?.emoji ?? "😀",
    emojis,
  }));
})();
