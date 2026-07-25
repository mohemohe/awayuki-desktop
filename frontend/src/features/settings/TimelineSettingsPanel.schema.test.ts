import { describe, expect, it } from "vitest";
import {
  CUSTOM_TIMELINE_QUERY_EXAMPLES,
  CUSTOM_TIMELINE_SCHEMA,
} from "./TimelineSettingsPanel";

describe("custom timeline schema reference", () => {
  it("documents the readable ICU full-text search tables", () => {
    const schema = new Map(
      CUSTOM_TIMELINE_SCHEMA.map((section) => [
        section.label,
        [...section.values],
      ]),
    );

    expect(schema.get("status_search_icu_content")).toEqual([
      "docid",
      "status_id",
      "server_domain",
      "token_text",
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

  it("provides executable status and account FTS query examples", () => {
    const examples = new Map(
      CUSTOM_TIMELINE_QUERY_EXAMPLES.map((example) => [
        example.label,
        example.sql,
      ]),
    );

    expect(examples.get("Latest statuses")).toContain("FROM statuses");
    expect(examples.get("Status full-text search")).toContain(
      "status_search_icu_fts MATCH",
    );
    expect(examples.get("Status full-text search")).toContain(
      "JOIN status_search_icu_content",
    );
    expect(examples.get("Account full-text search")).toContain(
      "account_search_icu_fts MATCH",
    );
    expect(examples.get("Account full-text search")).toContain(
      "JOIN account_search_icu_content",
    );
  });
});
