import type { SettingsSnapshot } from "../../types/app";

export const settingKeys = [
  "appearance",
  "performance",
  "confirmation",
  "bluesky_fetch",
  "sidecars",
  "account_source_colors",
  "preset_visibility",
  "debug",
  "notification_suppression",
] as const;

export type SettingKey = (typeof settingKeys)[number];

export function settingValue(
  settings: SettingsSnapshot,
  key: string,
): unknown {
  if (key === "appearance") return settings.appearance;
  if (key === "performance") return settings.performance;
  if (key === "confirmation") return settings.confirmation;
  if (key === "bluesky_fetch") return settings.blueskyFetch;
  if (key === "sidecars") return settings.sidecars;
  if (key === "account_source_colors") return settings.accountSourceColors;
  if (key === "preset_visibility") return settings.presetVisibility;
  if (key === "debug") return settings.debug;
  if (key === "notification_suppression") {
    return settings.notificationSuppression;
  }
  return undefined;
}

export function reduceSettingDraft(
  settings: SettingsSnapshot,
  key: string,
  value: unknown,
): SettingsSnapshot {
  if (key === "appearance") {
    return { ...settings, appearance: value as SettingsSnapshot["appearance"] };
  }
  if (key === "performance") {
    return {
      ...settings,
      performance: value as SettingsSnapshot["performance"],
    };
  }
  if (key === "confirmation") {
    return {
      ...settings,
      confirmation: value as SettingsSnapshot["confirmation"],
    };
  }
  if (key === "bluesky_fetch") {
    return {
      ...settings,
      blueskyFetch: value as SettingsSnapshot["blueskyFetch"],
    };
  }
  if (key === "sidecars") {
    return { ...settings, sidecars: value as SettingsSnapshot["sidecars"] };
  }
  if (key === "account_source_colors") {
    return {
      ...settings,
      accountSourceColors: value as SettingsSnapshot["accountSourceColors"],
    };
  }
  if (key === "preset_visibility") {
    return {
      ...settings,
      presetVisibility: value as SettingsSnapshot["presetVisibility"],
    };
  }
  if (key === "debug") {
    return { ...settings, debug: value as SettingsSnapshot["debug"] };
  }
  if (key === "notification_suppression") {
    return {
      ...settings,
      notificationSuppression:
        value as SettingsSnapshot["notificationSuppression"],
    };
  }
  return settings;
}

export function applyPersistedSetting(
  current: SettingsSnapshot,
  persisted: SettingsSnapshot,
  key: string,
) {
  return reduceSettingDraft(current, key, settingValue(persisted, key));
}

