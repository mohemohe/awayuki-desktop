/**
 * Native child WebViews (sidecars) are composited above the main WebView, so
 * HTML popovers cannot be stacked over them. Elements that host such a native
 * surface carry this attribute and popovers avoid their rectangles.
 */
export const NATIVE_OVERLAY_ATTRIBUTE = "data-native-overlay";

export type MenuAnchor = { top: number; left?: number; right?: number };

type Rect = { left: number; right: number; top: number; bottom: number };

export function nativeOverlayRects(root: ParentNode = document): Rect[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>(`[${NATIVE_OVERLAY_ATTRIBUTE}]`),
  )
    .map((element) => element.getBoundingClientRect())
    .filter((rect) => rect.width > 0 && rect.height > 0);
}

/**
 * Places a menu inside the viewport without covering native overlays.
 * Left-anchored menus (context menus) flip to the left of the anchor when the
 * right side has no room; right-anchored menus keep their anchor edge when it
 * fits and are otherwise shifted into the free horizontal span.
 */
export function placeMenu({
  anchor,
  size,
  viewport,
  obstacles,
  margin,
}: {
  anchor: MenuAnchor;
  size: { width: number; height: number };
  viewport: { width: number; height: number };
  obstacles: readonly Rect[];
  margin: number;
}): MenuAnchor {
  const maxTop = viewport.height - size.height - margin;
  const top = Math.max(margin, Math.min(anchor.top, maxTop));
  const rightAnchored = anchor.left === undefined;
  const anchorX = rightAnchored
    ? viewport.width - (anchor.right ?? margin)
    : (anchor.left ?? margin);

  let minX = margin;
  let maxX = viewport.width - margin;
  for (const obstacle of obstacles) {
    if (obstacle.bottom <= top || obstacle.top >= top + size.height) continue;
    if (obstacle.right <= anchorX) minX = Math.max(minX, obstacle.right);
    else if (obstacle.left >= anchorX) maxX = Math.min(maxX, obstacle.left);
  }

  let left: number;
  if (rightAnchored) {
    left = anchorX - size.width;
  } else if (anchorX + size.width <= maxX) {
    left = anchorX;
  } else if (anchorX - size.width >= minX) {
    left = anchorX - size.width;
  } else {
    left = maxX - size.width;
  }
  left = Math.max(minX, Math.min(left, maxX - size.width));

  return rightAnchored
    ? { top, right: viewport.width - (left + size.width) }
    : { top, left };
}
