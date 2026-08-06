import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  CUSTOM_TIMELINE_QUERY_EXAMPLES,
  CUSTOM_TIMELINE_SCHEMA,
  IcuTokenConverter,
} from "./TimelineSettingsPanel";

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
