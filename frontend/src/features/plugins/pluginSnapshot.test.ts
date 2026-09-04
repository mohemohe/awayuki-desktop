import { beforeEach, describe, expect, it, vi } from "vitest";

import type { PluginSnapshot } from "../../types/app";

const api = vi.hoisted(() => ({
  invokeTypedCommand: vi.fn(),
  invokeTypedReadCommand: vi.fn(),
}));

vi.mock("../../api/tauri", () => api);

const snapshot = (revision: number): PluginSnapshot => ({
  directory: "/tmp/awayuki/plugins",
  revision,
  plugins: [],
  composeButtons: [],
});

describe("pluginSnapshot", () => {
  beforeEach(() => {
    vi.resetModules();
    api.invokeTypedCommand.mockReset();
    api.invokeTypedReadCommand.mockReset();
  });

  it("deduplicates concurrent snapshot loads", async () => {
    let resolve: ((value: PluginSnapshot) => void) | undefined;
    api.invokeTypedReadCommand.mockReturnValue(
      new Promise<PluginSnapshot>((complete) => {
        resolve = complete;
      }),
    );
    const { loadPluginSnapshot } = await import("./pluginSnapshot");

    const first = loadPluginSnapshot();
    const second = loadPluginSnapshot();
    resolve?.(snapshot(1));

    await expect(first).resolves.toEqual(snapshot(1));
    await expect(second).resolves.toEqual(snapshot(1));
    expect(api.invokeTypedReadCommand).toHaveBeenCalledTimes(1);
    expect(api.invokeTypedReadCommand).toHaveBeenCalledWith("plugin_snapshot");
  });

  it("does not publish an older revision over the current snapshot", async () => {
    const { currentPluginSnapshot, publishPluginSnapshot } = await import(
      "./pluginSnapshot"
    );
    const listener = vi.fn();
    window.addEventListener("awayuki:plugins-changed", listener);

    publishPluginSnapshot(snapshot(2));
    publishPluginSnapshot(snapshot(1));

    expect(currentPluginSnapshot()).toEqual(snapshot(2));
    expect(listener).toHaveBeenCalledTimes(1);
    window.removeEventListener("awayuki:plugins-changed", listener);
  });

  it("refreshes the snapshot after invoking a compose button", async () => {
    api.invokeTypedCommand.mockResolvedValue({ text: "changed" });
    api.invokeTypedReadCommand.mockResolvedValue(snapshot(3));
    const { invokePluginComposeButton } = await import("./pluginSnapshot");
    const request = {
      pluginId: "tools",
      buttonId: "rewrite",
      generation: 2,
      compose: { text: "before" },
    };

    await expect(invokePluginComposeButton(request)).resolves.toEqual({
      text: "changed",
    });
    await vi.waitFor(() =>
      expect(api.invokeTypedReadCommand).toHaveBeenCalledWith("plugin_snapshot"),
    );
    expect(api.invokeTypedCommand).toHaveBeenCalledWith(
      "invoke_plugin_compose_button",
      { request },
    );
  });
});
