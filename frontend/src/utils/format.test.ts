import { describe, expect, it } from "vitest";
import { htmlToPlainText, statusEditText } from "./format";

describe("statusEditText", () => {
  it("preserves full accounts, plain text, hashtags, and ordinary links", () => {
    const content = '<p>@plain <a class="mention" href="https://remote.example/@full">@full@account.example</a> <a href="https://example.com/@link">@link</a> <a class="mention hashtag" href="https://example.com/tags/tag">#tag</a></p>';
    expect(statusEditText({ content })).toBe("@plain @full@account.example @link #tag");
  });

  it("uses a qualified account in a remote profile path", () => {
    const content = '<a class="mention" href="https://local.example/@aiwas@yysk.icu">@aiwas</a>';
    expect(statusEditText({ content })).toBe("@aiwas@yysk.icu");
    expect(htmlToPlainText(content)).toBe("@aiwas");
  });

  it("preserves unsupported or mismatched mention links", () => {
    const content = '<a class="mention" href="invalid">@one</a> <a class="mention" href="https://example.com/@other">@two</a>';
    expect(statusEditText({ content })).toBe("@one @two");
  });
});

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
