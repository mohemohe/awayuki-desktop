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

  const moveDragTo = (index: number) => {
    const from = dragIndexRef.current;
    if (from === null || from === index) return;
    onMove(from, index);
    dragIndexRef.current = index;
    setDragIndex(index);
  };

  return (
    <div
      className="mt-1 flex h-16 items-center gap-1 overflow-x-auto"
      onMouseLeave={finishDrag}
      onMouseMove={(event) => {
        if (dragIndexRef.current === null) return;
        const cards = Array.from(
          event.currentTarget.querySelectorAll<HTMLElement>(
            "[data-compose-media-index]",
          ),
        );
        let closestIndex = 0;
        let closestDistance = Number.POSITIVE_INFINITY;
        cards.forEach((card, index) => {
          const rect = card.getBoundingClientRect();
          const distance = Math.abs(
            event.clientX - (rect.left + rect.width / 2),
          );
          if (distance < closestDistance) {
            closestIndex = index;
            closestDistance = distance;
          }
        });
        moveDragTo(closestIndex);
      }}
      onMouseUp={finishDrag}
    >
      {attachments.map((attachment, index) => (
        <div
          key={`${attachment.id}-${attachment.filename}`}
          data-compose-media-index={index}
          className={`group relative h-14 w-20 shrink-0 overflow-hidden rounded border bg-base-100 ${
            dragIndex === index ? "border-blue shadow-lg" : "border-surface0"
          }`}
        >
          <ComposeAttachmentPreview attachment={attachment} />
          <button
            type="button"
            className="absolute inset-0 z-10 cursor-grab touch-none select-none active:cursor-grabbing"
            aria-label={`${t("Reorder media")}: ${attachment.filename}`}
            title={t("Reorder media")}
            onMouseDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              dragIndexRef.current = index;
              setDragIndex(index);
            }}
            onMouseEnter={() => {
              moveDragTo(index);
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
          >
            <GripVertical className="absolute left-1 top-1 h-3 w-3 rounded-br bg-crust/80 text-subtext0" />
          </button>
          <button
            type="button"
            className="absolute right-0 top-0 z-20 grid h-5 w-5 place-items-center rounded-bl bg-crust/85 text-text"
            onClick={() => onRemove(index)}
            title={t("Remove media")}
          >
            <X className="h-3 w-3" />
          </button>
          {attachment.uploading ? (
            <div className="pointer-events-none absolute inset-0 z-30 grid place-items-center bg-crust/60 text-[10px] text-blue">
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
