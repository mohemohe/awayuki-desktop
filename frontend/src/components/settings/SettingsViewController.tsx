import React from "react";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { SettingsSection } from "../../features/settings/descriptors";
import {
  AccountSettings,
  AppearanceSettingsPanel,
  BehaviorSettingsPanel,
  NotificationSettingsPanel,
  PerformanceSettingsPanel,
} from "../../features/settings/GeneralSettingsPanels";
import { SidecarSettingsPanel } from "../../features/settings/SidecarSettingsPanel";
import { PluginSettingsPanel } from "../../features/settings/PluginSettingsPanel";
import { TimelineSettingsPanel } from "../../features/settings/TimelineSettingsPanel";
import {
  AboutPanel,
  DatabaseSettingsPanel,
  DebugSettingsPanel,
} from "../../features/settings/SettingsUtilityPanels";
import { useAppLocale } from "../../hooks/useAppLocale";
import { SettingsViewLayout } from "./SettingsViewLayout";

export function SettingsViewController() {
  useAppLocale();
  const selectedSettings = useAppStore((state) => state.selectedSettings);
  const flushSettingSaves = useAppStore((state) => state.flushSettingSaves);
  const closeSettings = () => {
    void flushSettingSaves().finally(() => {
      useAppStore.setState({ settingsOpen: false });
    });
  };

  React.useEffect(
    () => () => {
      void flushSettingSaves();
    },
    [flushSettingSaves],
  );

  return (
    <SettingsViewLayout
      selectedSection={selectedSettings}
      onSelectSection={(selectedSettings) =>
        useAppStore.setState({ selectedSettings })
      }
      onBack={closeSettings}
      saveIndicator={<SettingsSaveIndicator />}
      panel={<SettingsPanel section={selectedSettings} />}
    />
  );
}

function SettingsSaveIndicator() {
  const mutations = useAppStore((state) => state.settingMutations);
  const entries = Object.values(mutations);
  const failed = entries.find(
    (entry) => entry.phase === "failed" || entry.phase === "conflict",
  );
  const saving = entries.some(
    (entry) => entry.phase === "dirty" || entry.phase === "saving",
  );
  const saved = entries.some((entry) => entry.phase === "saved");
  if (!failed && !saving && !saved) return null;
  return (
    <div
      className={`mx-3 mt-4 text-xs ${failed ? "text-red" : "text-subtext0"}`}
      role="status"
      aria-live="polite"
    >
      {failed
        ? failed.phase === "conflict"
          ? t("Settings changed while switching accounts")
          : t("Settings could not be saved")
        : saving
          ? t("Saving settings")
          : t("Settings saved")}
    </div>
  );
}

function SettingsPanel({ section }: { section: SettingsSection }) {
  const snapshot = useAppStore((state) => state.snapshot);
  if (!snapshot) return null;
  if (section === "Account") return <AccountSettings />;
  if (section === "Appearance") return <AppearanceSettingsPanel />;
  if (section === "Behavior") return <BehaviorSettingsPanel />;
  if (section === "Performance") return <PerformanceSettingsPanel />;
  if (section === "Notification") return <NotificationSettingsPanel />;
  if (section === "Timeline") return <TimelineSettingsPanel />;
  if (section === "Sidecar") return <SidecarSettingsPanel />;
  if (section === "Plugin") return <PluginSettingsPanel />;
  if (section === "Database") return <DatabaseSettingsPanel />;
  if (section === "Debug") return <DebugSettingsPanel />;
  return <AboutPanel />;
}
