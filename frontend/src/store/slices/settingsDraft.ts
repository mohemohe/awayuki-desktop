import type { SettingsSnapshot } from "../../types/app";
import {
  SETTING_KEYS,
  SETTING_SNAPSHOT_FIELD_BY_KEY,
  type SettingKey as GeneratedSettingKey,
} from "../../api/generated/contract";

export const settingKeys = SETTING_KEYS;

export type SettingKey = GeneratedSettingKey;

function isSettingKey(key: string): key is SettingKey {
  return Object.prototype.hasOwnProperty.call(
    SETTING_SNAPSHOT_FIELD_BY_KEY,
    key,
  );
}

export function settingValue(
  settings: SettingsSnapshot,
  key: string,
): unknown {
  if (!isSettingKey(key)) return undefined;
  return settings[SETTING_SNAPSHOT_FIELD_BY_KEY[key]];
}

export function reduceSettingDraft(
  settings: SettingsSnapshot,
  key: string,
  value: unknown,
): SettingsSnapshot {
  if (!isSettingKey(key)) return settings;
  const field = SETTING_SNAPSHOT_FIELD_BY_KEY[key];
  return { ...settings, [field]: value } as SettingsSnapshot;
}

export function applyPersistedSetting(
  current: SettingsSnapshot,
  persisted: SettingsSnapshot,
  key: string,
) {
  return reduceSettingDraft(current, key, settingValue(persisted, key));
}
