import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createMockFixture } from "../../api/mock";
import { setAppLocale } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { PluginSnapshot, TimelineStatus } from "../../types/app";
import { ComposeAreaController } from "./ComposeAreaController";

const pluginApi = vi.hoisted(() => ({
  currentPluginSnapshot: vi.fn(),
  loadPluginSnapshot: vi.fn(),
  subscribePluginSnapshot: vi.fn(),
  invokePluginComposeButton: vi.fn(),
}));
const composeMediaQueue = vi.hoisted(() => ({
  attachments: [] as Array<{
    id: string;
    filename: string;
    previewSrc: string;
    uploading?: boolean;
  }>,
  announcement: "",
  uploading: false,
  currentAttachmentState: { ids: [] as string[], uploading: false },
  uploadFiles: vi.fn(),
  handlePaste: vi.fn(),
  remove: vi.fn(),
  move: vi.fn(),
  replaceWithIds: vi.fn(),
  getCurrentAttachmentState: vi.fn(),
  clear: vi.fn(),
}));

vi.mock("../../features/plugins/pluginSnapshot", () => pluginApi);
vi.mock("../../features/compose/useComposeMediaQueue", () => ({
  useComposeMediaQueue: () => composeMediaQueue,
}));

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

function composeStatus(overrides: Partial<TimelineStatus> = {}): TimelineStatus {
  return {
    id: "target-1",
    originalStatusId: "target-1",
    statusIdentity: {
      protocol: "activityPub",
      serverDomain: "example.social",
      canonicalUri: "https://example.social/statuses/target-1",
      remoteId: "target-1",
    },
    accountId: "alice",
    serverDomain: "example.social",
    uri: "https://example.social/statuses/target-1",
    displayName: "Alice",
    acct: "alice@example.social",
    avatar: "",
    createdAt: new Date(0).toISOString(),
    content: "<p>target</p>",
    visibility: "public",
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
    ...overrides,
  };
}

describe("ComposeAreaController", () => {
  beforeEach(() => {
    const emptyPluginSnapshot: PluginSnapshot = {
      directory: "/tmp/awayuki/plugins",
      revision: 1,
      plugins: [],
      composeButtons: [],
    };
    pluginApi.currentPluginSnapshot.mockReset();
    pluginApi.loadPluginSnapshot.mockReset();
    pluginApi.subscribePluginSnapshot.mockReset();
    pluginApi.invokePluginComposeButton.mockReset();
    pluginApi.currentPluginSnapshot.mockReturnValue(null);
    pluginApi.loadPluginSnapshot.mockResolvedValue(emptyPluginSnapshot);
    pluginApi.subscribePluginSnapshot.mockImplementation(() => () => undefined);
    pluginApi.invokePluginComposeButton.mockResolvedValue({});
    composeMediaQueue.attachments = [];
    composeMediaQueue.announcement = "";
    composeMediaQueue.uploading = false;
    composeMediaQueue.currentAttachmentState = { ids: [], uploading: false };
    composeMediaQueue.uploadFiles.mockReset();
    composeMediaQueue.handlePaste.mockReset();
    composeMediaQueue.remove.mockReset();
    composeMediaQueue.move.mockReset();
    composeMediaQueue.replaceWithIds.mockReset();
    composeMediaQueue.getCurrentAttachmentState.mockReset();
    composeMediaQueue.getCurrentAttachmentState.mockImplementation(
      () => composeMediaQueue.currentAttachmentState,
    );
    composeMediaQueue.clear.mockReset();
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

  it("applies a plugin button result to the draft without posting", async () => {
    const pluginSnapshot: PluginSnapshot = {
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "transform",
          generation: 3,
          icon: "✨",
          label: "Transform draft",
        },
      ],
    };
    pluginApi.loadPluginSnapshot.mockResolvedValue(pluginSnapshot);
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      text: "transformed",
      cw_enabled: true,
      cw_title: "ぴえん",
      visibility: "direct",
      sensitive: false,
      media_ids: [],
      poll: {
        options: ["one", "two"],
        multiple: true,
        expires_in: 3600,
      },
      target: null,
    });
    useAppStore.setState({ composeText: "original" });
    const post = useAppStore.getState().post;

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Transform draft" }),
    );

    await waitFor(() =>
      expect(useAppStore.getState().composeText).toBe("transformed"),
    );
    expect(pluginApi.invokePluginComposeButton).toHaveBeenCalledWith({
      pluginId: "draft-tools",
      buttonId: "transform",
      generation: 3,
      compose: expect.objectContaining({
        text: "original",
        cw_enabled: false,
        cw_title: "",
        visibility: "public",
        sensitive: false,
        media_ids: [],
        poll: null,
        target: null,
      }),
    });
    expect(screen.getByDisplayValue("ぴえん")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Direct" })).toBeInTheDocument();
    expect(screen.getByDisplayValue("one")).toBeInTheDocument();
    expect(screen.getByDisplayValue("two")).toBeInTheDocument();
    expect(post).not.toHaveBeenCalled();
  });

  it("keeps the visibility menu outside the scrollable compose icon rail", async () => {
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "layout-tools",
          buttonId: "layout",
          generation: 1,
          icon: "✨",
        },
      ],
    });

    render(<ComposeAreaController />);

    const pluginButton = await screen.findByRole("button", {
      name: "layout-tools ✨",
    });
    const visibilityButton = screen.getByRole("button", { name: "Public" });
    const iconRail = pluginButton.parentElement;
    const toolbar = iconRail?.parentElement;

    expect(iconRail).toHaveClass("overflow-x-auto", "overflow-y-hidden");
    expect(iconRail).not.toContainElement(visibilityButton);
    expect(toolbar).toContainElement(visibilityButton);
    expect(toolbar).not.toHaveClass("overflow-x-auto");

    fireEvent.click(visibilityButton);
    const visibilityMenu = screen.getByRole("menu");
    expect(iconRail).not.toContainElement(visibilityMenu);
    expect(toolbar).toContainElement(visibilityMenu);
  });

  it("enables CW when the exact sample changes only cw_title", async () => {
    const fixture = createMockFixture();
    fixture.settings.presetVisibility = {
      entries: [{ keyword: "sample", visibility: "Unlisted" }],
    };
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "sample.mjs",
          buttonId: "0",
          generation: 1,
          icon: "🥹\u200b",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockImplementation(
      async ({ compose }) => ({
        ...(compose as Record<string, unknown>),
        cw_title: "ぴえん",
      }),
    );
    useAppStore.setState({ snapshot: fixture, composeText: "sample draft" });

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "sample.mjs 🥹\u200b" }),
    );

    expect(await screen.findByDisplayValue("ぴえん")).toBeInTheDocument();
    expect(pluginApi.invokePluginComposeButton).toHaveBeenCalledWith(
      expect.objectContaining({
        compose: expect.objectContaining({
          cw_enabled: false,
          cw_title: "",
        }),
      }),
    );
    fireEvent.change(screen.getByPlaceholderText("What's on your mind?"), {
      target: { value: "ordinary draft" },
    });
    await screen.findByRole("button", { name: "Public" });
    fireEvent.click(screen.getByRole("button", { name: "Post" }));
    await waitFor(() => expect(useAppStore.getState().post).toHaveBeenCalled());
    expect(useAppStore.getState().post).toHaveBeenCalledWith(
      expect.objectContaining({
        sensitive: true,
        spoilerText: "ぴえん",
        visibility: undefined,
      }),
    );
  });

  it("applies edit target visibility to the locked compose control", async () => {
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "edit-target",
          generation: 16,
          icon: "✏️",
          label: "Set edit target",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      text: "edited by plugin",
      visibility: "public",
      target: {
        kind: "edit",
        status: composeStatus({ visibility: "private" }),
      },
    });
    useAppStore.setState({ composeText: "ordinary draft", visibility: "public" });

    render(<ComposeAreaController />);
    fireEvent.click(await screen.findByRole("button", { name: "Set edit target" }));

    const visibility = await screen.findByRole("button", { name: "Private" });
    expect(visibility).toBeDisabled();
    expect(useAppStore.getState().visibility).toBe("private");
    expect(useAppStore.getState().composeTarget?.kind).toBe("edit");
    fireEvent.click(screen.getByRole("button", { name: "Edit post" }));
    await waitFor(() => expect(useAppStore.getState().post).toHaveBeenCalled());
    expect(useAppStore.getState().post).toHaveBeenCalledWith(
      expect.objectContaining({ visibility: undefined }),
    );
  });

  it("preserves the original visibility across plugin reply target changes", async () => {
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "reply-target",
          generation: 17,
          icon: "↩️",
          label: "Set reply target",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      visibility: "direct",
      target: {
        kind: "reply",
        status: composeStatus({ visibility: "unlisted" }),
      },
    });
    useAppStore.setState({ composeText: "reply draft", visibility: "private" });
    useAppStore.getState().replyStatus(
      composeStatus({
        id: "target-a",
        originalStatusId: "target-a",
        visibility: "direct",
        statusIdentity: {
          protocol: "activityPub",
          serverDomain: "example.social",
          canonicalUri: "https://example.social/statuses/target-a",
          remoteId: "target-a",
        },
      }),
    );

    render(<ComposeAreaController />);
    fireEvent.click(await screen.findByRole("button", { name: "Set reply target" }));

    await screen.findByRole("button", { name: "Unlisted" });
    expect(useAppStore.getState().visibility).toBe("unlisted");
    expect(useAppStore.getState().composeTarget).toMatchObject({
      kind: "reply",
      visibilityBeforeReply: "private",
    });
    fireEvent.click(screen.getByTitle("Clear target post"));
    await screen.findByRole("button", { name: "Private" });
  });

  it("does not invoke plugin buttons while an attachment is uploading", async () => {
    composeMediaQueue.uploading = true;
    composeMediaQueue.currentAttachmentState = {
      ids: ["pending-media"],
      uploading: true,
    };
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "media-transform",
          generation: 14,
          icon: "🖼️",
          label: "Transform media draft",
        },
      ],
    });

    render(<ComposeAreaController />);
    const button = await screen.findByRole("button", {
      name: "Transform media draft",
    });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(pluginApi.invokePluginComposeButton).not.toHaveBeenCalled();
  });

  it("rejects a plugin result when attachment ids change while it runs", async () => {
    let resolveInvocation: ((value: unknown) => void) | undefined;
    composeMediaQueue.attachments = [
      {
        id: "media-1",
        filename: "one.png",
        previewSrc: "https://example.social/one.png",
      },
    ];
    composeMediaQueue.currentAttachmentState = {
      ids: ["media-1"],
      uploading: false,
    };
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "slow-media-transform",
          generation: 15,
          icon: "🖼️",
          label: "Slow media transform",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockReturnValue(
      new Promise((resolve) => {
        resolveInvocation = resolve;
      }),
    );
    useAppStore.setState({ composeText: "safe media draft" });

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Slow media transform" }),
    );
    await waitFor(() =>
      expect(pluginApi.invokePluginComposeButton).toHaveBeenCalled(),
    );
    composeMediaQueue.currentAttachmentState = {
      ids: ["media-1", "media-2"],
      uploading: false,
    };
    act(() =>
      resolveInvocation?.({
        text: "stale media draft",
        media_ids: ["media-1"],
      }),
    );

    await waitFor(() =>
      expect(useAppStore.getState().error).toContain(
        "Compose attachments changed while the plugin button was running",
      ),
    );
    expect(useAppStore.getState().composeText).toBe("safe media draft");
    expect(composeMediaQueue.replaceWithIds).not.toHaveBeenCalled();
  });

  it("rejects a plugin result after the user edits the draft while it runs", async () => {
    let resolveInvocation: ((value: unknown) => void) | undefined;
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "slow-text-transform",
          generation: 18,
          icon: "🐢",
          label: "Slow text transform",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockReturnValue(
      new Promise((resolve) => {
        resolveInvocation = resolve;
      }),
    );
    useAppStore.setState({ composeText: "captured draft" });

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Slow text transform" }),
    );
    fireEvent.change(screen.getByPlaceholderText("What's on your mind?"), {
      target: { value: "new user edit" },
    });
    act(() => resolveInvocation?.({ text: "stale captured draft" }));

    await waitFor(() =>
      expect(useAppStore.getState().error).toContain(
        "Compose draft changed while the plugin button was running",
      ),
    );
    expect(useAppStore.getState().composeText).toBe("new user edit");
  });

  it("submits plugin visibility ahead of a matching preset", async () => {
    const fixture = createMockFixture();
    fixture.settings.presetVisibility = {
      entries: [{ keyword: "trigger", visibility: "Public" }],
    };
    useAppStore.setState({
      snapshot: fixture,
      composeText: "trigger",
    });
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "private",
          generation: 4,
          icon: "🔒",
          label: "Make direct",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      visibility: "direct",
      sensitive: true,
    });

    render(<ComposeAreaController />);
    fireEvent.click(await screen.findByRole("button", { name: "Make direct" }));
    await screen.findByRole("button", { name: "Direct" });
    fireEvent.click(screen.getByRole("button", { name: "Post" }));

    await waitFor(() => expect(useAppStore.getState().post).toHaveBeenCalled());
    expect(useAppStore.getState().post).toHaveBeenCalledWith(
      expect.objectContaining({ visibility: "direct" }),
    );
  });

  it("clears plugin visibility when visibility is selected manually", async () => {
    useAppStore.setState({ composeText: "draft" });
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "direct",
          generation: 5,
          icon: "✉️",
          label: "Make direct",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      visibility: "direct",
    });

    render(<ComposeAreaController />);
    fireEvent.click(await screen.findByRole("button", { name: "Make direct" }));
    fireEvent.click(await screen.findByRole("button", { name: "Direct" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Private" }));
    expect(screen.getByRole("button", { name: "Private" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Post" }));
    await waitFor(() => expect(useAppStore.getState().post).toHaveBeenCalled());
    expect(useAppStore.getState().post).toHaveBeenCalledWith(
      expect.objectContaining({ visibility: undefined, sensitive: false }),
    );
  });

  it("clears plugin visibility when a later reply target changes the draft", async () => {
    useAppStore.setState({ composeText: "draft" });
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "direct",
          generation: 6,
          icon: "✉️",
          label: "Make direct",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      visibility: "direct",
      sensitive: true,
    });

    render(<ComposeAreaController />);
    fireEvent.click(await screen.findByRole("button", { name: "Make direct" }));
    await screen.findByRole("button", { name: "Direct" });
    act(() => {
      useAppStore.getState().replyStatus(
        composeStatus({ visibility: "unlisted" }),
      );
    });

    await screen.findByRole("button", { name: "Unlisted" });
    fireEvent.click(screen.getByRole("button", { name: "Post" }));
    await waitFor(() => expect(useAppStore.getState().post).toHaveBeenCalled());
    expect(useAppStore.getState().post).toHaveBeenCalledWith(
      expect.objectContaining({ visibility: undefined, sensitive: false }),
    );
  });

  it("ignores a compose result after its button generation is unloaded", async () => {
    let listener: ((snapshot: PluginSnapshot) => void) | undefined;
    let resolveInvocation: ((value: unknown) => void) | undefined;
    const loaded: PluginSnapshot = {
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "slow",
          generation: 8,
          icon: "🐢",
          label: "Slow transform",
        },
      ],
    };
    pluginApi.loadPluginSnapshot.mockResolvedValue(loaded);
    pluginApi.subscribePluginSnapshot.mockImplementation((next) => {
      listener = next;
      return () => undefined;
    });
    pluginApi.invokePluginComposeButton.mockReturnValue(
      new Promise((resolve) => {
        resolveInvocation = resolve;
      }),
    );
    useAppStore.setState({ composeText: "keep me" });

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Slow transform" }),
    );
    await waitFor(() =>
      expect(pluginApi.invokePluginComposeButton).toHaveBeenCalled(),
    );
    act(() => {
      listener?.({ ...loaded, revision: 3, composeButtons: [] });
    });
    act(() => resolveInvocation?.({ text: "stale result" }));

    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Slow transform" }),
      ).not.toBeInTheDocument(),
    );
    expect(useAppStore.getState().composeText).toBe("keep me");
  });

  it("ignores a compose result that returns after the active account changes", async () => {
    let resolveInvocation: ((value: unknown) => void) | undefined;
    const fixture = createMockFixture();
    const firstAccount = fixture.accounts[0];
    const secondAccount = {
      ...firstAccount,
      acct: "second@example.social",
      accountId: "second-account",
      isActive: false,
    };
    fixture.accounts = [firstAccount, secondAccount];
    fixture.activeAcct = firstAccount.acct;
    useAppStore.setState({
      snapshot: fixture,
      composeText: "first account draft",
    });
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "slow-account-transform",
          generation: 10,
          icon: "🐢",
          label: "Slow account transform",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockReturnValue(
      new Promise((resolve) => {
        resolveInvocation = resolve;
      }),
    );

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Slow account transform" }),
    );
    await waitFor(() =>
      expect(pluginApi.invokePluginComposeButton).toHaveBeenCalled(),
    );
    act(() => {
      useAppStore.setState((state) => ({
        snapshot: state.snapshot
          ? {
              ...state.snapshot,
              activeAcct: secondAccount.acct,
              accounts: state.snapshot.accounts.map((account) => ({
                ...account,
                isActive: account.acct === secondAccount.acct,
              })),
            }
          : state.snapshot,
      }));
      resolveInvocation?.({
        text: "stale first-account result",
        cw_enabled: true,
        cw_title: "stale CW",
        poll: {
          options: ["stale option one", "stale option two"],
          multiple: false,
          expires_in: 3600,
        },
      });
    });
    await waitFor(() =>
      expect(useAppStore.getState().composeText).toBe(""),
    );
    expect(screen.queryByDisplayValue("stale CW")).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue("stale option one")).not.toBeInTheDocument();
  });

  it("ignores a compose result that returns after a successful submit", async () => {
    let resolveInvocation: ((value: unknown) => void) | undefined;
    useAppStore.setState({ composeText: "draft being posted" });
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "slow-submit-transform",
          generation: 11,
          icon: "🐢",
          label: "Slow submit transform",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockReturnValue(
      new Promise((resolve) => {
        resolveInvocation = resolve;
      }),
    );

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Slow submit transform" }),
    );
    await waitFor(() =>
      expect(pluginApi.invokePluginComposeButton).toHaveBeenCalled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Post" }));
    await waitFor(() =>
      expect(useAppStore.getState().composeText).toBe(""),
    );

    act(() => resolveInvocation?.({ text: "stale submitted result" }));

    await waitFor(() =>
      expect(useAppStore.getState().composeText).toBe(""),
    );
  });

  it("ignores a compose result after an external target replaces its draft", async () => {
    let resolveInvocation: ((value: unknown) => void) | undefined;
    useAppStore.setState({ composeText: "draft before reply" });
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "slow-target-transform",
          generation: 12,
          icon: "🐢",
          label: "Slow target transform",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockReturnValue(
      new Promise((resolve) => {
        resolveInvocation = resolve;
      }),
    );

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Slow target transform" }),
    );
    await waitFor(() =>
      expect(pluginApi.invokePluginComposeButton).toHaveBeenCalled(),
    );
    act(() => {
      useAppStore.getState().replyStatus(
        composeStatus({ visibility: "unlisted" }),
      );
    });
    await screen.findByText("Reply");

    act(() => resolveInvocation?.({ text: "stale pre-reply result" }));

    await waitFor(() =>
      expect(useAppStore.getState().composeText).not.toBe(
        "stale pre-reply result",
      ),
    );
    expect(useAppStore.getState().composeTarget?.kind).toBe("reply");
  });

  it("rejects an invalid plugin result without partially applying it", async () => {
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "invalid",
          generation: 9,
          icon: "⚠️",
          label: "Invalid transform",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      text: "must not apply",
      cw_title: 42,
    });
    useAppStore.setState({ composeText: "safe draft" });

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Invalid transform" }),
    );

    await waitFor(() =>
      expect(useAppStore.getState().error).toContain(
        "Plugin compose field cw_title must be a string",
      ),
    );
    expect(useAppStore.getState().composeText).toBe("safe draft");
  });

  it("rejects a malformed plugin target without partially applying text", async () => {
    pluginApi.loadPluginSnapshot.mockResolvedValue({
      directory: "/tmp/awayuki/plugins",
      revision: 2,
      plugins: [],
      composeButtons: [
        {
          pluginId: "draft-tools",
          buttonId: "malformed-target",
          generation: 13,
          icon: "⚠️",
          label: "Malformed target",
        },
      ],
    });
    pluginApi.invokePluginComposeButton.mockResolvedValue({
      text: "must not apply",
      target: {
        kind: "reply",
        status: {
          id: "incomplete",
          originalStatusId: "incomplete",
          acct: "alice@example.social",
          content: "<p>incomplete</p>",
          visibility: "public",
          statusIdentity: {
            canonicalUri: "https://example.social/statuses/incomplete",
          },
        },
      },
    });
    useAppStore.setState({ composeText: "safe target draft" });

    render(<ComposeAreaController />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Malformed target" }),
    );

    await waitFor(() =>
      expect(useAppStore.getState().error).toContain(
        "Plugin compose field target is invalid",
      ),
    );
    expect(useAppStore.getState().composeText).toBe("safe target draft");
    expect(useAppStore.getState().composeTarget).toBeNull();
  });
});
