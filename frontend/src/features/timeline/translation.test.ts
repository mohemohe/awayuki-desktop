import { afterEach, describe, expect, it } from "vitest";
import { getAppLocale, setAppLocale } from "../../i18n";
import {
  shouldOfferTranslation,
  translatedTextToHtml,
} from "./translation";

describe("timeline translation feature", () => {
  const initialLocale = getAppLocale();
  afterEach(() => setAppLocale(initialLocale));
  it("uses semantic locale rather than display labels", () => {
    setAppLocale("ja");
    expect(
      shouldOfferTranslation({ language: "en" } as never, "hello"),
    ).toBe(true);
    expect(
      shouldOfferTranslation({ language: "ja" } as never, "こんにちは"),
    ).toBe(false);
  });

  it("escapes translated plain text before rendering", () => {
    expect(translatedTextToHtml("<script>x</script>\n\nnext")).toBe(
      "<p>&lt;script&gt;x&lt;/script&gt;</p><p>next</p>",
    );
  });
});
