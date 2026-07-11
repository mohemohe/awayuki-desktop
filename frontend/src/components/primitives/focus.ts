import React from "react";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

export function focusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (element) => !element.hidden && element.getAttribute("aria-hidden") !== "true",
  );
}

export function useFocusLifecycle({
  active,
  containerRef,
  onEscape,
  initialFocusRef,
  trap = true,
}: {
  active: boolean;
  containerRef: React.RefObject<HTMLElement>;
  onEscape: () => void;
  initialFocusRef?: React.RefObject<HTMLElement>;
  trap?: boolean;
}) {
  const restoreRef = React.useRef<HTMLElement | null>(null);
  const onEscapeRef = React.useRef(onEscape);
  React.useEffect(() => {
    onEscapeRef.current = onEscape;
  }, [onEscape]);

  React.useEffect(() => {
    if (!active) return;
    restoreRef.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const frame = requestAnimationFrame(() => {
      const container = containerRef.current;
      if (!container) return;
      (initialFocusRef?.current ?? focusableElements(container)[0] ?? container).focus();
    });
    const onKeyDown = (event: KeyboardEvent) => {
      const container = containerRef.current;
      if (!container) return;
      if (event.key === "Escape") {
        event.preventDefault();
        onEscapeRef.current();
        return;
      }
      if (!trap || event.key !== "Tab") return;
      const focusable = focusableElements(container);
      if (focusable.length === 0) {
        event.preventDefault();
        container.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("keydown", onKeyDown);
      const target = restoreRef.current;
      requestAnimationFrame(() => {
        if (target?.isConnected) target.focus();
      });
    };
  }, [active, containerRef, initialFocusRef, trap]);
}
