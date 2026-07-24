import React from "react";
import {
  AlertTriangle,
  Check,
  Clock3,
  Loader2,
  RefreshCw,
  SendHorizontal,
  X,
} from "lucide-react";

import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { ComposeOutboxItem } from "../../types/app";
import { formatTime } from "../../utils/format";
import { Dialog } from "../primitives/Dialog";

export function ComposeOutboxDialog() {
  const open = useAppStore((state) => state.composeOutboxOpen);
  const items = useAppStore((state) => state.composeOutboxItems);
  const load = useAppStore((state) => state.loadComposeOutbox);
  const retry = useAppStore((state) => state.retryComposeOutboxItem);
  const cancel = useAppStore((state) => state.cancelComposeOutboxItem);

  React.useEffect(() => {
    if (open) void load();
  }, [load, open]);

  const close = () => useAppStore.setState({ composeOutboxOpen: false });

  return (
    <Dialog
      open={open}
      onClose={close}
      labelledBy="compose-outbox-title"
      className="modal modal-open"
    >
      <section className="modal-box flex max-h-[80vh] max-w-2xl flex-col rounded-md border border-surface0 bg-base-100 p-0">
        <header className="flex shrink-0 items-center gap-3 border-b border-surface0 px-4 py-3">
          <SendHorizontal className="h-4 w-4 text-blue" />
          <h2
            id="compose-outbox-title"
            className="min-w-0 flex-1 text-base font-semibold text-text"
          >
            {t("Send queue")}
          </h2>
          <button
            type="button"
            className="btn btn-circle btn-ghost btn-xs"
            onClick={() => void load()}
            title={t("Refresh")}
            aria-label={t("Refresh")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            className="btn btn-circle btn-ghost btn-xs"
            onClick={close}
            title={t("Close")}
            aria-label={t("Close")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {items.length === 0 ? (
            <div className="grid min-h-40 place-items-center px-6 text-sm text-overlay0">
              {t("No queued posts or edits.")}
            </div>
          ) : (
            <ul className="divide-y divide-surface0">
              {items.map((item) => (
                <OutboxRow
                  key={item.id}
                  item={item}
                  onRetry={() => void retry(item.id)}
                  onCancel={() => void cancel(item.id)}
                />
              ))}
            </ul>
          )}
        </div>
      </section>
      <div className="modal-backdrop">
        <button type="button" aria-label={t("Close")} onClick={close} />
      </div>
    </Dialog>
  );
}

function OutboxRow({
  item,
  onRetry,
  onCancel,
}: {
  item: ComposeOutboxItem;
  onRetry: () => void;
  onCancel: () => void;
}) {
  const retryable = ["failed", "uncertain", "cancelled"].includes(item.state);
  const cancellable = ["queued", "retrying", "failed", "uncertain"].includes(
    item.state,
  );
  return (
    <li className="flex gap-3 px-4 py-3">
      <StateIcon state={item.state} />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs">
          <span className="font-medium text-text">
            {item.operationKind === "edit" ? t("Post edit") : t("Post")}
          </span>
          <span className="text-subtext0">{item.actingAccountAcct}</span>
          <span className="text-overlay0">{formatTime(item.createdAt)}</span>
        </div>
        <p className="mt-1 whitespace-pre-wrap break-words text-sm text-subtext1">
          {item.contentPreview}
        </p>
        <div className="mt-1 flex flex-wrap items-center gap-x-2 text-xs text-overlay0">
          <span>{stateLabel(item.state)}</span>
          {item.attempts > 0 ? (
            <span>{t("Attempt {count}", { count: item.attempts })}</span>
          ) : null}
          {item.state === "retrying" ? (
            <span>{formatTime(item.nextAttemptAt)}</span>
          ) : null}
        </div>
        {item.state === "uncertain" ? (
          <p className="mt-1 text-xs text-yellow">
            {t("Verify on the server before retrying.")}
          </p>
        ) : null}
        {item.lastError && item.state !== "uncertain" ? (
          <p className="mt-1 break-words text-xs text-red">
            {outboxErrorLabel(item.lastError)}
          </p>
        ) : null}
      </div>
      <div className="flex shrink-0 items-start gap-1">
        {retryable ? (
          <button
            type="button"
            className="btn btn-ghost btn-xs"
            onClick={onRetry}
            title={t("Retry queued item")}
            aria-label={t("Retry queued item")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </button>
        ) : null}
        {cancellable ? (
          <button
            type="button"
            className="btn btn-ghost btn-xs text-red"
            onClick={onCancel}
            title={t("Cancel queued item")}
            aria-label={t("Cancel queued item")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        ) : null}
      </div>
    </li>
  );
}

function StateIcon({ state }: { state: ComposeOutboxItem["state"] }) {
  if (state === "sending") {
    return <Loader2 className="mt-0.5 h-4 w-4 shrink-0 animate-spin text-blue" />;
  }
  if (state === "succeeded") {
    return <Check className="mt-0.5 h-4 w-4 shrink-0 text-green" />;
  }
  if (state === "failed" || state === "uncertain") {
    return <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-yellow" />;
  }
  return <Clock3 className="mt-0.5 h-4 w-4 shrink-0 text-overlay0" />;
}

function stateLabel(state: ComposeOutboxItem["state"]) {
  switch (state) {
    case "queued":
      return t("Queued");
    case "sending":
      return t("Sending");
    case "retrying":
      return t("Waiting to retry");
    case "failed":
      return t("Failed");
    case "uncertain":
      return t("Delivery uncertain");
    case "succeeded":
      return t("Sent");
    case "cancelled":
      return t("Cancelled");
  }
}

function outboxErrorLabel(messageKey: string) {
  switch (messageKey) {
    case "errors.authentication_expired":
      return t("Authentication expired. Please sign in again.");
    case "errors.rate_limited":
      return t("The server rate limit was reached. Please try again later.");
    case "errors.timeout":
      return t("The operation timed out. Please try again.");
    case "errors.validation":
      return t("The request is invalid. Please review the input.");
    case "errors.database_busy":
      return t("The database is busy. Please try again.");
    case "errors.capability_unsupported":
      return t("This operation is not supported by the account.");
    case "errors.cancelled":
      return t("The operation was cancelled.");
    default:
      return t("The operation failed. Please try again.");
  }
}
