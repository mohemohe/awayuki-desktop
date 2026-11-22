import { afterEach, describe, expect, it } from "vitest";
import {
  NATIVE_OVERLAY_ATTRIBUTE,
  nativeOverlayRects,
  placeMenu,
} from "./menuPlacement";

const viewport = { width: 1000, height: 600 };
const size = { width: 160, height: 200 };
const sidecar = { left: 700, right: 1000, top: 40, bottom: 580 };

describe("placeMenu", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("opens at the anchor when it fits beside a native overlay", () => {
    expect(
      placeMenu({
        anchor: { top: 100, left: 400 },
        size,
        viewport,
        obstacles: [sidecar],
        margin: 8,
      }),
    ).toEqual({ top: 100, left: 400 });
  });

  it("flips a context menu to the left instead of covering the sidecar", () => {
    expect(
      placeMenu({
        anchor: { top: 100, left: 620 },
        size,
        viewport,
        obstacles: [sidecar],
        margin: 8,
      }),
    ).toEqual({ top: 100, left: 460 });
  });

  it("keeps the menu inside the free span when neither side fits", () => {
    expect(
      placeMenu({
        anchor: { top: 100, left: 600 },
        size,
        viewport,
        obstacles: [{ left: 0, right: 500, top: 0, bottom: 600 }, sidecar],
        margin: 8,
      }),
    ).toEqual({ top: 100, left: 540 });
  });

  it("ignores overlays that do not overlap vertically", () => {
    expect(
      placeMenu({
        anchor: { top: 300, left: 620 },
        size,
        viewport,
        obstacles: [{ left: 700, right: 1000, top: 0, bottom: 100 }],
        margin: 8,
      }),
    ).toEqual({ top: 300, left: 620 });
  });

  it("clamps vertically and keeps right-anchored menus off the overlay", () => {
    expect(
      placeMenu({
        anchor: { top: 550, right: 320 },
        size,
        viewport,
        obstacles: [{ left: 0, right: 600, top: 0, bottom: 600 }],
        margin: 8,
      }),
    ).toEqual({ top: 392, right: 240 });
  });

  it("collects only marked overlays with a visible size", () => {
    const marked = document.createElement("div");
    marked.setAttribute(NATIVE_OVERLAY_ATTRIBUTE, "");
    marked.getBoundingClientRect = () =>
      ({ ...sidecar, width: 300, height: 540 }) as DOMRect;
    const empty = document.createElement("div");
    empty.setAttribute(NATIVE_OVERLAY_ATTRIBUTE, "");
    document.body.append(marked, empty, document.createElement("div"));

    expect(nativeOverlayRects()).toEqual([
      expect.objectContaining({ left: 700, right: 1000 }),
    ]);
  });
});
