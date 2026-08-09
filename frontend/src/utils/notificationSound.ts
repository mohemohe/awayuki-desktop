import { t } from "../i18n";
import type { NotificationSound } from "../types/app";

export const notificationSoundValues = [
  "Default",
  "Silent",
  "Message",
  "Mail",
  "Reminder",
] as const satisfies readonly NotificationSound[];

export type PaneNotificationSound = NotificationSound | "Inherit";

export const paneNotificationSoundValues = [
  "Inherit",
  ...notificationSoundValues,
] as const satisfies readonly PaneNotificationSound[];

export function notificationSoundLabel(value: PaneNotificationSound) {
  switch (value) {
    case "Inherit":
      return t("Use global default");
    case "Default":
      return t("System default");
    case "Silent":
      return t("Silent");
    case "Message":
      return t("Message");
    case "Mail":
      return t("Mail");
    case "Reminder":
      return t("Reminder");
  }
}
