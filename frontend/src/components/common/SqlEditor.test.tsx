import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { KqEditor } from "./SqlEditor";

describe("KqEditor", () => {
  it("uses a dedicated KQ editor surface and highlights KQ tokens", async () => {
    const query = 'from home where text contains "snow"';
    const { container } = render(
      <KqEditor value={query} onChange={vi.fn()} />,
    );

    expect(screen.getByRole("textbox", { name: "KQ" })).toHaveTextContent(query);
    await waitFor(() => {
      const highlightedTokens = Array.from(
        container.querySelectorAll(".cm-line > span"),
      ).map((element) => element.textContent);
      expect(highlightedTokens).toEqual(
        expect.arrayContaining(["from", "home", "where", "contains", '"snow"']),
      );
    });
    expect(screen.queryByRole("textbox", { name: "YQ" })).toBeNull();
  });

  it("keeps Fediverse account and provider ID literals intact", async () => {
    const query =
      'where author.acct == @alice-smith@sub.example.social | id == #"did:plc:abc"';
    const { container } = render(
      <KqEditor value={query} onChange={vi.fn()} />,
    );

    await waitFor(() => {
      const highlightedTokens = Array.from(
        container.querySelectorAll(".cm-line > span"),
      ).map((element) => element.textContent);
      expect(highlightedTokens).toEqual(
        expect.arrayContaining([
          "@alice-smith@sub.example.social",
          '#"did:plc:abc"',
        ]),
      );
    });
  });
});
