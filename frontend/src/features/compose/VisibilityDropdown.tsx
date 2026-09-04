import React from "react";
import { ChevronDown } from "lucide-react";
import { t, type MessageId } from "../../i18n";
import type { AppStore } from "../../store/appStore";
import { useMenuFocus } from "../../components/primitives/Menu";

const visibilityOptions: Array<{
  value: AppStore["visibility"];
  label: MessageId;
}> = [
  { value: "public", label: "Public" },
  { value: "unlisted", label: "Unlisted" },
  { value: "private", label: "Private" },
  { value: "direct", label: "Direct" },
];

export function VisibilityDropdown({
  value,
  autoApplied = false,
  disabled = false,
  onChange,
}: {
  value: AppStore["visibility"];
  autoApplied?: boolean;
  disabled?: boolean;
  onChange: (value: AppStore["visibility"]) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const closeMenu = React.useCallback(() => setOpen(false), []);
  const menuOpen = open && !disabled;
  const { menuRef, onMenuKeyDown } = useMenuFocus(menuOpen, closeMenu);
  const selected =
    visibilityOptions.find((option) => option.value === value) ??
    visibilityOptions[0];

  return (
    <div
      className={`dropdown dropdown-bottom shrink-0 ${menuOpen ? "dropdown-open" : "dropdown-close"}`}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        className={`btn btn-outline btn-xs min-w-20 justify-between bg-base-100 px-2 font-normal text-text hover:border-surface1 hover:bg-surface0 hover:text-text ${autoApplied ? "border-blue" : "border-surface0"}`}
        title={autoApplied ? t("Auto visibility applied") : undefined}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        {t(selected.label)}
        <ChevronDown className="h-3 w-3 text-subtext0" />
      </button>
      {menuOpen ? (
        <ul
          ref={menuRef as React.RefObject<HTMLUListElement>}
          tabIndex={-1}
          className="dropdown-content menu z-50 w-36 rounded-box border border-surface0 bg-base-100 p-1 shadow"
          role="menu"
          onKeyDown={onMenuKeyDown}
        >
          {visibilityOptions.map((option) => (
            <li key={option.value}>
              <button
                type="button"
                className={option.value === value ? "active" : ""}
                role="menuitemradio"
                aria-checked={option.value === value}
                onClick={() => {
                  onChange(option.value);
                  setOpen(false);
                }}
              >
                {t(option.label)}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
