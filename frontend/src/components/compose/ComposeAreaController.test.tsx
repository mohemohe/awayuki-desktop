import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createMockFixture } from "../../api/mock";
import { setAppLocale } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import { ComposeAreaController } from "./ComposeAreaController";

vi.mock("../../features/compose/ComposeEmojiPicker", () => ({
  ComposeEmojiPicker: ({
    onPickEmoji,
  }: {
    onPickEmoji: (emoji: string) => void;
  }) => (
    <button onClick={() => onPickEmoji(":second:")}>Pick custom emoji</button>
  ),
}));

describe("ComposeAreaController", () => {
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

  it("grows the compose area as poll options are added", () => {
    const { container } = render(<ComposeAreaController />);
    const composeArea = container.querySelector("section")!;

    expect(composeArea).toHaveStyle({ height: "112px" });

    fireEvent.click(screen.getByTitle("Poll"));
    expect(composeArea).toHaveStyle({ height: "224px" });

    fireEvent.click(screen.getByRole("button", { name: "Add option" }));
    expect(composeArea).toHaveStyle({ height: "256px" });

    fireEvent.click(screen.getByRole("button", { name: "Add option" }));
    expect(composeArea).toHaveStyle({ height: "288px" });

    fireEvent.click(screen.getAllByTitle("Remove option")[0]);
    expect(composeArea).toHaveStyle({ height: "256px" });
  });

  it("inserts a no-width space between consecutive custom emojis", () => {
    useAppStore.setState({ composeText: ":first:" });
    render(<ComposeAreaController />);

    const textarea = screen.getByPlaceholderText<HTMLTextAreaElement>(
      "What's on your mind?",
    );
    textarea.setSelectionRange(textarea.value.length, textarea.value.length);
    fireEvent.click(screen.getByTitle("Emoji"));
    fireEvent.click(screen.getByRole("button", { name: "Pick custom emoji" }));

    expect(useAppStore.getState().composeText).toBe(
      ":first:\u200B:second:",
    );
  });
});
