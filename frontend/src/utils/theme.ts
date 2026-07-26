import type { AppearanceSettings } from "../types/app";

export const appearanceThemes = [
  "Latte",
  "Frappe",
  "Macchiato",
  "Mocha",
] as const satisfies readonly AppearanceSettings["theme"][];

const themeAttributes = {
  Latte: "catppuccin-latte",
  Frappe: "catppuccin-frappe",
  Macchiato: "catppuccin-macchiato",
  Mocha: "catppuccin-mocha",
} as const satisfies Record<AppearanceSettings["theme"], string>;

export function appearanceThemeAttribute(
  theme: AppearanceSettings["theme"],
) {
  return themeAttributes[theme];
}

export function applyAppearanceTheme(
  theme: AppearanceSettings["theme"],
  root: HTMLElement = document.documentElement,
) {
  root.dataset.theme = appearanceThemeAttribute(theme);
}
