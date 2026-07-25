import { X } from "lucide-react";
import { t } from "../../i18n";
import type { TimelineStatus } from "../../types/app";
import { statusPlainText } from "../../utils/format";

export function ComposeTargetPreview({
  kind,
  status,
  onClose,
}: {
  kind: "reply" | "quote" | "edit";
  status: TimelineStatus;
  onClose: () => void;
}) {
  const label =
    kind === "reply" ? t("Reply") : kind === "quote" ? t("Quote") : t("Edit");
  const previewText = statusPlainText(status) || `(${t("Media").toLowerCase()})`;

  return (
    <div className="mb-1 flex h-6 min-h-6 max-w-full items-center gap-2 overflow-hidden rounded border border-surface0 bg-base-100 px-2 text-xs text-subtext0">
      <span className="shrink-0 font-semibold text-text">{label}</span>
      <span className="shrink-0 text-overlay1">{status.acct}</span>
      <span className="min-w-0 flex-1 truncate">{previewText}</span>
      <button
        type="button"
        className="grid h-5 w-5 shrink-0 place-items-center rounded text-overlay0 hover:bg-surface0 hover:text-text"
        title={t("Clear target post")}
        aria-label={t("Clear target post")}
        onClick={onClose}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}
