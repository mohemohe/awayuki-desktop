import { describe, expect, it, vi } from "vitest";
import { installUnhandledErrorLogging, redactConsoleMessage } from "./consoleLogging";

describe("unhandled UI failures", () => {
  it("records JavaScript/async failures outside React without swallowing them", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    // Keep intentional failures off Vitest's own global error listener.
    const target = new EventTarget() as Window;
    const uninstall = installUnhandledErrorLogging(target);
    const failure = new Error("event handler failed");
    const event = new ErrorEvent("error", { error: failure, message: failure.message, cancelable: true });
    target.dispatchEvent(event);
    expect(error).toHaveBeenCalledWith("[awayuki][ui] uncaught JavaScript error", failure);
    expect(event.defaultPrevented).toBe(false);

    const rejection = new Event("unhandledrejection", { cancelable: true });
    Object.defineProperty(rejection, "reason", { value: failure });
    target.dispatchEvent(rejection);
    expect(error).toHaveBeenCalledWith("[awayuki][ui] unhandled promise rejection", failure);
    expect(rejection.defaultPrevented).toBe(false);

    error.mockClear();
    target.dispatchEvent(new Event("error"));
    expect(error).not.toHaveBeenCalled();
    uninstall();
    target.dispatchEvent(event);
    target.dispatchEvent(rejection);
    expect(error).not.toHaveBeenCalled();
  });
});

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

  it("removes content fields and local paths", () => {
    const output = redactConsoleMessage(
      'content="private post" notification_body=private-notice /Users/alice/Awayuki/awayuki.db C:\\Users\\alice\\awayuki.db',
    );
    expect(output).not.toContain("private post");
    expect(output).not.toContain("private-notice");
    expect(output).not.toContain("alice");
    expect(output.match(/\[local-path\]/g)).toHaveLength(2);
  });
});
