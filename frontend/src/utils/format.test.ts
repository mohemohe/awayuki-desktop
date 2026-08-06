import { describe, expect, it } from "vitest";
import { htmlToPlainText } from "./format";

describe("htmlToPlainText", () => {
  it("preserves line breaks in copied status text", () => {
    expect(
      htmlToPlainText(
        "<p>暑いからと扇風機直撃にしたら3分と経たずに<br>#ponponpainになって草<br>弱すぎる</p>",
      ),
    ).toBe(
      "暑いからと扇風機直撃にしたら3分と経たずに\n#ponponpainになって草\n弱すぎる",
    );
  });

  it("keeps paragraph boundaries as line breaks", () => {
    expect(htmlToPlainText("<p>first paragraph</p><p>second paragraph</p>"))
      .toBe("first paragraph\nsecond paragraph");
  });

  it("decodes nested HTML entities for plain-text consumers", () => {
    expect(
      htmlToPlainText(
        "<p>&amp;#34;&amp;#34;&amp;#34;マイナアプリ&amp;#34;&amp;#34;&amp;#34;</p>",
      ),
    ).toBe('"""マイナアプリ"""');
  });
});
