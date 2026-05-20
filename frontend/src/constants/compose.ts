import fullEmojiList from "full-emoji-list";

export const pollDurations = [
  { label: "5m", labelJa: "5 分", seconds: 5 * 60 },
  { label: "30m", labelJa: "30 分", seconds: 30 * 60 },
  { label: "1h", labelJa: "1 時間", seconds: 60 * 60 },
  { label: "6h", labelJa: "6 時間", seconds: 6 * 60 * 60 },
  { label: "12h", labelJa: "12 時間", seconds: 12 * 60 * 60 },
  { label: "1d", labelJa: "1 日", seconds: 24 * 60 * 60 },
  { label: "3d", labelJa: "3 日", seconds: 3 * 24 * 60 * 60 },
  { label: "7d", labelJa: "7 日", seconds: 7 * 24 * 60 * 60 },
];

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

const normalizeEmojiSearchText = (value: string) =>
  value
    .toLowerCase()
    .replace(/[:_-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();

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

export function pollDurationLabel(seconds: number) {
  const duration = pollDurations.find((item) => item.seconds === seconds);
  return duration?.label ?? "1d";
}
