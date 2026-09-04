import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import type { AppSnapshot } from "../../types/app";
import { SettingsViewController } from "./SettingsViewController";

vi.mock("../../features/settings/PluginSettingsPanel", () => ({
  PluginSettingsPanel: () => <div>Plugin panel fixture</div>,
}));

describe("SettingsViewController plugin section", () => {
  const previousSnapshot = useAppStore.getState().snapshot;
  const previousSection = useAppStore.getState().selectedSettings;

  beforeEach(() => {
    useAppStore.setState({
      snapshot: {} as AppSnapshot,
      selectedSettings: "Plugin",
    });
  });

  afterEach(() => {
    useAppStore.setState({
      snapshot: previousSnapshot,
      selectedSettings: previousSection,
    });
  });

  it("renders the Plugins panel instead of falling through to About", () => {
    render(<SettingsViewController />);

    expect(screen.getByText("Plugin panel fixture")).toBeVisible();
    expect(screen.queryByText(/Tauri \/ React \/ Vite/)).not.toBeInTheDocument();
  });
});
