import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createMockFixture } from "../../api/mock";
import { setAppLocale } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import { ComposeAreaController } from "./ComposeAreaController";

describe("ComposeAreaController live commentary mode", () => {
  beforeEach(() => {
    setAppLocale("en");
    useAppStore.setState({
      snapshot: createMockFixture(),
      composeText: "",
      composeTarget: null,
      visibility: "public",
      mutationStates: {},
      post: vi.fn(async () => {
        useAppStore.setState({ composeText: "", composeTarget: null });
        return true;
      }),
    });
  });

  it("marks the toggle active and restores hashtags after a successful post", async () => {
    render(<ComposeAreaController />);

    const toggle = screen.getByTitle("Live commentary mode");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(toggle).toHaveClass("text-blue");

    fireEvent.change(screen.getByPlaceholderText("What's on your mind?"), {
      target: { value: "Live update #foo and #bar" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Post" }));

    await waitFor(() =>
      expect(useAppStore.getState().composeText).toBe("#foo #bar"),
    );
  });

  it("leaves the draft cleared when live commentary mode is disabled", async () => {
    render(<ComposeAreaController />);

    expect(screen.getByTitle("Live commentary mode")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    fireEvent.change(screen.getByPlaceholderText("What's on your mind?"), {
      target: { value: "One-off update #foo" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Post" }));

    await waitFor(() => expect(useAppStore.getState().composeText).toBe(""));
  });
});
