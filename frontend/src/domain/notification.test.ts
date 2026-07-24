import { describe, expect, it } from "vitest";
import {
  notificationKindIsReblog,
  semanticNotificationKind,
} from "./notification";

describe("semantic notification kind", () => {
  it("normalizes provider spellings without consulting display text", () => {
    expect(semanticNotificationKind("favorite")).toBe("favourite");
    expect(semanticNotificationKind("boost")).toBe("reblog");
    expect(semanticNotificationKind("repost")).toBe("reblog");
    expect(semanticNotificationKind("follow_request")).toBe("follow");
    expect(semanticNotificationKind("future-provider-kind")).toBe("unknown");
  });

  it("does not infer boost behavior from a translated label", () => {
    expect(notificationKindIsReblog("mention")).toBe(false);
    expect(notificationKindIsReblog("reblog")).toBe(true);
  });
});
