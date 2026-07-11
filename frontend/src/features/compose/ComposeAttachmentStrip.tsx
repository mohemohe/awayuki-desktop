import React from "react";
import { GripVertical, Loader2, X } from "lucide-react";
import { t } from "../../i18n";
import type { ComposeMediaAttachment } from "../../types/app";
import { ComposeAttachmentPreview } from "./ComposeAttachmentPreview";

export function ComposeAttachmentStrip({
  attachments,
  onMove,
  onRemove,
}: {
  attachments: ComposeMediaAttachment[];
  onMove: (from: number, to: number, announce?: boolean) => void;
  onRemove: (index: number) => void;
}) {
  const dragIndexRef = React.useRef<number | null>(null);
  const [dragIndex, setDragIndex] = React.useState<number | null>(null);

  if (attachments.length === 0) return null;

  const finishDrag = () => {
    dragIndexRef.current = null;
    setDragIndex(null);
  };

  return (
    <div className="mt-1 flex h-16 items-center gap-1 overflow-x-auto">
      {attachments.map((attachment, index) => (
        <div
          key={`${attachment.id}-${attachment.filename}`}
          className={`group relative h-14 w-20 shrink-0 overflow-hidden rounded border bg-base-100 ${dragIndex !== null && dragIndex !== index ? "border-blue/70" : "border-surface0"}`}
          onDragOver={(event) => {
            event.preventDefault();
            event.stopPropagation();
            event.dataTransfer.dropEffect = "move";
          }}
          onDrop={(event) => {
            event.preventDefault();
            event.stopPropagation();
            const fromText = event.dataTransfer.getData(
              "application/x-awayuki-media-index",
            );
            const from =
              dragIndexRef.current ??
              (fromText ? Number(fromText) : Number.NaN);
            if (Number.isFinite(from)) onMove(from, index);
            finishDrag();
          }}
          onDragEnd={finishDrag}
        >
          <ComposeAttachmentPreview attachment={attachment} />
          <button
            type="button"
            className="absolute left-0 top-0 grid h-5 w-5 cursor-grab place-items-center rounded-br bg-crust/80 text-subtext0 active:cursor-grabbing"
            draggable
            aria-label={`${t("Reorder media")}: ${attachment.filename}`}
            onDragStart={(event) => {
              event.stopPropagation();
              dragIndexRef.current = index;
              setDragIndex(index);
              event.dataTransfer.effectAllowed = "move";
              event.dataTransfer.setData(
                "application/x-awayuki-media-index",
                String(index),
              );
            }}
            onKeyDown={(event) => {
              const to =
                event.key === "ArrowLeft"
                  ? Math.max(0, index - 1)
                  : event.key === "ArrowRight"
                    ? Math.min(attachments.length - 1, index + 1)
                    : event.key === "Home"
                      ? 0
                      : event.key === "End"
                        ? attachments.length - 1
                        : index;
              if (to === index) return;
              event.preventDefault();
              onMove(index, to, true);
            }}
            title={t("Reorder media")}
          >
            <GripVertical className="h-3 w-3" />
          </button>
          <button
            className="absolute right-0 top-0 grid h-5 w-5 place-items-center rounded-bl bg-crust/85 text-text"
            onClick={() => onRemove(index)}
            title={t("Remove media")}
          >
            <X className="h-3 w-3" />
          </button>
          {attachment.uploading ? (
            <div className="absolute inset-0 grid place-items-center bg-crust/60 text-[10px] text-blue">
              {attachment.uploadProgress !== undefined ? (
                `${Math.round(attachment.uploadProgress * 100)}%`
              ) : (
                <Loader2 className="h-4 w-4 animate-spin text-blue" />
              )}
            </div>
          ) : null}
        </div>
      ))}
    </div>
  );
}
