import { afterEach, describe, expect, it } from "vitest";

import {
  IpcAppError,
  normalizeIpcError,
  publicErrorMessage,
} from "./ipcErrors";
import { getAppLocale, setAppLocale } from "../i18n";

const requestId = "11111111-1111-4111-8111-111111111111";
const initialLocale = getAppLocale();

describe("IPC error envelope", () => {
  afterEach(() => setAppLocale(initialLocale));

  it("preserves stable codes without exposing backend causes", () => {
    const error = normalizeIpcError(
      {
        code: "rate_limited",
        messageKey: "errors.rate_limited",
        safeDetails: {
          retryAfterSeconds: "30",
          line: "3",
          column: "7",
          token: "secret",
        },
        retryable: true,
        requestId,
        cause: "Authorization: Bearer secret",
      },
      requestId,
    );

    expect(error).toBeInstanceOf(IpcAppError);
    expect(error.code).toBe("rate_limited");
    expect(error.safeDetails).toEqual({
      retryAfterSeconds: "30",
      line: "3",
      column: "7",
    });
    expect(String(error)).not.toContain("secret");
  });

  it("turns arbitrary legacy errors into a safe internal error", () => {
    const error = normalizeIpcError(
      new Error("SQL failed at /Users/alice/app.db token=hunter2"),
      requestId,
    );
    expect(error.code).toBe("internal");
    expect(error.requestId).toBe(requestId);
    expect(String(error)).not.toContain("alice");
    expect(String(error)).not.toContain("hunter2");
  });

  it("uses an operation-specific fallback without exposing the error class", () => {
    setAppLocale("en");
    const error = normalizeIpcError(
      new Error("database is corrupt at /Users/alice/awayuki.db"),
      requestId,
      "load_timeline",
    );

    expect(String(error)).toBe(
      "The timeline could not be loaded. Please try again.",
    );
    expect(String(error)).not.toContain("IpcAppError");
  });

  it("renders the reviewed FTS guidance in Japanese", () => {
    setAppLocale("ja");
    const error = normalizeIpcError(
      {
        code: "validation",
        messageKey: "errors.custom_timeline_fts_match_or",
        retryable: false,
        requestId,
      },
      requestId,
      "load_timeline",
    );

    expect(String(error)).toBe(
      "FTSの検索条件が正しくありません。複数の候補は、1つの MATCH 式の中で OR を使って結合してください。",
    );
  });

  it("rejects unreviewed message keys", () => {
    setAppLocale("en");
    const error = normalizeIpcError(
      {
        code: "internal",
        messageKey: "errors.show_raw_backend_message",
        retryable: false,
        requestId,
      },
      requestId,
      "load_timeline",
    );

    expect(String(error)).toBe(
      "The timeline could not be loaded. Please try again.",
    );
  });

  it("renders reviewed KQ errors and keeps only safe source details", () => {
    setAppLocale("ja");
    const error = normalizeIpcError(
      {
        code: "validation",
        messageKey: "errors.kq_invalid_query",
        safeDetails: {
          line: "2",
          column: "14",
          token: '"private query"',
        },
        retryable: false,
        requestId,
      },
      requestId,
      "load_timeline",
    );

    expect(String(error)).toBe(
      "KQクエリが正しくありません。クエリを確認して、もう一度お試しください。",
    );
    expect(error.safeDetails).toEqual({ line: "2", column: "14" });
  });

  it("represents cooperative cancellation as a safe non-retryable result", () => {
    const error = normalizeIpcError(
      {
        code: "cancelled",
        messageKey: "errors.cancelled",
        retryable: false,
        requestId,
      },
      requestId,
    );
    expect(error.code).toBe("cancelled");
    expect(error.retryable).toBe(false);
    expect(String(error)).not.toContain(requestId);
  });

  it("never renders an unclassified browser or library error verbatim", () => {
    const message = publicErrorMessage(
      new Error(
        "SQL failed at /Users/alice/Awayuki/awayuki.db?token=secret-token",
      ),
    );

    expect(message.length).toBeGreaterThan(0);
    expect(message).not.toContain("/Users/alice");
    expect(message).not.toContain("secret-token");
    expect(message).not.toContain("SQL");
  });
});
