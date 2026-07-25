import { describe, expect, it } from "vitest";

import { retainedComposeHashtags } from "./composeHashtags";

describe("retainedComposeHashtags", () => {
  it("keeps every hashtag in input order", () => {
    expect(
      retainedComposeHashtags("Opening #foo, then #bar and #実況_2026!"),
    ).toBe("#foo #bar #実況_2026");
  });

  it("does not treat URL fragments or embedded hashes as hashtags", () => {
    expect(
      retainedComposeHashtags(
        "https://example.com/#fragment word#embedded (#actual)",
      ),
    ).toBe("#actual");
  });

  it("returns an empty draft when the post has no hashtags", () => {
    expect(retainedComposeHashtags("No tags here")).toBe("");
  });
});
