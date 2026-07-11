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
});
