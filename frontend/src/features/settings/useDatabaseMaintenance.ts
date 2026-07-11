import { invokeCommand } from "../../api/tauri";
import { isResponseLossError } from "../../api/ipcErrors";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { DbSummary } from "../../types/app";

export type DatabaseMaintenanceCommand =
  | "vacuum_database"
  | "clear_status_cache";

export function useDatabaseMaintenance() {
  const refresh = useAppStore((state) => state.loadSnapshot);
  const requestConfirmation = useAppStore((state) => state.requestConfirmation);
  const runMutation = useAppStore((state) => state.runMutation);
  const mutationStates = useAppStore((state) => state.mutationStates);

  const run = async (command: DatabaseMaintenanceCommand) => {
    await runMutation(`database:${command}`, {
      confirm: () =>
        requestConfirmation({
          title:
            command === "vacuum_database"
              ? t("Vacuum database")
              : t("Clear Status Cache"),
          message:
            command === "vacuum_database"
              ? t("Run database maintenance now?")
              : t("Delete all cached statuses? This cannot be undone."),
          confirmLabel:
            command === "vacuum_database" ? t("Vacuum") : t("Delete"),
          danger: command === "clear_status_cache",
        }),
      execute: async () => {
        const result = await invokeCommand<DbSummary>(command);
        await refresh();
        return result;
      },
      isUncertain: isResponseLossError,
    });
  };

  return {
    run,
    vacuumState: mutationStates["database:vacuum_database"],
    clearState: mutationStates["database:clear_status_cache"],
  };
}

