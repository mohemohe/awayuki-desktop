import { describe, expect, it } from "vitest";
import {
  SIDECAR_DEFAULT_WIDTH,
  SIDECAR_MIN_WIDTH,
  SidecarLifecycleManager,
  isSupportedSidecarUrl,
  normalizeSidecarSettings,
  normalizeSidecarWidth,
} from "./sidecar";

describe("sidecar URL policy", () => {
  it.each([
    "https://example.com",
    "http://localhost:3000/path",
    " HTTPS://EXAMPLE.COM/path ",
  ])("allows http/https URLs with a host: %s", (url) => {
    expect(isSupportedSidecarUrl(url)).toBe(true);
  });

  it.each([
    "javascript:alert(1)",
    "file:///tmp/private",
    "data:text/html,hello",
    "https://",
    "not a URL",
  ])("rejects URLs outside policy: %s", (url) => {
    expect(isSupportedSidecarUrl(url)).toBe(false);
  });
});

describe("sidecar normalization", () => {
  it("normalizes widths and drops entries outside the URL policy", () => {
    expect(normalizeSidecarWidth(1)).toBe(SIDECAR_MIN_WIDTH);
    expect(normalizeSidecarWidth("invalid")).toBe(SIDECAR_DEFAULT_WIDTH);
    expect(
      normalizeSidecarSettings({
        entries: [
          {
            id: "ok",
            name: "  News  ",
            url: " https://example.com ",
            userStyleEnabled: false,
            userStyle: "",
            width: 1,
          },
          {
            id: "blocked",
            name: "Local",
            url: "file:///tmp/private",
            userStyleEnabled: false,
            userStyle: "",
            width: 500,
          },
        ],
        mainViewIndex: 9,
      }),
    ).toEqual({
      entries: [
        {
          id: "ok",
          name: "News",
          url: "https://example.com",
          userStyleEnabled: false,
          userStyle: "",
          width: SIDECAR_MIN_WIDTH,
        },
      ],
      mainViewIndex: 0,
    });
  });
});

describe("SidecarLifecycleManager", () => {
  it("aborts older generations and rejects their completions", () => {
    const manager = new SidecarLifecycleManager();
    const create = manager.begin("news", "creating");
    const navigate = manager.begin("news", "navigating");

    expect(create.signal.aborted).toBe(true);
    expect(manager.isCurrent(create)).toBe(false);
    expect(manager.transition(create, "ready")).toBe(false);
    expect(manager.isCurrent(navigate)).toBe(true);
    expect(manager.transition(navigate, "visible")).toBe(true);
    expect(manager.status("news")).toBe("visible");
  });

  it("cancels every in-flight operation during component cleanup", () => {
    const manager = new SidecarLifecycleManager();
    const first = manager.begin("first", "ready");
    const second = manager.begin("second", "creating");

    manager.cancelAll();

    expect(first.signal.aborted).toBe(true);
    expect(second.signal.aborted).toBe(true);
    expect(manager.ids()).toEqual([]);
  });
});
