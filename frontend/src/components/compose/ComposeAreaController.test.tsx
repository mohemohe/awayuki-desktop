import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createMockFixture } from "../../api/mock";
import { setAppLocale } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { TimelineStatus } from "../../types/app";
import { ComposeAreaController } from "./ComposeAreaController";

vi.mock("../../features/compose/ComposeEmojiPicker", () => ({
  ComposeEmojiPicker: ({
    onPickEmoji,
  }: {
    onPickEmoji: (emoji: string) => void;
  }) => (
    <>
      <button onClick={() => onPickEmoji(":second:")}>Pick custom emoji</button>
      <button onClick={() => onPickEmoji("😀")}>Pick Unicode emoji</button>
    </>
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

  it("locks visibility to the original value while editing", () => {
    useAppStore.getState().beginEditStatus({
      id: "edit-1",
      originalStatusId: "edit-1",
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "example.social",
        canonicalUri: "https://example.social/statuses/edit-1",
        remoteId: "edit-1",
      },
      accountId: "alice",
      serverDomain: "example.social",
      uri: "https://example.social/statuses/edit-1",
      displayName: "Alice",
      acct: "alice@example.social",
      avatar: "",
      createdAt: new Date(0).toISOString(),
      content: "<p>notification</p>",
      visibility: "private",
      spoilerText: "",
      reblogsCount: 0,
      favouritesCount: 0,
      repliesCount: 0,
      sensitive: false,
      favourited: false,
      reblogged: false,
      bookmarked: false,
      media: [],
      emojis: [],
      accountEmojis: [],
    } satisfies TimelineStatus);

    render(<ComposeAreaController />);

    const visibility = screen.getByRole("button", { name: "Private" });
    expect(visibility).toBeDisabled();
    expect(visibility).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(visibility);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("inserts a no-width space after a custom emoji from the picker", () => {
    useAppStore.setState({ composeText: ":first:" });
    render(<ComposeAreaController />);

    const textarea = screen.getByPlaceholderText<HTMLTextAreaElement>(
      "What's on your mind?",
    );
    textarea.setSelectionRange(textarea.value.length, textarea.value.length);
    fireEvent.click(screen.getByTitle("Emoji"));
    fireEvent.click(screen.getByRole("button", { name: "Pick custom emoji" }));

    expect(useAppStore.getState().composeText).toBe(
      ":first:\u200B:second:\u200B",
    );
  });

  it("inserts a no-width space after a Unicode emoji from the picker", () => {
    render(<ComposeAreaController />);

    fireEvent.click(screen.getByTitle("Emoji"));
    fireEvent.click(screen.getByRole("button", { name: "Pick Unicode emoji" }));

    expect(useAppStore.getState().composeText).toBe("😀\u200B");
  });

  it("inserts a no-width space after a shortcode suggestion", async () => {
    render(<ComposeAreaController />);

    const textarea = screen.getByPlaceholderText("What's on your mind?");
    fireEvent.change(textarea, { target: { value: ":away" } });

    const suggestion = await screen.findByTitle(":awayuki:");
    fireEvent.mouseDown(suggestion.closest('[role="option"]')!);

    expect(useAppStore.getState().composeText).toBe(":awayuki:\u200B");
  });
});
