import React from "react";
import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot, MediaPreviewState } from "../../types/app";

const mocks = vi.hoisted(() => {
  const webview = {
    show: vi.fn(async () => undefined),
    hide: vi.fn(async () => undefined),
    setPosition: vi.fn(async () => undefined),
    setSize: vi.fn(async () => undefined),
  };
  return {
    webview,
    getByLabel: vi.fn(async () => webview),
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

describe("Sidecar media preview visibility", () => {
  beforeEach(() => {
    mocks.webview.show.mockClear();
    mocks.webview.hide.mockClear();
    mocks.webview.setPosition.mockClear();
    mocks.webview.setSize.mockClear();
    mocks.getByLabel.mockClear();
    mocks.invokeCommand.mockClear();

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
});
