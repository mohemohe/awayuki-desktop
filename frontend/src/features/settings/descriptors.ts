import type { MessageId } from "../../i18n";

export const settingsSections = [
  { id: "Account", labelId: "settings.section.account", fullWidth: false },
  { id: "Timeline", labelId: "settings.section.timeline", fullWidth: true },
  { id: "Sidecar", labelId: "settings.section.sidecar", fullWidth: true },
  {
    id: "Notification",
    labelId: "settings.section.notification",
    fullWidth: false,
  },
  { id: "Behavior", labelId: "settings.section.behavior", fullWidth: false },
  {
    id: "Appearance",
    labelId: "settings.section.appearance",
    fullWidth: false,
  },
  {
    id: "Performance",
    labelId: "settings.section.performance",
    fullWidth: false,
  },
  { id: "Plugin", labelId: "settings.section.plugin", fullWidth: false },
  { id: "Database", labelId: "settings.section.database", fullWidth: false },
  { id: "Debug", labelId: "settings.section.debug", fullWidth: false },
  { id: "About", labelId: "settings.section.about", fullWidth: false },
] as const satisfies readonly {
  id: string;
  labelId: MessageId;
  fullWidth: boolean;
}[];

export type SettingsSection = (typeof settingsSections)[number]["id"];

export function settingsSectionDescriptor(section: SettingsSection) {
  return settingsSections.find((descriptor) => descriptor.id === section)!;
}
