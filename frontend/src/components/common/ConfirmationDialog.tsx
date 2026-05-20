import React from "react";
import { createPortal } from "react-dom";
import { AlertTriangle } from "lucide-react";
import { useAppStore } from "../../store/appStore";
import { t } from "../../i18n";

export function ConfirmationDialog() {
  const dialog = useAppStore((state) => state.confirmationDialog);
  const resolveConfirmation = useAppStore(
    (state) => state.resolveConfirmation,
  );
  const dialogRef = React.useRef<HTMLDialogElement>(null);

  React.useEffect(() => {
    if (!dialog || !dialogRef.current || dialogRef.current.open) return;
    dialogRef.current.showModal();
  }, [dialog]);

  if (!dialog) return null;

  return createPortal(
    <dialog
      ref={dialogRef}
      className="modal"
      onCancel={(event) => {
        event.preventDefault();
        resolveConfirmation(false);
      }}
    >
      <div className="modal-box max-w-md rounded-md border border-surface0 bg-base-100 p-0">
        <div className="flex items-start gap-3 border-b border-surface0 px-5 py-4">
          <div
            className={`mt-0.5 grid h-8 w-8 shrink-0 place-items-center rounded bg-base-200 ${dialog.danger ? "text-red" : "text-blue"}`}
          >
            <AlertTriangle className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h2 className="text-base font-semibold text-white">
              {dialog.title}
            </h2>
            <p className="mt-1 text-sm leading-5 text-subtext0">
              {dialog.message}
            </p>
          </div>
        </div>
        <div className="modal-action m-0 px-5 py-4">
          <button
            type="button"
            className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
            onClick={() => resolveConfirmation(false)}
          >
            {t("Cancel")}
          </button>
          <button
            type="button"
            className={`btn btn-sm h-8 min-h-8 px-4 text-sm font-normal ${dialog.danger ? "btn-error" : "btn-primary"}`}
            onClick={() => resolveConfirmation(true)}
          >
            {dialog.confirmLabel}
          </button>
        </div>
      </div>
      <form method="dialog" className="modal-backdrop">
        <button
          type="button"
          aria-label={t("Cancel")}
          onClick={() => resolveConfirmation(false)}
        />
      </form>
    </dialog>,
    document.body,
  );
}
