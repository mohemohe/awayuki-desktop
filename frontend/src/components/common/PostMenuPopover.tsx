import React from "react";
import { createPortal } from "react-dom";

export function PostMenuPopover({
  position,
  items,
  onClose,
  widthClassName = "w-36",
}: {
  position: { top: number; left?: number; right?: number };
  items: Array<{
    label: string;
    action: () => void;
    disabled?: boolean;
    danger?: boolean;
  }>;
  onClose: () => void;
  widthClassName?: string;
}) {
  React.useEffect(() => {
    const close = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (target?.closest("[data-post-menu]")) return;
      onClose();
    };
    const closeOnScroll = () => onClose();
    const closeOnKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };

    document.addEventListener("pointerdown", close, true);
    document.addEventListener("keydown", closeOnKey);
    window.addEventListener("scroll", closeOnScroll, true);
    window.addEventListener("resize", closeOnScroll);
    return () => {
      document.removeEventListener("pointerdown", close, true);
      document.removeEventListener("keydown", closeOnKey);
      window.removeEventListener("scroll", closeOnScroll, true);
      window.removeEventListener("resize", closeOnScroll);
    };
  }, [onClose]);

  return createPortal(
    <div
      className={`fixed z-50 ${widthClassName} rounded-md border border-surface0 bg-base-100 p-1 text-sm text-text shadow-xl`}
      style={{
        top: position.top,
        ...(position.left !== undefined
          ? { left: position.left }
          : { right: position.right ?? 8 }),
      }}
      data-post-menu
    >
      {items.map((item) => (
        <button
          key={item.label}
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
