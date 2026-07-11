import React from "react";
import { AlertTriangle } from "lucide-react";
import { useAppStore } from "../../store/appStore";
import { t } from "../../i18n";
import { Dialog } from "../primitives/Dialog";

export function ConfirmationDialog() {
  const dialog = useAppStore((state) => state.confirmationDialog);
  const resolveConfirmation = useAppStore(
    (state) => state.resolveConfirmation,
  );
  const cancelConfirmation = useAppStore(
    (state) => state.cancelConfirmation,
  );
  React.useEffect(() => {
    if (!dialog) return;
    const dialogId = dialog.id;
    return () => cancelConfirmation(dialogId);
  }, [cancelConfirmation, dialog]);

  return (
    <Dialog
      open={Boolean(dialog)}
      onClose={() => {
        if (dialog) resolveConfirmation(dialog.id, false);
      }}
      labelledBy={dialog ? `confirmation-title-${dialog.id}` : undefined}
      className="modal modal-open"
    >
      {dialog ? (
        <div className="modal-box max-w-md rounded-md border border-surface0 bg-base-100 p-0">
        <div className="flex items-start gap-3 border-b border-surface0 px-5 py-4">
          <div
            className={`mt-0.5 grid h-8 w-8 shrink-0 place-items-center rounded bg-base-200 ${dialog.danger ? "text-red" : "text-blue"}`}
          >
            <AlertTriangle className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h2
              id={`confirmation-title-${dialog.id}`}
              className="text-base font-semibold text-white"
            >
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
            onClick={() => resolveConfirmation(dialog.id, false)}
          >
            {t("Cancel")}
          </button>
          <button
            type="button"
            className={`btn btn-sm h-8 min-h-8 px-4 text-sm font-normal ${dialog.danger ? "btn-error" : "btn-primary"}`}
            onClick={() => resolveConfirmation(dialog.id, true)}
          >
            {dialog.confirmLabel}
          </button>
        </div>
        </div>
      ) : null}
      <div className="modal-backdrop">
        <button
          type="button"
          aria-label={t("Cancel")}
          onClick={() => {
            if (dialog) resolveConfirmation(dialog.id, false);
          }}
        />
      </div>
    </Dialog>
  );
}
