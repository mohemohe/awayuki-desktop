import { describe, expect, it } from "vitest";

import {
  appearanceThemeAttribute,
  appearanceThemes,
  applyAppearanceTheme,
} from "./theme";

describe("appearance theme", () => {
  it("exposes all Catppuccin flavors", () => {
    expect(appearanceThemes).toEqual([
      "Latte",
      "Frappe",
      "Macchiato",
      "Mocha",
    ]);
  });

  it.each([
    ["Latte", "catppuccin-latte"],
    ["Frappe", "catppuccin-frappe"],
    ["Macchiato", "catppuccin-macchiato"],
    ["Mocha", "catppuccin-mocha"],
  ] as const)("maps %s to its DaisyUI theme", (theme, expected) => {
    expect(appearanceThemeAttribute(theme)).toBe(expected);
  });

  it("applies the selected theme to the document root", () => {
    const root = document.createElement("html");

    applyAppearanceTheme("Latte", root);

    expect(root.dataset.theme).toBe("catppuccin-latte");
  });
});
