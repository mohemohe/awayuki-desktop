import type { StoreApi } from "zustand";

import { invokeTypedCommand } from "../../api/tauri";
import type { SettingsMutationCoordinator } from "../../domain/settingsMutations";
import type { ColumnSummary, SettingsSnapshot } from "../../types/app";
import { normalizeColumns, reconcileActiveTabs } from "../../utils/columns";
import { reduceSettingDraft } from "../slices/settingsDraft";
import { cancelQuoteConsumer } from "./timelineQueryActions";
import type { AppStore } from "../appStore";

type SettingsActionContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  coordinator: SettingsMutationCoordinator<SettingsSnapshot>;
  removeTimelineColumns: (state: AppStore, columnIds: string[]) => Partial<AppStore>;
};

export function createSettingsActions({
  set,
  get,
  coordinator,
  removeTimelineColumns,
}: SettingsActionContext): Pick<
  AppStore,
  "saveSetting" | "flushSettingSaves" | "saveColumns"
> {
  return {
    saveSetting: async (key, value) => {
      set((state) =>
        state.snapshot
          ? {
              snapshot: {
                ...state.snapshot,
                settings: reduceSettingDraft(state.snapshot.settings, key, value),
              },
            }
          : {},
      );
      await coordinator.enqueue(key, value);
    },
    flushSettingSaves: () => coordinator.flush(),
    saveColumns: async (columns: ColumnSummary[]) => {
      try {
        const snapshot = await invokeTypedCommand("save_columns", {
          request: { columns: normalizeColumns(columns) },
        });
        const retained = new Set(
          [...snapshot.columns, ...get().dynamicColumns].map((column) => column.id),
        );
        const removed = Object.keys(get().timelineKeys).filter(
          (columnId) => !retained.has(columnId),
        );
        for (const columnId of removed) {
          void cancelQuoteConsumer(columnId).catch(() => undefined);
        }
        set((state) => {
          return {
            ...removeTimelineColumns(state, removed),
            snapshot,
            activeTabs: reconcileActiveTabs(snapshot.columns, state.activeTabs),
          };
        });
        await Promise.all(
          snapshot.columns.map((column) => get().loadTimeline(column, true)),
        );
      } catch (error) {
        set({ error: String(error) });
      }
    },
  };
}
