import React from "react";
import { ChevronLeft } from "lucide-react";
import {
  settingsSectionDescriptor,
  settingsSections,
  type SettingsSection,
} from "../../features/settings/descriptors";
import { t } from "../../i18n";
import { Tab, TabList } from "../primitives/Tabs";

export function SettingsViewLayout({
  selectedSection,
  onSelectSection,
  onBack,
  saveIndicator,
  panel,
}: {
  selectedSection: SettingsSection;
  onSelectSection: (section: SettingsSection) => void;
  onBack: () => void;
  saveIndicator?: React.ReactNode;
  panel: React.ReactNode;
}) {
  const descriptor = settingsSectionDescriptor(selectedSection);

  return (
    <div className="flex h-screen flex-col bg-base-100">
      <div
        className="h-8 shrink-0 border-b border-surface0 bg-base-300"
        data-tauri-drag-region
      />
      <div className="flex min-h-0 flex-1">
        <aside className="w-40 shrink-0 border-r border-surface0 bg-base-300">
          <button
            className="btn btn-secondary btn-sm m-2 h-8 min-h-8 px-4 text-sm font-normal"
            onClick={onBack}
          >
            <ChevronLeft className="h-4 w-4" />
            {t("Back")}
          </button>
          <TabList
            label={t("Settings")}
            orientation="vertical"
            className="mt-2 flex flex-col"
          >
            {settingsSections.map((section) => (
              <Tab
                key={section.id}
                id={`settings-tab-${section.id}`}
                controls={`settings-panel-${section.id}`}
                selected={selectedSection === section.id}
                className={`h-10 px-3 text-left text-sm font-normal ${selectedSection === section.id ? "bg-surface0 text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
                onSelect={() => onSelectSection(section.id)}
              >
                {t(section.labelId)}
              </Tab>
            ))}
          </TabList>
          {saveIndicator}
        </aside>
        <section
          id={`settings-panel-${selectedSection}`}
          role="tabpanel"
          aria-labelledby={`settings-tab-${selectedSection}`}
          className={`min-w-0 flex-1 overflow-y-auto bg-base ${descriptor.fullWidth ? "" : "px-6 py-7"}`}
        >
          {descriptor.fullWidth ? (
            panel
          ) : (
            <div className="settings-content">
              <h1 className="mb-5 text-lg font-normal text-text">
                {t(descriptor.labelId)}
              </h1>
              {panel}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
