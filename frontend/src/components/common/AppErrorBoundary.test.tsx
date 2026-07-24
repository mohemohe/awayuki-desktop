import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppErrorBoundary } from "./AppErrorBoundary";

function BrokenView(): React.ReactNode {
  throw new Error("render failed");
}

describe("AppErrorBoundary", () => {
  it("replaces an uncaught render failure with a recovery screen", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const suppressExpectedError = (event: ErrorEvent) => {
      if (event.error instanceof Error && event.error.message === "render failed") {
        event.preventDefault();
      }
    };
    window.addEventListener("error", suppressExpectedError);

    render(
      <AppErrorBoundary>
        <BrokenView />
      </AppErrorBoundary>,
    );
    window.removeEventListener("error", suppressExpectedError);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Awayuki encountered an unexpected UI error",
    );
    expect(screen.getByRole("button", { name: "Reload" })).toBeEnabled();
  });

  it("copies explicit diagnostics without exposing them on the recovery screen", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const suppressExpectedError = (event: ErrorEvent) => {
      if (event.error instanceof Error && event.error.message === "render failed") {
        event.preventDefault();
      }
    };
    window.addEventListener("error", suppressExpectedError);
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(
      <AppErrorBoundary>
        <BrokenView />
      </AppErrorBoundary>,
    );
    window.removeEventListener("error", suppressExpectedError);

    expect(screen.queryByText("render failed")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText.mock.calls[0][0]).toContain("render failed");
    expect(screen.getByRole("button", { name: "Copied" })).toBeEnabled();
  });
});
