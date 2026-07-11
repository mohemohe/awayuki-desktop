import React from "react";
import { render, screen } from "@testing-library/react";
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
});
