import React from "react";
import { createPortal } from "react-dom";
import {
  Download,
  ExternalLink,
  Loader2,
  Maximize2,
  MessageCircle,
  Minimize2,
  Repeat2,
  Star,
  X,
} from "lucide-react";
import { invokeCommand } from "../../api/tauri";
import { statusIdentity, useAppStore } from "../../store/appStore";
import type { MediaPreviewState, TimelineStatus } from "../../types/app";
import { openExternalUrl } from "../../utils/browser";
import { confirmStatusAction } from "../../utils/confirmation";
import { clamp, computeMediaFitScale, filenameFromUrl } from "../../utils/format";
import { previewMediaSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";
import { t } from "../../i18n";

export function MediaPreviewOverlay({
  preview,
}: {
  preview: MediaPreviewState;
}) {
  const closeMediaPreview = useAppStore((state) => state.closeMediaPreview);
  const requestConfirmation = useAppStore(
    (state) => state.requestConfirmation,
  );
  const confirmationSettings = useAppStore(
    (state) => state.snapshot?.settings.confirmation,
  );
  const mediaSourcePreference = useAppStore(
    (state) => state.snapshot?.settings.confirmation.media_source ?? "Local",
  );
  const [naturalSize, setNaturalSize] = React.useState({ width: 0, height: 0 });
  const [scale, setScale] = React.useState(1);
  const [panOffset, setPanOffset] = React.useState({ x: 0, y: 0 });
  const dragRef = React.useRef<{
    pointerId: number;
    lastX: number;
    lastY: number;
  } | null>(null);
  const isVideo =
    preview.media.media_type?.startsWith("video") ||
    preview.media.type?.startsWith("video");
  const mediaSources = React.useMemo(() => {
    const sources = previewMediaSources(
      preview.media,
      isVideo,
      mediaSourcePreference,
    );
    return sources.length ? sources : [preview.src];
  }, [
    isVideo,
    mediaSourcePreference,
    preview.media.preview_url,
    preview.media.remote_url,
    preview.media.url,
    preview.src,
  ]);
  const mediaSource = useRetriedMediaSource(mediaSources);
  const fitScale = React.useMemo(() => {
    if (!naturalSize.width || !naturalSize.height) return 1;
    return computeMediaFitScale(naturalSize.width, naturalSize.height);
  }, [naturalSize.height, naturalSize.width]);
  const resetScale = fitScale < 1 ? fitScale : 1;
  const resetDisabled = Math.abs(scale - resetScale) < 0.005;
  const resetIcon =
    scale > resetScale ? (
      <Minimize2 className="h-4 w-4" />
    ) : (
      <Maximize2 className="h-4 w-4" />
    );

  React.useEffect(() => {
    setNaturalSize({ width: 0, height: 0 });
    setScale(1);
    setPanOffset({ x: 0, y: 0 });
    dragRef.current = null;
  }, [preview.src]);

  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMediaPreview();
    };
    const onResize = () => {
      if (!naturalSize.width || !naturalSize.height) return;
      setScale(computeMediaFitScale(naturalSize.width, naturalSize.height));
      setPanOffset({ x: 0, y: 0 });
    };
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onResize);
    };
  }, [closeMediaPreview, naturalSize.height, naturalSize.width]);

  const resetZoom = () => {
    setScale(resetScale);
    setPanOffset({ x: 0, y: 0 });
  };
  const zoomBy = (delta: number) =>
    setScale((current) => clamp(current * delta, 0.1, 5));
  const startPan = (event: React.PointerEvent<HTMLDivElement>) => {
    if (isVideo || event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      lastX: event.clientX,
      lastY: event.clientY,
    };
  };
  const movePan = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    const dx = event.clientX - drag.lastX;
    const dy = event.clientY - drag.lastY;
    drag.lastX = event.clientX;
    drag.lastY = event.clientY;
    setPanOffset((current) => ({
      x: current.x + dx,
      y: current.y + dy,
    }));
  };
  const endPan = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    dragRef.current = null;
  };
  const mediaUrl = mediaSources[0] ?? preview.src;
  const suggestedFilename =
    preview.media.description ||
    filenameFromUrl(mediaUrl) ||
    preview.media.id ||
    "media";
  const runStatusAction = async (action: string) => {
    try {
      const confirmed = await confirmStatusAction(
        confirmationSettings,
        requestConfirmation,
        preview.status,
        action,
      );
      if (!confirmed) return;
      closeMediaPreview();
      const updated = await invokeCommand<TimelineStatus>("status_action", {
        request: { statusId: preview.status.originalStatusId, action },
      });
      useAppStore.setState((state) => ({
        timelines: Object.fromEntries(
          Object.entries(state.timelines).map(([id, statuses]) => [
            id,
            statuses.map((item) =>
              statusIdentity(item) === statusIdentity(preview.status)
                ? updated
                : item,
            ),
          ]),
        ),
      }));
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    }
  };
  const download = async () => {
    try {
      await invokeCommand("download_media", {
        request: { url: mediaUrl, suggestedFilename },
      });
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    }
  };
  const reply = () => {
    closeMediaPreview();
    useAppStore.getState().replyStatus(preview.status);
  };
  const openMediaInBrowser = () => {
    closeMediaPreview();
    void openExternalUrl(mediaUrl);
  };
  return createPortal(
    <div
      className="fixed inset-0 z-[10000] text-text"
      style={{ backgroundColor: "rgba(0, 0, 0, 0.72)" }}
      onClick={closeMediaPreview}
      onWheel={(event) => {
        event.preventDefault();
        zoomBy(event.deltaY < 0 ? 1.1 : 0.9);
      }}
    >
      <div className="pointer-events-none absolute left-7 top-7 z-20 text-xs font-semibold text-text">
        <div>
          {mediaSource.failed
            ? t("Media failed to load")
            : mediaSource.retrying
              ? t("Retrying media")
              : naturalSize.width && naturalSize.height
                ? `${naturalSize.width} x ${naturalSize.height} px`
                : t("Loading media")}
        </div>
        <div>{Math.round(scale * 100)}%</div>
      </div>
      <div className="absolute right-4 top-4 z-20 flex items-center gap-2">
        <button
          className="btn btn-circle btn-ghost btn-sm bg-crust/80 text-text hover:bg-surface1/80 disabled:bg-crust/55 disabled:text-overlay0"
          disabled={resetDisabled}
          onClick={(event) => {
            event.stopPropagation();
            resetZoom();
          }}
          title={t("Reset zoom")}
        >
          {resetIcon}
        </button>
        <button
          className="btn btn-circle btn-ghost btn-sm bg-crust/80 text-text hover:bg-surface1/80"
          onClick={(event) => {
            event.stopPropagation();
            closeMediaPreview();
          }}
          title={t("Close")}
        >
          <X className="h-4 w-4" />
        </button>
      </div>
      <div className="relative z-0 grid h-full place-items-center px-12 py-16">
        <div
          className={`max-h-full max-w-full ${isVideo ? "" : "cursor-grab active:cursor-grabbing"}`}
          onClick={(event) => event.stopPropagation()}
          onMouseDown={(event) => {
            if (event.button === 1) {
              event.preventDefault();
              resetZoom();
            }
          }}
          onPointerDown={startPan}
          onPointerMove={movePan}
          onPointerUp={endPan}
          onPointerCancel={endPan}
          style={
            isVideo
              ? undefined
              : {
                  transform: `translate(${panOffset.x}px, ${panOffset.y}px)`,
                }
          }
        >
          {isVideo ? (
            <video
              key={mediaSource.key}
              src={mediaSource.src ?? undefined}
              className={`max-h-[calc(100vh-8rem)] max-w-[calc(100vw-6rem)] ${mediaSource.loaded ? "" : "opacity-0"}`}
              controls
              autoPlay
              onError={mediaSource.onError}
              onLoadedMetadata={(event) => {
                mediaSource.onLoad();
                const video = event.currentTarget;
                const nextSize = {
                  width: video.videoWidth,
                  height: video.videoHeight,
                };
                setNaturalSize(nextSize);
                setScale(computeMediaFitScale(nextSize.width, nextSize.height));
                setPanOffset({ x: 0, y: 0 });
              }}
            />
          ) : (
            <>
              {!mediaSource.loaded ? (
                <div className="grid min-h-40 min-w-60 place-items-center text-overlay0">
                  {mediaSource.failed ? null : (
                    <Loader2 className="h-5 w-5 animate-spin" />
                  )}
                </div>
              ) : null}
              {mediaSource.src ? (
                <img
                  key={mediaSource.key}
                  src={mediaSource.src}
                  alt={preview.media.description ?? ""}
                  className={`max-w-none select-none ${mediaSource.loaded ? "" : "opacity-0"}`}
                  style={{
                    width: naturalSize.width
                      ? `${naturalSize.width * scale}px`
                      : undefined,
                    height: naturalSize.height
                      ? `${naturalSize.height * scale}px`
                      : undefined,
                  }}
                  draggable={false}
                  onError={mediaSource.onError}
                  onLoad={(event) => {
                    mediaSource.onLoad();
                    const image = event.currentTarget;
                    const nextSize = {
                      width: image.naturalWidth,
                      height: image.naturalHeight,
                    };
                    setNaturalSize(nextSize);
                    setScale(
                      computeMediaFitScale(nextSize.width, nextSize.height),
                    );
                  }}
                />
              ) : null}
            </>
          )}
        </div>
      </div>
      <div
        className="absolute bottom-8 left-1/2 z-20 flex -translate-x-1/2 items-center gap-5 rounded-full bg-crust/65 px-5 py-2 text-subtext0"
        onClick={(event) => event.stopPropagation()}
      >
        <button className="hover:text-text" title={t("Reply")} onClick={reply}>
          <MessageCircle className="h-4 w-4" />
        </button>
        <button
          className="hover:text-text"
          title={t("Boost")}
          onClick={() =>
            void runStatusAction(
              preview.status.reblogged ? "unreblog" : "reblog",
            )
          }
        >
          <Repeat2 className="h-4 w-4" />
        </button>
        <button
          className="hover:text-text"
          title={t("Favorite")}
          onClick={() =>
            void runStatusAction(
              preview.status.favourited ? "unfavourite" : "favourite",
            )
          }
        >
          <Star className="h-4 w-4" />
        </button>
        <button
          className="hover:text-text"
          title={t("Download")}
          onClick={() => void download()}
        >
          <Download className="h-4 w-4" />
        </button>
        <button
          className="hover:text-text"
          title={t("Open in browser")}
          onClick={openMediaInBrowser}
        >
          <ExternalLink className="h-4 w-4" />
        </button>
      </div>
    </div>,
    document.body,
  );
}
