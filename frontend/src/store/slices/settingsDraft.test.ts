import { describe, expect, it } from "vitest";
import { createMockFixture } from "../../api/mock";
import { reduceSettingDraft } from "./settingsDraft";

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
});

