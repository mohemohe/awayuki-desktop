import React from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot, MediaPreviewState } from "../../types/app";

const mocks = vi.hoisted(() => {
  const visibilityOperations: string[] = [];
  const createWebview = (id: string) => ({
    show: vi.fn(async () => {
      visibilityOperations.push(`show:${id}`);
    }),
    hide: vi.fn(async () => {
      visibilityOperations.push(`hide:${id}`);
    }),
    setPosition: vi.fn(async () => undefined),
    setSize: vi.fn(async () => undefined),
  });
  const webviews = {
    "sidecar-social": createWebview("social"),
    "sidecar-news": createWebview("news"),
  };
  return {
    webview: webviews["sidecar-social"],
    webviews,
    visibilityOperations,
    getByLabel: vi.fn(
      async (label: keyof typeof webviews) => webviews[label] ?? null,
    ),
    invokeCommand: vi.fn(async () => undefined),
  };
});

vi.mock("@tauri-apps/api/webview", () => ({
  Webview: { getByLabel: mocks.getByLabel },
}));

vi.mock("@tauri-apps/api/dpi", () => ({
  LogicalPosition: class LogicalPosition {
    constructor(
      public x: number,
      public y: number,
    ) {}
  },
  LogicalSize: class LogicalSize {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({
  hasTauriRuntime: () => true,
  invokeCommand: mocks.invokeCommand,
  invokeTypedCommand: mocks.invokeCommand,
  invokeTypedCommandWithOperationId: mocks.invokeCommand,
  invokeTypedReadCommand: mocks.invokeCommand,
  invokeTypedReadCommandWithOperationId: mocks.invokeCommand,
}));

vi.mock("../../utils/browser", () => ({
  getClientPlatform: () => "linux",
}));

vi.mock("../compose/ComposeArea", () => ({ ComposeArea: () => null }));
vi.mock("../status/StatusBar", () => ({ StatusBar: () => null }));
vi.mock("../timeline/TimelineArea", () => ({ TimelineArea: () => null }));

import { useAppStore } from "../../store/appStore";
import { WorkspaceView } from "./WorkspaceView";

const snapshot = {
  version: "test",
  accounts: [],
  activeAcct: null,
  columns: [],
  settings: {
    sidecars: {
      entries: [
        {
          id: "social",
          name: "Social",
          url: "https://example.test/",
          userStyleEnabled: false,
          userStyle: "",
          width: 320,
        },
      ],
      mainViewIndex: 0,
    },
  },
  database: {},
} as unknown as AppSnapshot;

describe("Sidecar native WebView lifecycle", () => {
  beforeEach(() => {
    for (const webview of Object.values(mocks.webviews)) {
      webview.show.mockClear();
      webview.hide.mockClear();
      webview.setPosition.mockClear();
      webview.setSize.mockClear();
    }
    mocks.getByLabel.mockClear();
    mocks.invokeCommand.mockClear();
    mocks.visibilityOperations.length = 0;

    vi.stubGlobal(
      "ResizeObserver",
      class ResizeObserver {
        observe() {}
        disconnect() {}
      },
    );
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 800,
      y: 100,
      left: 800,
      top: 100,
      right: 1120,
      bottom: 700,
      width: 320,
      height: 600,
      toJSON: () => ({}),
    });
    useAppStore.setState({
      snapshot,
      activeTabs: {},
      dynamicColumns: [],
      mediaPreview: null,
    });
  });

  it("hides native sidecar webviews while previewing and restores them after close", async () => {
    render(<WorkspaceView />);

    await waitFor(() => expect(mocks.webview.show).toHaveBeenCalledTimes(1));

    await act(async () => {
      useAppStore.setState({ mediaPreview: {} as MediaPreviewState });
    });
    await waitFor(() => expect(mocks.webview.hide).toHaveBeenCalledTimes(1));

    await act(async () => {
      useAppStore.setState({ mediaPreview: null });
    });
    await waitFor(() => expect(mocks.webview.show).toHaveBeenCalledTimes(2));
  });

  it("closes the native webview and removes the region when sidecars become empty", async () => {
    render(<WorkspaceView />);

    await waitFor(() => expect(mocks.webview.show).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("region", { name: "Sidecar" })).toBeVisible();

    await act(async () => {
      useAppStore.setState({
        snapshot: {
          ...snapshot,
          settings: {
            ...snapshot.settings,
            sidecars: { entries: [], mainViewIndex: 0 },
          },
        },
      });
    });

    expect(
      screen.queryByRole("region", { name: "Sidecar" }),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(mocks.invokeCommand).toHaveBeenCalledWith(
        "close_sidecar_webview",
        { sidecarId: "social" },
      ),
    );
  });

  it("switches sidecar tabs by hiding and showing the existing webviews", async () => {
    useAppStore.setState({
      snapshot: {
        ...snapshot,
        settings: {
          ...snapshot.settings,
          sidecars: {
            entries: [
              ...snapshot.settings.sidecars.entries,
              {
                id: "news",
                name: "News",
                url: "https://news.example.test/",
                userStyleEnabled: false,
                userStyle: "",
                width: 420,
              },
            ],
            mainViewIndex: 0,
          },
        },
      },
    });

    render(<WorkspaceView />);

    await waitFor(() =>
      expect(mocks.webviews["sidecar-social"].show).toHaveBeenCalledTimes(1),
    );
    expect(mocks.webviews["sidecar-news"].show).not.toHaveBeenCalled();
    mocks.visibilityOperations.length = 0;

    fireEvent.click(screen.getByRole("tab", { name: "News" }));

    await waitFor(() => {
      expect(mocks.webviews["sidecar-social"].hide).toHaveBeenCalledTimes(1);
      expect(mocks.webviews["sidecar-news"].show).toHaveBeenCalledTimes(1);
    });
    expect(mocks.visibilityOperations).toEqual(["hide:social", "show:news"]);
    expect(screen.getByRole("tab", { name: "News" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    mocks.visibilityOperations.length = 0;

    fireEvent.click(screen.getByRole("tab", { name: "Social" }));

    await waitFor(() => {
      expect(mocks.webviews["sidecar-news"].hide).toHaveBeenCalledTimes(1);
      expect(mocks.webviews["sidecar-social"].show).toHaveBeenCalledTimes(2);
    });
    expect(mocks.visibilityOperations).toEqual(["hide:news", "show:social"]);
  });
});
