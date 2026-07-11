import { describe, expect, it } from "vitest";
import { settingsSections } from "./descriptors";

describe("settings section descriptors", () => {
  it("has unique semantic ids and labels", () => {
    expect(new Set(settingsSections.map(({ id }) => id)).size).toBe(
      settingsSections.length,
    );
    for (const descriptor of settingsSections) {
      expect(descriptor.labelId).toMatch(/^settings\.section\./);
    }
  });
});
