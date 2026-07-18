export type EmotionalTextStyle =
  | "boldSerif"
  | "italicSerif"
  | "boldItalicSerif"
  | "boldSansSerif"
  | "italicSansSerif"
  | "boldItalicSansSerif"
  | "boldScript"
  | "fraktur"
  | "frakturBold"
  | "monoSpace";

type EmotionalCodePoints = {
  upperA: number;
  lowerA: number;
  mapping?: Readonly<Record<string, string>>;
};

const emotionalCodePoints: Record<EmotionalTextStyle, EmotionalCodePoints> = {
  boldSerif: { upperA: 0x1d400, lowerA: 0x1d41a },
  italicSerif: {
    upperA: 0x1d434,
    lowerA: 0x1d44e,
    mapping: { h: "ℎ" },
  },
  boldItalicSerif: { upperA: 0x1d468, lowerA: 0x1d482 },
  boldSansSerif: { upperA: 0x1d5d4, lowerA: 0x1d5ee },
  italicSansSerif: { upperA: 0x1d608, lowerA: 0x1d622 },
  boldItalicSansSerif: { upperA: 0x1d63c, lowerA: 0x1d656 },
  boldScript: { upperA: 0x1d4d0, lowerA: 0x1d4ea },
  fraktur: {
    upperA: 0x1d504,
    lowerA: 0x1d51e,
    mapping: {
      C: "ℭ",
      H: "ℌ",
      I: "ℑ",
      R: "ℜ",
      Z: "ℨ",
    },
  },
  frakturBold: { upperA: 0x1d56c, lowerA: 0x1d586 },
  monoSpace: { upperA: 0x1d670, lowerA: 0x1d68a },
};

export const emotionalTextOptions: ReadonlyArray<{
  value: EmotionalTextStyle;
  label: string;
}> = [
  { value: "boldSerif", label: "Serif (bold)" },
  { value: "italicSerif", label: "Serif (italic)" },
  { value: "boldItalicSerif", label: "Serif (bold italic)" },
  { value: "boldSansSerif", label: "Sans-serif (bold)" },
  { value: "italicSansSerif", label: "Sans-serif (italic)" },
  { value: "boldItalicSansSerif", label: "Sans-serif (bold italic)" },
  { value: "boldScript", label: "Script (bold)" },
  { value: "fraktur", label: "Fraktur" },
  { value: "frakturBold", label: "Fraktur (bold)" },
  { value: "monoSpace", label: "Monospace" },
];

export function toEmotionalText(text: string, style: EmotionalTextStyle) {
  const target = emotionalCodePoints[style];
  return text.replace(/[A-Za-z]/g, (character) => {
    const mapped = target.mapping?.[character];
    if (mapped) return mapped;
    const isUppercase = character >= "A" && character <= "Z";
    const base = isUppercase ? 0x41 : 0x61;
    const targetBase = isUppercase ? target.upperA : target.lowerA;
    return String.fromCodePoint(character.charCodeAt(0) - base + targetBase);
  });
}

export function emotionalizeComposeText(
  text: string,
  style: EmotionalTextStyle,
) {
  return text
    .split(/(\s+)/)
    .map((token) =>
      token.startsWith("@") || token.startsWith("#")
        ? token
        : toEmotionalText(token, style),
    )
    .join("");
}
