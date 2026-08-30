import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getAppLocale, setAppLocale } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { AppSnapshot } from "../../types/app";
import { PerformanceSettingsPanel } from "./GeneralSettingsPanels";

const snapshot = {
  settings: {
    performance: {
      mention_source: "SQLite",
      hashtag_source: "SQLite",
      timeline_renderer: "VirtualList",
      sidecar_hidden_tab_behavior: "Keep",
    },
  },
} as AppSnapshot;

describe("PerformanceSettingsPanel", () => {
  const saveSetting = vi.fn(async (_key: string, _value: unknown) => undefined);
  const previousSnapshot = useAppStore.getState().snapshot;
  const previousSaveSetting = useAppStore.getState().saveSetting;
  const previousLocale = getAppLocale();
  let unmount: (() => void) | undefined;

  beforeEach(() => {
    saveSetting.mockClear();
    setAppLocale("ja");
    useAppStore.setState({ snapshot, saveSetting });
  });

  afterEach(() => {
    unmount?.();
    unmount = undefined;
    useAppStore.setState({
      snapshot: previousSnapshot,
      saveSetting: previousSaveSetting,
    });
    setAppLocale(previousLocale);
  });

  it("defaults hidden sidecar tabs to keep and can save discard", async () => {
    ({ unmount } = render(<PerformanceSettingsPanel />));

    const select = screen.getByRole("combobox", {
      name: "サイドカーの非表示タブ",
    });
    expect(select).toHaveValue("Keep");
    expect(screen.getByRole("option", { name: "保持する" })).toBeVisible();
    expect(screen.getByRole("option", { name: "破棄する" })).toBeVisible();

    fireEvent.change(select, { target: { value: "Discard" } });

    await waitFor(() =>
      expect(saveSetting).toHaveBeenCalledWith("performance", {
        mention_source: "SQLite",
        hashtag_source: "SQLite",
        timeline_renderer: "VirtualList",
        sidecar_hidden_tab_behavior: "Discard",
      }),
    );
  });
});
