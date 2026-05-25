import type { AppStore } from "../store/appStore";
import type { PresetVisibilitySettings } from "../types/app";

export const presetVisibilityValues = [
  "Public",
  "Unlisted",
  "Private",
  "Direct",
] as const;

const presetToComposeVisibility = {
  Public: "public",
  Unlisted: "unlisted",
  Private: "private",
  Direct: "direct",
} satisfies Record<
  (typeof presetVisibilityValues)[number],
  AppStore["visibility"]
>;

export function matchPresetVisibility(
  settings: PresetVisibilitySettings | undefined,
  text: string,
): AppStore["visibility"] | undefined {
  const lowerText = text.toLocaleLowerCase();
  for (const entry of settings?.entries ?? []) {
    const keyword = entry.keyword.trim();
    if (!keyword) continue;
    if (lowerText.includes(keyword.toLocaleLowerCase())) {
      return presetToComposeVisibility[entry.visibility];
    }
  }
  return undefined;
}
