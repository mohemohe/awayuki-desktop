import { describe, expect, it } from "vitest";

import {
  IpcAppError,
  normalizeIpcError,
  publicErrorMessage,
} from "./ipcErrors";

const requestId = "11111111-1111-4111-8111-111111111111";

describe("IPC error envelope", () => {
  it("preserves stable codes without exposing backend causes", () => {
    const error = normalizeIpcError(
      {
        code: "rate_limited",
        messageKey: "errors.rate_limited",
        safeDetails: {
          retryAfterSeconds: "30",
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
    expect(error.safeDetails).toEqual({ retryAfterSeconds: "30" });
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
