import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  CUSTOM_TIMELINE_QUERY_EXAMPLES,
  CUSTOM_TIMELINE_SCHEMA,
  IcuTokenConverter,
  KQ_REFERENCE,
  TimelineSettingsPanel,
} from "./TimelineSettingsPanel";
import { useAppStore } from "../../store/appStore";
import type { AppSnapshot } from "../../types/app";

const api = vi.hoisted(() => ({
  invokeTypedReadCommand: vi.fn(),
}));

vi.mock("../../api/tauri", () => api);

describe("custom timeline schema reference", () => {
  it("documents every public table available to custom timeline SQL", () => {
    const schema = new Map(
      CUSTOM_TIMELINE_SCHEMA.map((section) => [
        section.label,
        [...section.values],
      ]),
    );

    expect([...schema.keys()]).toEqual([
      "statuses",
      "accounts",
      "notifications",
      "timeline_entries",
      "status_tags",
      "status_viewer_state",
      "tags",
      "status_search_icu_content",
      "status_search_icu_fts",
      "account_search_icu_content",
      "account_search_icu_fts",
    ]);
    expect(schema.get("notifications")).toEqual([
      "id",
      "server_domain",
      "account_acct",
      "notification_type",
      "created_at",
      "account_id",
      "status_id",
      "read_at",
      "fetched_at",
    ]);
    expect(schema.get("status_tags")).toEqual([
      "status_id",
      "server_domain",
      "tag_name",
    ]);
    expect(schema.get("status_viewer_state")).toEqual([
      "login_account_acct",
      "status_id",
      "server_domain",
      "favourited",
      "reblogged",
      "muted",
      "bookmarked",
      "pinned",
      "updated_at",
    ]);
    expect(schema.get("tags")).toEqual(["name", "server_domain"]);
    expect(schema.get("status_search_icu_content")).toEqual([
      "docid",
      "status_id",
      "server_domain",
      "token_text",
      "text_scope_version",
    ]);
    expect(schema.get("status_search_icu_fts")).toEqual([
      "rowid",
      "token_text",
    ]);
    expect(schema.get("account_search_icu_content")).toEqual([
      "docid",
      "account_id",
      "server_domain",
      "token_text",
    ]);
    expect(schema.get("account_search_icu_fts")).toEqual([
      "rowid",
      "token_text",
    ]);
  });

  it("provides executable hashtag and FTS query examples in the documented order", () => {
    const examples = new Map(
      CUSTOM_TIMELINE_QUERY_EXAMPLES.map((example) => [
        example.label,
        example.sql,
      ]),
    );

    expect(
      CUSTOM_TIMELINE_QUERY_EXAMPLES.slice(0, 3).map((example) => example.label),
    ).toEqual(["Latest statuses", "Hashtag search", "Status full-text search"]);
    expect(examples.get("Latest statuses")).toContain("FROM statuses");
    expect(examples.get("Hashtag search")).toContain("FROM status_tags");
    expect(examples.get("Hashtag search")).toContain(
      "IN ('hashtag1', 'hashtag2')",
    );
    expect(examples.get("Status full-text search")).toContain(
      "status_search_icu_fts MATCH",
    );
    expect(examples.get("Status full-text search")).toContain(
      "JOIN status_search_icu_content",
    );
    expect(examples.get("Status full-text search")).toContain(
      "statuses.content and statuses.spoiler_text only",
    );
    expect(examples.get("Account full-text search")).toContain(
      "account_search_icu_fts MATCH",
    );
    expect(examples.get("Account full-text search")).toContain(
      "JOIN account_search_icu_content",
    );
  });
});

describe("KQ reference", () => {
  it("documents the KQ grammar separately from YQ", () => {
    const reference = new Map<string, string[]>(
      KQ_REFERENCE.map((section) => [section.label, [...section.values]]),
    );

    expect(reference.get("syntax")).toContain(
      "from <source>[, <source>...] [where <expression>]",
    );
    expect(reference.get("sources")).toEqual(
      expect.arrayContaining([
        "local",
        "home",
        "mentions",
        'list:"list-id"',
        'search:"keyword"',
        "public",
        "local_public",
        'hashtag:"tag"',
        "bookmarks",
        "favourites",
      ]),
    );
    expect(reference.get("status variables")).toEqual(
      expect.arrayContaining([
        "text",
        "reblog",
        "has_media",
        "visibility",
      ]),
    );
    expect(reference.get("account variables")).toEqual(
      expect.arrayContaining([
        "author.acct",
        "author.username",
        "author.locked",
        "booster.acct",
        "booster.locked",
      ]),
    );
    expect(reference.get("viewer variables")).toEqual(
      expect.arrayContaining([
        "viewer.favourited",
        "viewer.reblogged",
        "viewer.pinned",
      ]),
    );
    expect(reference.get("reply & quote variables")).toEqual(
      expect.arrayContaining([
        "reply.id",
        "quote.id",
        "quote.text",
        "quote.author.acct",
      ]),
    );
    expect(reference.get("reply & quote variables")).not.toContain(
      "quote.state",
    );
    expect(reference.get("media & poll variables")).toEqual(
      expect.arrayContaining([
        "media.types",
        "has_image",
        "has_video",
        "has_audio",
        "poll.options",
        "has_card",
      ]),
    );
    expect(reference.get("operators")).toEqual(
      expect.arrayContaining([
        "&&",
        "||",
        "contains",
        "in",
        "startswith",
        "endswith",
        "regex",
        "caseful",
      ]),
    );
    expect(reference.get("operators")).not.toEqual(
      expect.arrayContaining(["and", "or", "not"]),
    );
    expect(reference.get("Awayuki operator extensions")).toEqual([
      "and",
      "or",
      "not",
    ]);
    expect(reference.get("functions")).toBeUndefined();
  });

  it("renders the KQ-specific editor and inline reference", async () => {
    const previousSnapshot = useAppStore.getState().snapshot;
    useAppStore.setState({
      snapshot: {
        version: "test",
        accounts: [],
        activeAcct: null,
        columns: [
          {
            id: "kq",
            columnType: "kq",
            columnParam: 'where text contains "snow"',
            name: "KQ",
            maxStatuses: 100,
            paneIndex: 0,
            position: 0,
          },
        ],
        settings: { appearance: { theme: "Mocha" } },
        database: {},
      } as unknown as AppSnapshot,
    });

    const { unmount } = render(createElement(TimelineSettingsPanel));
    expect(await screen.findByRole("textbox", { name: "KQ" })).toHaveTextContent(
      'where text contains "snow"',
    );
    expect(screen.getByText("KQ Reference")).toBeVisible();
    const referenceLink = screen.getByRole("link", {
      name: "Krile Query Language",
    });
    expect(referenceLink).toHaveAttribute(
      "href",
      "https://github.com/mohemohe/awayuki-desktop/blob/main/docs/kq-query-reference.md",
    );
    expect(referenceLink).toHaveAttribute("target", "_blank");
    expect(screen.queryByRole("textbox", { name: "YQ" })).toBeNull();

    unmount();
    useAppStore.setState({ snapshot: previousSnapshot });
  });
});

describe("pane notification settings", () => {
  it("renders only the notification controls directly below the name", () => {
    const previousSnapshot = useAppStore.getState().snapshot;
    useAppStore.setState({
      snapshot: {
        version: "test",
        accounts: [],
        activeAcct: null,
        columns: [
          {
            id: "custom",
            columnType: "custom",
            columnParam: "SELECT * FROM statuses LIMIT 100",
            name: "Custom",
            maxStatuses: 100,
            paneIndex: 0,
            position: 0,
            desktopNotifications: true,
            notificationSound: null,
          },
        ],
        settings: { appearance: { theme: "Mocha" } },
        database: {},
      } as unknown as AppSnapshot,
    });

    const { unmount } = render(createElement(TimelineSettingsPanel));
    const name = screen.getByRole("textbox", { name: "Name" });
    const desktopNotifications = screen.getByRole("checkbox", {
      name: "Desktop notifications",
    });
    const notificationSound = screen.getByRole("combobox", {
      name: "Notification sound",
    });
    const type = screen.getByRole("combobox", { name: "Type" });

    expect(name.compareDocumentPosition(desktopNotifications)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(desktopNotifications.compareDocumentPosition(notificationSound)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(notificationSound.compareDocumentPosition(type)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(screen.queryByText("Pane settings")).toBeNull();

    unmount();
    useAppStore.setState({ snapshot: previousSnapshot });
  });
});

describe("ICU token converter", () => {
  beforeEach(() => {
    api.invokeTypedReadCommand.mockReset();
  });

  it("shows and copies the backend expression after index normalization", async () => {
    api.invokeTypedReadCommand.mockResolvedValueOnce('"x66663134"*');
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(createElement(IcuTokenConverter));

    const copyButton = screen.getByRole("button", { name: "Copy" });
    expect(copyButton).toBeDisabled();

    fireEvent.change(screen.getByRole("textbox", { name: "Search term" }), {
      target: { value: "FF14" },
    });

    await waitFor(() =>
      expect(screen.getByRole("status", { name: "MATCH expression" })).toHaveTextContent(
        '"x66663134"*',
      ),
    );
    expect(api.invokeTypedReadCommand).toHaveBeenCalledWith(
      "icu_match_expression",
      { request: { term: "FF14" } },
    );
    expect(copyButton).toBeEnabled();
    fireEvent.click(copyButton);

    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith('"x66663134"*'),
    );
    expect(screen.getByRole("button", { name: "Copied" })).toBeVisible();
  });

  it("renders the ICU dictionary segments returned for Japanese", async () => {
    api.invokeTypedReadCommand.mockResolvedValueOnce(
      '"xe3818b"* AND "xe381bf"* AND "xe38192"* AND "xe383bc"*',
    );
    render(createElement(IcuTokenConverter));

    fireEvent.change(screen.getByRole("textbox", { name: "Search term" }), {
      target: { value: "かみげー" },
    });

    await waitFor(() =>
      expect(screen.getByRole("status", { name: "MATCH expression" })).toHaveTextContent(
        '"xe3818b"* AND "xe381bf"* AND "xe38192"* AND "xe383bc"*',
      ),
    );
  });
});
