import { afterEach, describe, expect, it } from "vitest";
import {
  enMessages,
  getAppLocale,
  ja,
  messageCatalog,
  resolveSupportedLocale,
  setAppLocale,
  t,
} from "./i18n";
import { formatCompactNumber } from "./utils/format";

describe("typed message catalogs", () => {
  const initialLocale = getAppLocale();
  afterEach(() => setAppLocale(initialLocale));

  it("keeps EN and JA catalogs exhaustive", () => {
    expect(Object.keys(enMessages).sort()).toEqual(Object.keys(ja).sort());
    for (const value of Object.values(messageCatalog("en"))) {
      expect(value.trim()).not.toBe("");
    }
    for (const value of Object.values(messageCatalog("ja"))) {
      expect(value.trim()).not.toBe("");
    }
  });

  it("uses the first supported locale candidate", () => {
    expect(resolveSupportedLocale(["fr-FR", "ja-JP", "en-US"])).toBe("ja");
    expect(resolveSupportedLocale(["en-GB", "ja-JP"])).toBe("en");
    expect(resolveSupportedLocale(["fr-FR"])).toBe("en");
  });

  it("supports a runtime locale change", () => {
    setAppLocale("ja");
    expect(t("timeline.empty")).toBe("読み込まれた投稿はありません。");
    setAppLocale("en");
    expect(t("timeline.empty")).toBe("No statuses loaded.");
  });

  it("uses the exact Plugins section label in both locales", () => {
    setAppLocale("ja");
    expect(t("settings.section.plugin")).toBe("プラグイン");
    setAppLocale("en");
    expect(t("settings.section.plugin")).toBe("Plugins");
  });

  it("uses the exact open-directory label in both locales", () => {
    setAppLocale("ja");
    expect(t("Open directory")).toBe("ディレクトリを開く");
    setAppLocale("en");
    expect(t("Open directory")).toBe("Open directory");
  });

  it("keeps KQ labels distinct from YQ in both locales", () => {
    setAppLocale("ja");
    expect(t("timeline.kqSlow", { scanned: 120, duration: 45 })).toBe(
      "KQが低速です: 120件を45msで評価しました",
    );
    setAppLocale("en");
    expect(t("timeline.kqSlow", { scanned: 120, duration: 45 })).toBe(
      "Slow KQ query: evaluated 120 rows in 45ms",
    );
  });

  it("uses descriptive labels for analytical timeline types", () => {
    for (const locale of ["ja", "en"] as const) {
      setAppLocale(locale);
      expect(t("timeline.custom")).toBe("SQL");
      expect(t("timeline.yq")).toBe("Yukari Query");
      expect(t("timeline.kq")).toBe("Krile Query");
    }
  });

  it("rebuilds Intl formatters from the runtime app locale", () => {
    setAppLocale("ja");
    const japanese = formatCompactNumber(12_000);
    setAppLocale("en");
    const english = formatCompactNumber(12_000);

    expect(japanese).toContain("万");
    expect(english).toMatch(/K$/i);
    expect(japanese).not.toBe(english);
  });
});
