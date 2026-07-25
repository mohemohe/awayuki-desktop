import { describe, expect, it } from "vitest";
import { createMockFixture } from "../../api/mock";
import {
  SETTINGS_SCHEMA_VERSION,
  SETTING_DESCRIPTORS,
} from "../../api/generated/contract";
import {
  reduceSettingDraft,
  settingKeys,
  settingValue,
} from "./settingsDraft";

describe("settings draft reducer", () => {
  it("updates only its resource", () => {
    const settings = createMockFixture().settings;
    const next = reduceSettingDraft(settings, "appearance", {
      ...settings.appearance,
      font_size: "Large",
    });
    expect(next.appearance.font_size).toBe("Large");
    expect(next.performance).toBe(settings.performance);
    expect(settings.appearance.font_size).toBe("Medium");
  });

  it("ignores an unknown forward-compatible key", () => {
    const settings = createMockFixture().settings;
    expect(reduceSettingDraft(settings, "future", {})).toBe(settings);
  });

  it("uses the generated Rust settings registry for every snapshot field", () => {
    const settings = createMockFixture().settings;
    expect(SETTINGS_SCHEMA_VERSION).toBe(1);
    expect(settingKeys).toEqual(Object.keys(SETTING_DESCRIPTORS));
    for (const key of settingKeys) {
      const value = settingValue(settings, key);
      expect(value, key).not.toBeUndefined();
      expect(settingValue(reduceSettingDraft(settings, key, value), key)).toBe(
        value,
      );
    }
  });
});
