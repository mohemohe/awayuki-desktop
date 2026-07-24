import { describe, expect, it } from "vitest";
import {
  detectComposeAutocomplete,
  emojiAutocompleteItems,
  indexedCustomEmojis,
  uniqueAutocompleteItems,
} from "./composeAutocomplete";
import type { CustomEmojiSummary } from "../types/app";

describe("compose autocomplete service", () => {
  it("detects the resource at the caret and rejects malformed tokens", () => {
    expect(detectComposeAutocomplete("hello @awayuki", 14)).toMatchObject({
      kind: "mention",
      query: "awayuki",
      start: 6,
    });
    expect(detectComposeAutocomplete("#fediverse", 10)).toMatchObject({
      kind: "hashtag",
      query: "fediverse",
    });
    expect(detectComposeAutocomplete("hello :blobcat", 14)).toMatchObject({
      kind: "emoji",
      query: "blobcat",
    });
    expect(detectComposeAutocomplete("https://example.com", 19)).toBeNull();
  });

  it("pre-indexes custom emoji and caps suggestions at eight", () => {
    const emojis = Array.from({ length: 20 }, (_, index) =>
      emoji(`awayuki_${index}`),
    );
    expect(indexedCustomEmojis(emojis)).toBe(indexedCustomEmojis(emojis));

    const suggestions = emojiAutocompleteItems(emojis, [], "awayuki");
    expect(suggestions).toHaveLength(8);
    expect(new Set(suggestions.map((item) => item.value)).size).toBe(8);
  });

  it("deduplicates provider results by normalized identity", () => {
    expect(
      uniqueAutocompleteItems("mention", [
        { value: "@User", label: "first" },
        { value: "user", label: "duplicate" },
      ]),
    ).toEqual([{ value: "@User", label: "first" }]);
  });
});

function emoji(shortcode: string): CustomEmojiSummary {
  return {
    shortcode,
    url: `https://example.com/${shortcode}.png`,
    staticUrl: `https://example.com/${shortcode}.png`,
    category: "Awayuki",
  };
}
