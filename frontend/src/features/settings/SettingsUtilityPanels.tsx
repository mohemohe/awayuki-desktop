import React from "react";
import { createInMemorySupportBundle } from "../../api/diagnostics";
import { invokeCommand } from "../../api/tauri";
import { t, translateKnownMessage } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { DebugSettings } from "../../types/app";
import { Metric } from "../../components/common/Metric";
import { SelectRow, ToggleRow } from "../../components/common/FormRows";
import { useDatabaseMaintenance } from "./useDatabaseMaintenance";

const optionLabel = (value: string) => translateKnownMessage(value);

export function DatabaseSettingsPanel() {
  const database = useAppStore((state) => state.snapshot!.database);
  const { run: runDbCommand, vacuumState, clearState } =
    useDatabaseMaintenance();
  const busy = (phase: string | undefined) =>
    phase === "confirming" || phase === "pending";
  const latestState = clearState ?? vacuumState;
  return (
    <div className="space-y-5 text-sm">
      <div className="grid max-w-4xl grid-cols-4 border border-surface0 bg-base-200">
        <Metric
          label={t("Statuses")}
          value={database.statusCount.toLocaleString()}
        />
        <Metric
          label={t("Last 24h")}
          value={database.recentStatusCount.toLocaleString()}
        />
        <Metric
          label={t("Accounts")}
          value={database.accountCount.toLocaleString()}
        />
        <Metric label={t("Size")} value={database.size} />
      </div>
      <div className="text-sm text-subtext0">{database.path}</div>
      <div className="flex gap-2">
        <button
          className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
          onClick={() => void runDbCommand("vacuum_database")}
          disabled={busy(vacuumState?.phase)}
        >
          {t("Vacuum")}
        </button>
        <button
          className="btn btn-error btn-sm h-8 min-h-8 px-4 text-sm font-normal"
          onClick={() => void runDbCommand("clear_status_cache")}
          disabled={busy(clearState?.phase)}
        >
          {t("Clear Status Cache")}
        </button>
      </div>
      {latestState ? (
        <p
          className={`text-xs ${latestState.phase === "failed" || latestState.phase === "uncertain" ? "text-red" : "text-subtext0"}`}
          role="status"
        >
          {latestState.phase === "confirming"
            ? t("Waiting for confirmation")
            : latestState.phase === "pending"
              ? t("Working")
              : latestState.phase === "succeeded"
                ? t("Completed")
                : latestState.phase === "uncertain"
                  ? t("The result is uncertain. Refresh before retrying.")
                  : latestState.phase === "failed"
                    ? t("Operation failed")
                    : t("Cancelled")}
        </p>
      ) : null}
    </div>
  );
}

export function DebugSettingsPanel() {
  const settings = useAppStore((state) => state.snapshot!.settings.debug);
  const save = useAppStore((state) => state.saveSetting);
  const [supportBundle, setSupportBundle] = React.useState<string>();
  const [supportError, setSupportError] = React.useState<string>();
  const update = (patch: Partial<DebugSettings>) =>
    void save("debug", { ...settings, ...patch });
  return (
    <div className="settings-grid">
      <ToggleRow
        label={t("File logging")}
        checked={settings.logging_enabled}
        onChange={(logging_enabled) => update({ logging_enabled })}
      />
      <SelectRow
        label={t("Log level")}
        value={settings.log_level}
        values={["Error", "Warn", "Info", "Debug", "Trace"]}
        optionLabel={optionLabel}
        onChange={(log_level) => update({ log_level })}
      />
      <button
        className="btn btn-secondary btn-sm h-8 min-h-8 justify-self-start px-4 text-sm font-normal"
        onClick={() => void invokeCommand("open_log_file")}
      >
        {t("Open Log")}
      </button>
      <button
        className="btn btn-secondary btn-sm h-8 min-h-8 justify-self-start px-4 text-sm font-normal"
        onClick={() => {
          setSupportError(undefined);
          void createInMemorySupportBundle()
            .then((bundle) => setSupportBundle(JSON.stringify(bundle, null, 2)))
            .catch((error) => setSupportError(String(error)));
        }}
      >
        {t("Generate diagnostics")}
      </button>
      {supportError ? (
        <p className="text-xs text-red" role="alert">
          {supportError}
        </p>
      ) : null}
      {supportBundle ? (
        <details className="col-span-full max-w-4xl" open>
          <summary className="cursor-pointer text-sm text-subtext0">
            {t("In-memory support bundle")}
          </summary>
          <pre className="mt-2 max-h-80 overflow-auto whitespace-pre-wrap break-all rounded bg-base-300 p-3 text-xs">
            {supportBundle}
          </pre>
        </details>
      ) : null}
    </div>
  );
}

export function AboutPanel() {
  const snapshot = useAppStore((state) => state.snapshot!);
  return (
    <div className="space-y-2 text-sm text-text">
      <div className="font-semibold">Awayuki {snapshot.version}</div>
      <div className="text-subtext0">Tauri / React / Vite / DaisyUI</div>
    </div>
  );
}
