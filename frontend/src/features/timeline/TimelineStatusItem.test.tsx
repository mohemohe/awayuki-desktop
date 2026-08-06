import React from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import type {
  AppSnapshot,
  ColumnSummary,
  TimelineStatus,
} from "../../types/app";
import { StatusItem } from "./TimelineStatusItem";

const openThreadPane = vi.fn();
const openMediaPreview = vi.fn();

const column: ColumnSummary = {
  id: "home",
  columnType: "home",
  name: "Home",
  maxStatuses: 100,
  paneIndex: 0,
  position: 0,
};

function status(
  id: string,
  overrides: Partial<TimelineStatus> = {},
): TimelineStatus {
  return {
    id,
    originalStatusId: id,
    statusIdentity: {
      protocol: "activityPub",
      serverDomain: "example.test",
      canonicalUri: `https://example.test/statuses/${id}`,
      remoteId: id,
    },
    sourceAcct: null,
    accountId: "account",
    serverDomain: "example.test",
    uri: `https://example.test/statuses/${id}`,
    url: `https://example.test/statuses/${id}`,
    displayName: "User",
    acct: "user@example.test",
    avatar: "",
    createdAt: "2026-07-16T02:38:35.000Z",
    content: `<p>${id}</p>`,
    spoilerText: "",
    reblogsCount: 0,
    favouritesCount: 0,
    repliesCount: 0,
    visibility: "public",
    sensitive: false,
    favourited: false,
    reblogged: false,
    bookmarked: false,
    media: [],
    emojis: [],
    accountEmojis: [],
    ...overrides,
  };
}

describe("quoted status preview", () => {
  beforeEach(() => {
    openThreadPane.mockReset();
    openMediaPreview.mockReset();
    useAppStore.setState({
      snapshot: {
        version: "test",
        accounts: [],
        activeAcct: null,
        columns: [column],
        settings: {
          appearance: {
            cw_behavior: "Hide",
            nsfw_behavior: "Hide",
            font_size: "Medium",
            display_mode: "StarryEyes",
          },
          confirmation: { media_source: "Local" },
          accountSourceColors: { "source@example.test": "Mauve" },
        },
        database: {},
      } as unknown as AppSnapshot,
      openThreadPane,
      openMediaPreview,
    });
  });

  it("opens the quoted post detail and renders its media thumbnail", () => {
    const quoted = status("quoted", {
      media: [
        {
          id: "quoted-media",
          media_type: "image/png",
          url: "https://example.test/media/original.png",
          preview_url: "https://example.test/media/preview.png",
          description: "Quoted attachment",
        },
      ],
    });

    render(
      <StatusItem
        column={column}
        status={status("parent", { quote: quoted })}
      />,
    );

    fireEvent.click(screen.getByTitle("Open quoted post"));
    expect(openThreadPane).toHaveBeenCalledWith(quoted);

    const thumbnail = screen.getByRole("img", { name: "Quoted attachment" });
    expect(thumbnail).toHaveAttribute(
      "src",
      "https://example.test/media/preview.png",
    );

    fireEvent.click(screen.getByTitle("Open media preview"));
    expect(openMediaPreview).toHaveBeenCalledWith(quoted, quoted.media[0]);
  });

  it("uses only the timeline source border for replies", () => {
    const { container } = render(
      <StatusItem
        column={column}
        status={status("reply", {
          sourceAcct: "source@example.test",
          inReplyToId: "parent",
        })}
      />,
    );

    const reply = container.querySelector("article");
    expect(reply).toHaveClass("px-3");
    expect(reply?.style.paddingLeft).toBe("");
    expect(reply).toHaveStyle({
      borderLeftColor: "rgb(var(--ctp-mauve))",
    });
    expect(reply?.querySelector(":scope > span")).toBeNull();
  });
});
