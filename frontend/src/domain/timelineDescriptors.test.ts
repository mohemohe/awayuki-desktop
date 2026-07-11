import { describe, expect, it } from "vitest";
import type { SessionCapabilities } from "../types/app";
import {
  availableConfigurableTimelineTypes,
  configurableTimelineTypes,
  timelineDescriptor,
  timelineDescriptorRegistry,
  timelineTypeRequiresAccount,
} from "./timelineDescriptors";

const capabilities: SessionCapabilities = {
  protocol: "atProto",
  timelines: {
    home: true,
    public: false,
    local: false,
    lists: false,
    hashtags: false,
    notifications: true,
    bookmarks: false,
    favourites: false,
  },
  status: {
    favourite: true,
    reblog: true,
    bookmark: false,
    vote: false,
    edit: false,
    delete: true,
  },
  relationship: { follow: true, mute: true, block: true },
  compose: {
    mediaUpload: true,
    poll: false,
    quote: true,
    maxMediaAttachments: 4,
    maxCharacters: 300,
  },
  streaming: false,
};

describe("timeline descriptor registry", () => {
  it("is exhaustive and every type has all behavioral metadata", () => {
    for (const [type, descriptor] of Object.entries(
      timelineDescriptorRegistry,
    )) {
      expect(descriptor.type).toBe(type);
      expect(descriptor.labelId).toMatch(/^timeline\./);
      expect(descriptor.defaultName).not.toBe("");
      expect(descriptor.loadStrategy).toBeTruthy();
      expect(descriptor.streamPolicy).toBeTruthy();
      expect(descriptor.parameterEditor).toBeTruthy();
      expect(["api", "local", "none"]).toContain(descriptor.pagination);
      expect(typeof descriptor.supportsDisplayFilter).toBe("boolean");
    }
    expect(configurableTimelineTypes).toHaveLength(10);
  });

  it("filters unsupported configurable types by backend capability", () => {
    expect(availableConfigurableTimelineTypes(capabilities)).toEqual([
      "home",
      "notification",
      "custom",
      "yq",
    ]);
  });

  it("returns no descriptor for a future saved value", () => {
    expect(timelineDescriptor("future-timeline")).toBeUndefined();
  });

  it("requires explicit account scope only for remote account-bound timelines", () => {
    expect(["local", "hashtag", "list"].map(timelineTypeRequiresAccount)).toEqual([
      true,
      true,
      true,
    ]);
    expect(
      [
        "home",
        "public",
        "notification",
        "bookmarks",
        "favourites",
        "custom",
        "yq",
        "search",
        "thread",
      ].some(timelineTypeRequiresAccount),
    ).toBe(false);
  });
});
