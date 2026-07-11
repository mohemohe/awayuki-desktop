import React from "react";
import { createPortal } from "react-dom";
import { focusableElements, useFocusLifecycle } from "./focus";

export type MenuItem = {
  id: string;
  label: string;
  action: () => void;
  disabled?: boolean;
  danger?: boolean;
};

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
        top: position.top,
        ...(position.left !== undefined
          ? { left: position.left }
          : { right: position.right ?? 8 }),
      }}
      data-post-menu
      onKeyDown={onMenuKeyDown}
    >
      {items.map((item) => (
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
      ))}
    </div>,
    document.body,
  );
}
