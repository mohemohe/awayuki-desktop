import React from "react";
import { createPortal } from "react-dom";
import { focusableElements, useFocusLifecycle } from "./focus";
import { nativeOverlayRects, placeMenu } from "./menuPlacement";

export type MenuActionItem = {
  id: string;
  kind?: "action";
  label: string;
  action: () => void;
  disabled?: boolean;
  danger?: boolean;
};

export type MenuSeparatorItem = {
  id: string;
  kind: "separator";
};

export type MenuItem = MenuActionItem | MenuSeparatorItem;

const MENU_VIEWPORT_MARGIN = 8;

export function useMenuFocus(open: boolean, onClose: () => void) {
  const ref = React.useRef<HTMLElement>(null);
  useFocusLifecycle({
    active: open,
    containerRef: ref,
    onEscape: onClose,
    trap: false,
  });
  const onKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const options = ref.current ? focusableElements(ref.current) : [];
    if (options.length === 0) return;
    const current = options.indexOf(document.activeElement as HTMLElement);
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? options.length - 1
          : (current + (event.key === "ArrowDown" ? 1 : -1) + options.length) %
            options.length;
    options[next]?.focus();
  };
  return { menuRef: ref, onMenuKeyDown: onKeyDown };
}

export function MenuPopover({
  position,
  items,
  onClose,
  widthClassName = "w-36",
}: {
  position: { top: number; left?: number; right?: number };
  items: readonly MenuItem[];
  onClose: () => void;
  widthClassName?: string;
}) {
  const { menuRef, onMenuKeyDown } = useMenuFocus(true, onClose);
  const { top: requestedTop, left: requestedLeft, right: requestedRight } =
    position;
  const [adjustedPosition, setAdjustedPosition] = React.useState(position);

  React.useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) {
      setAdjustedPosition({
        top: requestedTop,
        left: requestedLeft,
        right: requestedRight,
      });
      return;
    }
    const rect = menu.getBoundingClientRect();
    setAdjustedPosition(
      placeMenu({
        anchor: {
          top: requestedTop,
          left: requestedLeft,
          right: requestedRight,
        },
        size: { width: rect.width, height: rect.height },
        viewport: { width: window.innerWidth, height: window.innerHeight },
        obstacles: nativeOverlayRects(),
        margin: MENU_VIEWPORT_MARGIN,
      }),
    );
  }, [menuRef, requestedLeft, requestedRight, requestedTop]);

  React.useEffect(() => {
    const close = (event: PointerEvent) => {
      if (menuRef.current?.contains(event.target as Node | null)) return;
      onClose();
    };
    const closeOnViewportChange = () => onClose();
    document.addEventListener("pointerdown", close, true);
    window.addEventListener("scroll", closeOnViewportChange, true);
    window.addEventListener("resize", closeOnViewportChange);
    return () => {
      document.removeEventListener("pointerdown", close, true);
      window.removeEventListener("scroll", closeOnViewportChange, true);
      window.removeEventListener("resize", closeOnViewportChange);
    };
  }, [menuRef, onClose]);

  return createPortal(
    <div
      ref={menuRef as React.RefObject<HTMLDivElement>}
      role="menu"
      className={`fixed z-50 ${widthClassName} rounded-md border border-surface0 bg-base-100 p-1 text-sm text-text shadow-xl`}
      style={{
        top: adjustedPosition.top,
        ...(adjustedPosition.left !== undefined
          ? { left: adjustedPosition.left }
          : { right: adjustedPosition.right ?? 8 }),
      }}
      data-post-menu
      onKeyDown={onMenuKeyDown}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item) =>
        item.kind === "separator" ? (
          <div
            key={item.id}
            role="separator"
            className="my-1 border-t border-surface0"
          />
        ) : (
          <button
            key={item.id}
            type="button"
            role="menuitem"
            className={`block w-full rounded px-3 py-2 text-left hover:bg-surface0 disabled:cursor-not-allowed disabled:text-overlay0 disabled:hover:bg-transparent ${item.danger ? "text-red" : ""}`}
            disabled={item.disabled}
            onClick={() => {
              if (item.disabled) return;
              item.action();
              onClose();
            }}
          >
            {item.label}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
