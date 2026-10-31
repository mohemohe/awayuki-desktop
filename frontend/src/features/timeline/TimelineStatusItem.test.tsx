import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import { canonicalStatusKey } from "../../domain/timelineEntities";
import type {
  AppSnapshot,
  ColumnSummary,
  TimelineStatus,
} from "../../types/app";
import { StatusItem } from "./TimelineStatusItem";

const openThreadPane = vi.fn();
const openMediaPreview = vi.fn();
const writeText = vi.fn();

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

describe("StatusItem", () => {
  beforeEach(() => {
    openThreadPane.mockReset();
    openMediaPreview.mockReset();
    writeText.mockReset();
    writeText.mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
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

  describe.each(["post", "quoted post"])("attachments in a %s", (context) => {
    function renderAttachments(count: number, sensitive = false) {
      const attachmentStatus = status("attachments", {
        sensitive,
        media: Array.from({ length: count }, (_, index) => ({
          id: `media-${index}`,
          media_type: "image/png",
          url: `https://example.test/media/${index}.png`,
          preview_url: `https://example.test/media/${index}-preview.png`,
          description: `Attachment ${index + 1}`,
        })),
      });
      render(
        <StatusItem
          column={column}
          status={
            context === "quoted post"
              ? status("parent", { quote: attachmentStatus })
              : attachmentStatus
          }
        />,
      );
      return attachmentStatus;
    }

    it.each([5, 6])(
      "renders all %i attachments in order and opens each preview",
      (count) => {
        const attachmentStatus = renderAttachments(count);
        const thumbnails = screen.getAllByRole("img", { name: /^Attachment / });
        const previewButtons = screen.getAllByTitle("Open media preview");

        expect(thumbnails).toHaveLength(count);
        expect(previewButtons).toHaveLength(count);
        attachmentStatus.media.forEach((media, index) => {
          expect(thumbnails[index]).toHaveAttribute("src", media.preview_url);
          expect(thumbnails[index]).toHaveAttribute("alt", media.description);
          fireEvent.click(previewButtons[index]);
          expect(openMediaPreview).toHaveBeenNthCalledWith(
            index + 1,
            attachmentStatus,
            media,
          );
        });
      },
    );

    it("keeps the fifth sensitive attachment hidden until revealed", () => {
      const attachmentStatus = renderAttachments(5, true);
      expect(screen.queryByRole("img", { name: "Attachment 5" })).toBeNull();

      const revealButtons = screen.getAllByTitle("Reveal media");
      fireEvent.click(revealButtons[revealButtons.length - 1]);

      expect(screen.getByRole("img", { name: "Attachment 5" })).toHaveAttribute(
        "src",
        attachmentStatus.media[4].preview_url,
      );
      expect(openMediaPreview).not.toHaveBeenCalled();
      fireEvent.click(screen.getByTitle("Open media preview"));
      expect(openMediaPreview).toHaveBeenCalledWith(
        attachmentStatus,
        attachmentStatus.media[4],
      );
    });
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

  it("does not render mutation progress below the post", () => {
    const pending = status("pending-bookmark");
    useAppStore.setState({
      statusMutations: {
        [canonicalStatusKey(pending)]: {
          operationId: "11111111-1111-4111-8111-111111111111",
          phase: "pending",
          beforeImage: pending,
        },
      },
    });

    const { container } = render(
      <StatusItem column={column} status={pending} />,
    );

    expect(container.querySelector('[role="status"]')).toBeNull();
  });

  it("shows only the CW title in compact Mystique mode when CW content is hidden", () => {
    const snapshot = useAppStore.getState().snapshot!;
    useAppStore.setState({
      snapshot: {
        ...snapshot,
        settings: {
          ...snapshot.settings,
          appearance: {
            ...snapshot.settings.appearance,
            display_mode: "Mystique",
            cw_behavior: "Hide",
          },
        },
      },
    });

    render(
      <StatusItem
        column={column}
        status={status("cw-post", {
          spoilerText: "CW title",
          content: "<p>hidden CW content</p>",
        })}
      />,
    );

    const compactPost = screen.getByTitle("Expand post");
    expect(compactPost).toHaveTextContent("CW title");
    expect(compactPost).not.toHaveTextContent("hidden CW content");
  });

  it("copies status text without dropping its line breaks", async () => {
    render(
      <StatusItem
        column={column}
        status={status("multiline", {
          content:
            "<p>暑いからと扇風機直撃にしたら3分と経たずに<br>#ponponpainになって草<br>弱すぎる</p>",
        })}
      />,
    );

    fireEvent.click(screen.getByTitle("More"));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "Copy text" }),
    );

    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        "暑いからと扇風機直撃にしたら3分と経たずに\n#ponponpainになって草\n弱すぎる",
      ),
    );
  });

  it("renders Misskey code blocks with their preformatted contents", () => {
    const code = "View the Web UI:\nhttps://example.test/very-long-url";
    const { container } = render(
      <StatusItem
        column={column}
        status={status("code-block", {
          content: `<p>Using tunnel</p><pre><code>${code}</code></pre><p>うそつけｗ</p>`,
        })}
      />,
    );

    const content = container.querySelector(".status-content");
    const pre = content?.querySelector("pre");
    expect(content).not.toBeNull();
    expect(pre).not.toBeNull();
    expect(pre?.querySelector("code")?.textContent).toBe(code);
  });
});
