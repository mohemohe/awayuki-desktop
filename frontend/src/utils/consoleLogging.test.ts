import { describe, expect, it } from "vitest";
import { redactConsoleMessage } from "./consoleLogging";

describe("redactConsoleMessage", () => {
  it("removes credentials and OAuth query values", () => {
    const output = redactConsoleMessage(
      "access_token=secret code=oauth-code password: hunter2",
    );

    expect(output).not.toContain("secret");
    expect(output).not.toContain("oauth-code");
    expect(output).not.toContain("hunter2");
  });

  it("removes bearer values", () => {
    expect(redactConsoleMessage("Authorization: Bearer abc.def")).toBe(
      "Authorization: Bearer [redacted]",
    );
  });
});
