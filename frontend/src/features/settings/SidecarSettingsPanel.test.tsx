import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot } from "../../types/app";

vi.mock("../../components/common/SqlEditor", () => ({
  CssEditor: () => null,
}));

import { useAppStore } from "../../store/appStore";
import { SidecarSettingsPanel } from "./SidecarSettingsPanel";

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

describe("SidecarSettingsPanel", () => {
  const saveSetting = vi.fn(async (_key: string, _value: unknown) => undefined);
  const previousSnapshot = useAppStore.getState().snapshot;
  const previousSaveSetting = useAppStore.getState().saveSetting;
  let unmount: (() => void) | undefined;

  beforeEach(() => {
    saveSetting.mockClear();
    useAppStore.setState({ snapshot, saveSetting });
  });

  afterEach(() => {
    unmount?.();
    unmount = undefined;
    useAppStore.setState({
      snapshot: previousSnapshot,
      saveSetting: previousSaveSetting,
    });
  });

  it("persists an empty sidecar list after removing the final entry", async () => {
    ({ unmount } = render(<SidecarSettingsPanel />));

    fireEvent.click(screen.getByRole("button", { name: "Remove Sidecar" }));

    expect(screen.getByRole("button", { name: "Save" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(saveSetting).toHaveBeenCalledWith("sidecars", {
        entries: [],
        mainViewIndex: 0,
      }),
    );
  });
});
