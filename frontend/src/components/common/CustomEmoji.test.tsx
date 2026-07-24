import { describe, expect, it } from "vitest";
import { renderStatusHtmlWithCustomEmojis } from "./CustomEmoji";

describe("renderStatusHtmlWithCustomEmojis", () => {
  it("removes active content, event handlers, styles, and unsafe links", () => {
    const result = renderStatusHtmlWithCustomEmojis(
      '<p onclick="alert(1)" style="color:red">safe<script>alert(1)</script>' +
        '<a href="javascript:alert(1)">unsafe</a>' +
        '<a href="https://example.social/@user/1" onmouseover="alert(1)">safe link</a>' +
        '<img src="https://tracker.invalid/pixel" onerror="alert(1)"></p>',
      [],
    );

    const template = document.createElement("template");
    template.innerHTML = result;
    expect(template.content.querySelector("script,img,style,form,iframe")).toBeNull();
    expect(result).not.toContain("onclick");
    expect(result).not.toContain("onmouseover");
    expect(result).not.toContain("javascript:");
    expect(result).not.toContain("color:red");

    const links = template.content.querySelectorAll("a");
    expect(links).toHaveLength(1);
    expect(links[0]?.getAttribute("href")).toBe(
      "https://example.social/@user/1",
    );
    expect(links[0]?.getAttribute("rel")).toBe("nofollow noopener noreferrer");
    expect(links[0]?.getAttribute("target")).toBe("_blank");
  });

  it("only inserts custom emoji images with safe HTTP sources", () => {
    const safe = renderStatusHtmlWithCustomEmojis("<p>:party:</p>", [
      {
        shortcode: "party",
        url: "https://cdn.example/party.png",
        staticUrl: "https://cdn.example/party-static.png",
      },
    ]);
    const unsafe = renderStatusHtmlWithCustomEmojis("<p>:party:</p>", [
      {
        shortcode: "party",
        url: "data:image/svg+xml,unsafe",
        staticUrl: "javascript:alert(1)",
      },
    ]);

    expect(safe).toContain('class="status-custom-emoji"');
    expect(safe).toContain("https://cdn.example/party.png");
    expect(unsafe).toBe("<p>:party:</p>");
  });

  it("drops SVG namespaces and active descendants even when markup is malformed", () => {
    const result = renderStatusHtmlWithCustomEmojis(
      '<p>before<svg onload="alert(1)"><foreignObject><iframe srcdoc="unsafe">' +
        '</iframe></foreignObject><a xlink:href="javascript:alert(1)">svg link</a></svg>' +
        '<b><i>kept</b><img src=x onerror="alert(1)">after',
      [],
    );

    const template = document.createElement("template");
    template.innerHTML = result;
    expect(
      template.content.querySelector(
        "svg,foreignObject,iframe,img,script,style,object,embed",
      ),
    ).toBeNull();
    expect(result).not.toMatch(/onload|onerror|srcdoc|xlink:href|javascript:/i);
    expect(template.content.textContent).toContain("before");
    expect(template.content.textContent).toContain("kept");
    expect(template.content.textContent).toContain("after");
  });

  it("preserves mention, hashtag, paragraphs, and explicit line breaks", () => {
    const result = renderStatusHtmlWithCustomEmojis(
      '<p>Hello <span class="h-card"><a class="u-url mention extra" ' +
        'href="https://social.example/@alice">@<span>alice</span></a></span><br>' +
        '<a class="hashtag" href="http://social.example/tags/rust">#rust</a>\nnext</p>',
      [],
    );

    const template = document.createElement("template");
    template.innerHTML = result;
    const links = template.content.querySelectorAll("a");
    expect(links).toHaveLength(2);
    expect(links[0]?.className).toBe("u-url mention");
    expect(links[1]?.className).toBe("hashtag");
    expect(template.content.querySelector("br")).not.toBeNull();
    expect(template.content.textContent).toContain("next");
    links.forEach((link) => {
      expect(link.getAttribute("rel")).toBe("nofollow noopener noreferrer");
      expect(link.getAttribute("target")).toBe("_blank");
    });
  });

  it.each([
    "javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "file:///etc/passwd",
    "ftp://files.example/archive",
    "mailto:alice@example.com",
    "//example.com/scheme-relative",
    "httpsx://example.com/typo",
  ])("unwraps links using a disallowed scheme: %s", (href) => {
    const result = renderStatusHtmlWithCustomEmojis(
      `<a class="mention" href="${href}">safe label</a>`,
      [],
    );

    const template = document.createElement("template");
    template.innerHTML = result;
    expect(template.content.querySelector("a")).toBeNull();
    expect(template.content.textContent).toBe("safe label");
  });

  it.each([
    "https://example.com/path?x=1#fragment",
    "http://127.0.0.1:8080/@alice",
  ])("retains an explicitly allowed HTTP link: %s", (href) => {
    const result = renderStatusHtmlWithCustomEmojis(
      `<a href="${href}">link</a>`,
      [],
    );
    const template = document.createElement("template");
    template.innerHTML = result;
    expect(template.content.querySelector("a")?.getAttribute("href")).toBe(href);
  });
});
