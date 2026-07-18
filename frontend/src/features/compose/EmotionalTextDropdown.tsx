import React from "react";
import { createPortal } from "react-dom";
import { useMenuFocus } from "../../components/primitives/Menu";
import { t } from "../../i18n";
import {
  emotionalTextOptions,
  toEmotionalText,
  type EmotionalTextStyle,
} from "../../utils/emotionalText";

const previewText = "Lorem ipsum dolor sit amet,";

export function EmotionalTextDropdown({
  open,
  onOpenChange,
  onSelect,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (style: EmotionalTextStyle) => void;
}) {
  const buttonRef = React.useRef<HTMLButtonElement>(null);
  const closeMenu = React.useCallback(
    () => onOpenChange(false),
    [onOpenChange],
  );
  const { menuRef, onMenuKeyDown } = useMenuFocus(open, closeMenu);
  const [position, setPosition] = React.useState<{
    left: number;
    top: number;
    width: number;
    maxHeight: number;
  } | null>(null);

  React.useLayoutEffect(() => {
    if (!open) {
      setPosition(null);
      return;
    }
    const updatePosition = () => {
      const button = buttonRef.current;
      if (!button) return;
      const rect = button.getBoundingClientRect();
      const width = Math.min(288, Math.max(0, window.innerWidth - 16));
      const maxHeight = Math.min(440, Math.max(0, window.innerHeight - 16));
      const below = rect.bottom + 4;
      setPosition({
        left: Math.max(
          8,
          Math.min(rect.left, window.innerWidth - width - 8),
        ),
        top:
          below + maxHeight <= window.innerHeight - 8
            ? below
            : Math.max(8, rect.top - maxHeight - 4),
        width,
        maxHeight,
      });
    };

    updatePosition();
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [open]);

  React.useEffect(() => {
    if (!open) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (
        buttonRef.current?.contains(target) ||
        menuRef.current?.contains(target)
      ) {
        return;
      }
      closeMenu();
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer, true);
    return () =>
      document.removeEventListener("pointerdown", closeOnOutsidePointer, true);
  }, [closeMenu, menuRef, open]);

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className={`btn btn-ghost btn-xs px-1.5 ${open ? "bg-surface1 text-text" : ""}`}
        title={t("Emotional Text")}
        aria-label={t("Emotional Text")}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => onOpenChange(!open)}
      >
        <span aria-hidden="true" className="text-[13px] leading-none">
          𝓐𝓪
        </span>
      </button>
      {open && position
        ? createPortal(
            <div
              ref={menuRef as React.RefObject<HTMLDivElement>}
              role="menu"
              aria-label={t("Emotional Text")}
              className="fixed z-50 overflow-y-auto rounded-md border border-surface0 bg-base-100 p-1.5 text-sm text-text shadow-xl"
              style={position}
              onKeyDown={onMenuKeyDown}
            >
              {emotionalTextOptions.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  role="menuitem"
                  className="block w-full rounded px-3 py-2 text-left hover:bg-surface0 focus:bg-surface0 focus:outline-none"
                  onClick={() => {
                    onSelect(option.value);
                    closeMenu();
                  }}
                >
                  <strong className="block text-xs">{option.label}</strong>
                  <span className="mt-0.5 block whitespace-nowrap text-xs text-subtext0">
                    {toEmotionalText(previewText, option.value)}
                  </span>
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
