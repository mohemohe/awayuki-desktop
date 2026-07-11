import React from "react";
import { createPortal } from "react-dom";
import { useFocusLifecycle } from "./focus";

export function Dialog({
  open,
  onClose,
  label,
  labelledBy,
  className,
  children,
  ...dialogProps
}: {
  open: boolean;
  onClose: () => void;
  label?: string;
  labelledBy?: string;
  className?: string;
  children: React.ReactNode;
} & Omit<
  React.HTMLAttributes<HTMLDivElement>,
  "role" | "aria-modal" | "aria-label" | "aria-labelledby"
>) {
  const ref = React.useRef<HTMLDivElement>(null);
  useFocusLifecycle({
    active: open,
    containerRef: ref,
    onEscape: onClose,
  });
  if (!open) return null;
  return createPortal(
    <div
      ref={ref}
      role="dialog"
      aria-modal="true"
      aria-label={label}
      aria-labelledby={labelledBy}
      tabIndex={-1}
      className={className}
      {...dialogProps}
    >
      {children}
    </div>,
    document.body,
  );
}
