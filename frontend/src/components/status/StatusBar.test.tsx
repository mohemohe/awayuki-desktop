import React from "react";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import { StatusBar } from "./StatusBar";

describe("StatusBar", () => {
  const loadStatusBar = vi.fn(async () => undefined);

  beforeEach(() => {
    loadStatusBar.mockClear();
    useAppStore.setState({
      statusMessage: "ブックマークに追加しています",
      loadStatusBar,
      statusBar: undefined,
      composeOutboxItems: [],
    });
  });

  it("renders status mutation feedback in its live status region", () => {
    render(<StatusBar />);

    expect(screen.getByRole("status")).toHaveTextContent(
      "ブックマークに追加しています",
    );
  });
});
