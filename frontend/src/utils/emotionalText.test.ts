import { describe, expect, it } from "vitest";
import {
  emotionalizeComposeText,
  emotionalTextOptions,
  toEmotionalText,
} from "./emotionalText";

describe("Emotional Text", () => {
  it.each([
    ["boldSerif", "𝐀𝐚"],
    ["italicSerif", "𝐴𝑎"],
    ["boldItalicSerif", "𝑨𝒂"],
    ["boldSansSerif", "𝗔𝗮"],
    ["italicSansSerif", "𝘈𝘢"],
    ["boldItalicSansSerif", "𝘼𝙖"],
    ["boldScript", "𝓐𝓪"],
    ["fraktur", "𝔄𝔞"],
    ["frakturBold", "𝕬𝖆"],
    ["monoSpace", "𝙰𝚊"],
  ] as const)("converts ASCII letters using %s", (style, expected) => {
    expect(toEmotionalText("Aa", style)).toBe(expected);
  });

  it("uses the Unicode letterlike-symbol exceptions", () => {
    expect(toEmotionalText("h", "italicSerif")).toBe("ℎ");
    expect(toEmotionalText("CHIRZ", "fraktur")).toBe("ℭℌℑℜℨ");
  });

  it("keeps mentions and hashtags unchanged while preserving whitespace", () => {
    expect(
      emotionalizeComposeText("Hello @Awayuki\n#Fediverse world", "boldSerif"),
    ).toBe("𝐇𝐞𝐥𝐥𝐨 @Awayuki\n#Fediverse 𝐰𝐨𝐫𝐥𝐝");
  });

  it("exposes the ten styles from the reference implementation", () => {
    expect(emotionalTextOptions).toHaveLength(10);
  });
});
