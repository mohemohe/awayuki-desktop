import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PluginSettingsPanel } from "./PluginSettingsPanel";

const api = vi.hoisted(() => ({
  invokeTypedCommand: vi.fn(),
  invokeTypedReadCommand: vi.fn(),
}));

const plugins = vi.hoisted(() => ({
  loadPluginSnapshot: vi.fn(),
  publishPluginSnapshot: vi.fn(),
  subscribePluginSnapshot: vi.fn(() => () => undefined),
}));

vi.mock("../../api/tauri", () => api);
vi.mock("../plugins/pluginSnapshot", () => plugins);

const snapshot = {
  directory: "/tmp/awayuki/plugins",
  revision: 3,
  composeButtons: [],
  plugins: [
    {
      id: "sample.mjs",
      fileName: "sample.mjs",
      version: 1,
      state: "loaded",
      error: null,
      logs: [
        {
          timestamp: "2026-08-30T12:34:56Z",
          level: "info",
          message: "a deliberately long console line that remains horizontally accessible",
        },
      ],
    },
    {
      id: "broken",
      fileName: "broken.js",
      version: null,
      state: "error",
      error: "SyntaxError: unexpected token",
      logs: [],
    },
  ],
};

describe("PluginSettingsPanel", () => {
  beforeEach(() => {
    api.invokeTypedCommand.mockReset();
    api.invokeTypedReadCommand.mockReset();
    plugins.loadPluginSnapshot.mockReset();
    plugins.publishPluginSnapshot.mockReset();
    plugins.subscribePluginSnapshot.mockClear();
    plugins.loadPluginSnapshot.mockResolvedValue(snapshot);
  });

  it("lists loaded and failed discoveries and preserves horizontal console access", async () => {
    render(<PluginSettingsPanel />);

    expect(await screen.findByText("sample.mjs")).toBeVisible();
    expect(screen.getByText("broken.js")).toBeVisible();
    expect(screen.getByText("SyntaxError: unexpected token")).toBeVisible();
    expect(screen.getByText("/tmp/awayuki/plugins")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Reload sample.mjs" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Reload broken" }),
    ).toBeVisible();
    const consoleLog = screen.getByLabelText("Console log");
    expect(consoleLog).toHaveClass("overflow-auto", "whitespace-pre");
    expect(consoleLog).toHaveTextContent(
      "a deliberately long console line that remains horizontally accessible",
    );

    fireEvent.click(screen.getByRole("button", { pressed: false }));
    expect(screen.getByLabelText("Console log")).toHaveTextContent(
      "No console messages",
    );
  });

  it("opens the plugin directory after its snapshot is available", async () => {
    let resolveOpen!: () => void;
    api.invokeTypedCommand.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveOpen = resolve;
        }),
    );
    render(<PluginSettingsPanel />);

    const openButton = screen.getByRole("button", { name: "Open directory" });
    expect(openButton).toBeDisabled();
    await screen.findByText("sample.mjs");
    expect(openButton).toBeEnabled();

    fireEvent.click(openButton);

    expect(api.invokeTypedCommand).toHaveBeenCalledWith("open_plugin_directory");
    expect(openButton).toBeDisabled();
    resolveOpen();
    await waitFor(() => expect(openButton).toBeEnabled());
  });

  it("shows an error when opening the plugin directory fails", async () => {
    api.invokeTypedCommand.mockRejectedValueOnce(new Error("open failed"));
    render(<PluginSettingsPanel />);
    await screen.findByText("sample.mjs");

    fireEvent.click(screen.getByRole("button", { name: "Open directory" }));

    expect(await screen.findByText("Error: open failed")).toHaveAttribute(
      "role",
      "alert",
    );
    expect(
      screen.getByRole("button", { name: "Open directory" }),
    ).toBeEnabled();
  });

  it("keeps an unloaded plugin discoverable and publishes the returned snapshot", async () => {
    const onlyPlugin = { ...snapshot, plugins: [snapshot.plugins[0]] };
    const unloaded = {
      ...onlyPlugin,
      revision: 4,
      plugins: [{ ...snapshot.plugins[0], state: "unloaded" }],
    };
    plugins.loadPluginSnapshot.mockResolvedValue(onlyPlugin);
    api.invokeTypedCommand.mockResolvedValue(unloaded);
    render(<PluginSettingsPanel />);
    await screen.findByText("sample.mjs");

    fireEvent.click(
      screen.getByRole("button", { name: "Unload sample.mjs" }),
    );

    await waitFor(() =>
      expect(api.invokeTypedCommand).toHaveBeenCalledWith("unload_plugin", {
        request: { pluginId: "sample.mjs" },
      }),
    );
    expect(plugins.publishPluginSnapshot).toHaveBeenCalledWith(unloaded);
    expect(screen.getByText("sample.mjs")).toBeVisible();
    expect(screen.getByText("Unloaded")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Unload sample.mjs" }),
    ).toBeDisabled();
  });

  it("reloads one plugin and reloads all plugins through typed commands", async () => {
    plugins.loadPluginSnapshot.mockResolvedValue({
      ...snapshot,
      plugins: [snapshot.plugins[0]],
    });
    api.invokeTypedCommand.mockResolvedValue(snapshot);
    render(<PluginSettingsPanel />);
    await screen.findByText("sample.mjs");

    fireEvent.click(
      screen.getByRole("button", { name: "Reload sample.mjs" }),
    );
    await waitFor(() =>
      expect(api.invokeTypedCommand).toHaveBeenCalledWith("reload_plugin", {
        request: { pluginId: "sample.mjs" },
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Reload all" }));
    await waitFor(() =>
      expect(api.invokeTypedCommand).toHaveBeenCalledWith("reload_plugins"),
    );
    expect(plugins.publishPluginSnapshot).toHaveBeenCalledTimes(2);
  });

  it("refreshes background console output without reloading plugins", async () => {
    const refreshed = {
      ...snapshot,
      revision: 4,
      plugins: [
        {
          ...snapshot.plugins[0],
          logs: [
            ...snapshot.plugins[0].logs,
            {
              timestamp: "2026-08-30T12:35:01Z",
              level: "info",
              message: "background timer fired",
            },
          ],
        },
      ],
    };
    api.invokeTypedReadCommand.mockResolvedValue(refreshed);
    render(<PluginSettingsPanel />);
    await screen.findByText("sample.mjs");

    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));

    await waitFor(() =>
      expect(api.invokeTypedReadCommand).toHaveBeenCalledWith("plugin_snapshot"),
    );
    expect(screen.getByLabelText("Console log")).toHaveTextContent(
      "background timer fired",
    );
    expect(api.invokeTypedCommand).not.toHaveBeenCalledWith("reload_plugins");
    expect(plugins.publishPluginSnapshot).toHaveBeenCalledWith(refreshed);
  });
});
