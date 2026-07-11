import React from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, Loader2 } from "lucide-react";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { AccountSummary } from "../../types/app";
import { Avatar } from "../../components/common/Avatar";
import { useMenuFocus } from "../../components/primitives/Menu";

export function AccountQuickSwitcher({
  accounts,
  activeAcct,
}: {
  accounts: AccountSummary[];
  activeAcct?: string | null;
}) {
  const switchAccount = useAppStore((state) => state.switchAccount);
  const [open, setOpen] = React.useState(false);
  const [position, setPosition] = React.useState<{
    top: number;
    left: number;
  } | null>(null);
  const [switchingAcct, setSwitchingAcct] = React.useState<string | null>(null);
  const buttonRef = React.useRef<HTMLButtonElement | null>(null);
  const active =
    accounts.find((account) => account.acct === activeAcct) ?? accounts[0];
  const canSwitch = accounts.length > 1;

  const updatePosition = React.useCallback(() => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (!rect) return;
    setPosition({
      top: Math.max(8, rect.top),
      left: Math.min(rect.right + 8, window.innerWidth - 312),
    });
  }, []);

  const openSwitcher = () => {
    if (!canSwitch) return;
    updatePosition();
    setOpen((current) => !current);
  };

  const chooseAccount = async (acct: string) => {
    if (acct === activeAcct) {
      setOpen(false);
      return;
    }
    setSwitchingAcct(acct);
    await switchAccount(acct);
    setSwitchingAcct(null);
    setOpen(false);
  };

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className="group relative h-9 w-9 rounded-md focus:outline-none focus:ring-1 focus:ring-blue disabled:cursor-default"
        onClick={openSwitcher}
        disabled={!canSwitch}
        title={canSwitch ? t("Switch account") : (active?.acct ?? t("Account"))}
        aria-haspopup={canSwitch ? "menu" : undefined}
        aria-expanded={canSwitch ? open : undefined}
        data-account-switcher-trigger
      >
        <Avatar
          src={active?.avatar}
          label={active?.displayName || active?.acct || "A"}
          size="lg"
        />
        {canSwitch ? (
          <span className="absolute -bottom-0.5 -right-0.5 grid h-4 w-4 place-items-center rounded-full border border-base bg-surface0 text-subtext0 group-hover:text-text">
            <ChevronDown className="h-3 w-3" />
          </span>
        ) : null}
      </button>
      {open && position ? (
        <AccountSwitcherPopover
          accounts={accounts}
          activeAcct={activeAcct ?? active?.acct ?? null}
          switchingAcct={switchingAcct}
          position={position}
          onSelect={(acct) => void chooseAccount(acct)}
          onClose={() => setOpen(false)}
          onReposition={updatePosition}
        />
      ) : null}
    </>
  );
}

function AccountSwitcherPopover({
  accounts,
  activeAcct,
  switchingAcct,
  position,
  onSelect,
  onClose,
  onReposition,
}: {
  accounts: AccountSummary[];
  activeAcct?: string | null;
  switchingAcct: string | null;
  position: { top: number; left: number };
  onSelect: (acct: string) => void;
  onClose: () => void;
  onReposition: () => void;
}) {
  const { menuRef, onMenuKeyDown } = useMenuFocus(true, onClose);
  React.useEffect(() => {
    const close = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (
        target?.closest("[data-account-switcher]") ||
        target?.closest("[data-account-switcher-trigger]")
      ) {
        return;
      }
      onClose();
    };
    const reposition = () => onReposition();

    document.addEventListener("pointerdown", close, true);
    window.addEventListener("scroll", reposition, true);
    window.addEventListener("resize", reposition);
    return () => {
      document.removeEventListener("pointerdown", close, true);
      window.removeEventListener("scroll", reposition, true);
      window.removeEventListener("resize", reposition);
    };
  }, [onClose, onReposition]);

  return createPortal(
    <div
      ref={menuRef as React.RefObject<HTMLDivElement>}
      className="fixed z-50 w-72 rounded-md border border-surface0 bg-base-100 p-1 text-sm text-text shadow-xl"
      style={{
        top: position.top,
        left: Math.max(8, position.left),
      }}
      data-account-switcher
      role="menu"
      onKeyDown={onMenuKeyDown}
    >
      {accounts.map((account) => {
        const selected = account.acct === activeAcct;
        const switching = account.acct === switchingAcct;
        return (
          <button
            key={account.acct}
            type="button"
            className={`flex w-full items-center gap-2 rounded px-2 py-2 text-left hover:bg-surface0 disabled:cursor-wait disabled:hover:bg-transparent ${selected ? "bg-surface0 text-text" : "text-subtext0"}`}
            onClick={() => onSelect(account.acct)}
            disabled={switchingAcct !== null}
            role="menuitemradio"
            aria-checked={selected}
          >
            <span className="grid h-4 w-4 shrink-0 place-items-center text-blue">
              {selected ? <Check className="h-4 w-4" /> : null}
            </span>
            <Avatar
              src={account.avatar}
              label={account.displayName || account.acct}
              size="md"
            />
            <span className="min-w-0 flex-1">
              <span className="block truncate font-semibold text-text">
                {account.displayName || account.acct}
              </span>
              <span className="block truncate text-xs text-subtext0">
                @{account.acct}
              </span>
            </span>
            {switching ? (
              <Loader2 className="h-4 w-4 shrink-0 animate-spin text-blue" />
            ) : null}
          </button>
        );
      })}
    </div>,
    document.body,
  );
}
