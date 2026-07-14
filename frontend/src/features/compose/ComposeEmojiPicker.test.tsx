import React from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ComposeEmojiPicker } from "./ComposeEmojiPicker";

vi.mock("react-virtuoso", () => ({
  VirtuosoGrid: ({
    className,
    data,
    itemContent,
    style,
  }: {
    className?: string;
    data: Array<{ emoji: string }>;
    itemContent: (index: number, item: { emoji: string }) => React.ReactNode;
    style?: React.CSSProperties;
  }) => (
    <div className={className} data-testid="emoji-grid" style={style}>
      {data.map((item, index) => itemContent(index, item))}
    </div>
  ),
}));

describe("ComposeEmojiPicker", () => {
  it("portals the picker outside the clipped compose pane", () => {
    const anchor = document.createElement("div");
    anchor.getBoundingClientRect = vi.fn(() => ({
      bottom: 112,
      height: 112,
      left: 52,
      right: 420,
      top: 0,
      width: 368,
      x: 52,
      y: 0,
      toJSON: () => ({}),
    }));
    const host = document.createElement("div");
    document.body.append(host);

    render(
      <ComposeEmojiPicker
        anchorRef={{ current: anchor }}
        customEmojis={[]}
        unicodeEmojiCategories={[
          {
            name: "Smileys & Emotion",
            icon: "😀",
            emojis: [
              {
                emoji: "😀",
                name: "grinning face",
                group: "Smileys & Emotion",
                subGroup: "face-smiling",
                searchText: "grinning face",
                codePointsHex: ["1F600"],
              },
            ],
          },
        ]}
        onPickEmoji={vi.fn()}
      />,
      { container: host },
    );

    const grid = screen.getByTestId("emoji-grid");
    expect(grid).toHaveStyle({ height: "240px" });
    expect(host).not.toContainElement(grid);
    expect(grid.parentElement).toHaveStyle({ left: "60px", top: "116px" });
    expect(screen.getByTitle("grinning face")).toBeVisible();
  });
});
