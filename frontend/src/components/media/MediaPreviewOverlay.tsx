import React from "react";
import { listen } from "@tauri-apps/api/event";
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
import {
  hasTauriRuntime,
  invokeTypedCommand,
  invokeTypedCommandWithOperationId,
} from "../../api/tauri";
import { IpcAppError } from "../../api/ipcErrors";
import { useAppStore } from "../../store/appStore";
import type {
  MediaDownloadProgressEvent,
  MediaPreviewState,
} from "../../types/app";
import { openExternalUrl } from "../../utils/browser";
import { clamp, computeMediaFitScale, filenameFromUrl } from "../../utils/format";
import {
  isAudioMedia,
  isVideoMedia,
  mediaKind,
  previewMediaSources,
  thumbnailMediaSources,
} from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";
import { t } from "../../i18n";
import { Dialog } from "../primitives/Dialog";

export function releaseMediaElement(media: HTMLMediaElement) {
  media.pause();
  media.removeAttribute("src");
  // Removing src alone does not run the media resource selection algorithm.
  // load() deterministically drops WebKit's decoder and buffered media state.
  media.load();
}

export function releaseVideoElement(video: HTMLVideoElement) {
  releaseMediaElement(video);
}

export function MediaPreviewOverlay({
  preview,
}: {
  preview: MediaPreviewState;
}) {
  const closeMediaPreview = useAppStore((state) => state.closeMediaPreview);
  const actionStatus = useAppStore((state) => state.actionStatus);
  const mediaSourcePreference = useAppStore(
    (state) => state.snapshot?.settings.confirmation.media_source ?? "Local",
  );
  const [naturalSize, setNaturalSize] = React.useState({ width: 0, height: 0 });
  const [scale, setScale] = React.useState(1);
  const [panOffset, setPanOffset] = React.useState({ x: 0, y: 0 });
  const [downloadProgress, setDownloadProgress] = React.useState<
    MediaDownloadProgressEvent | undefined
  >();
  const downloadOperationRef = React.useRef<string | null>(null);
  const mediaElementRef = React.useRef<HTMLMediaElement | null>(null);
  const dragRef = React.useRef<{
    pointerId: number;
    lastX: number;
    lastY: number;
  } | null>(null);
  const isVideo = isVideoMedia(preview.media);
  const isAudio = isAudioMedia(preview.media);
  const isPlayableMedia = isVideo || isAudio;
  const isUnknownMedia = mediaKind(preview.media) === "unknown";
  const setMediaElementRef = React.useCallback(
    (media: HTMLMediaElement | null) => {
      const previous = mediaElementRef.current;
      if (previous && previous !== media) releaseMediaElement(previous);
      mediaElementRef.current = media;
    },
    [],
  );
  const mediaSources = React.useMemo(() => {
    const sources = isUnknownMedia
      ? thumbnailMediaSources(preview.media, mediaSourcePreference)
      : previewMediaSources(
          preview.media,
          isPlayableMedia,
          mediaSourcePreference,
        );
    if (sources.length) return sources;
    return isUnknownMedia ? [] : [preview.src];
  }, [
    isPlayableMedia,
    isUnknownMedia,
    mediaSourcePreference,
    preview.media,
    preview.src,
  ]);
  const actionMediaSources = React.useMemo(
    () =>
      previewMediaSources(
        preview.media,
        isPlayableMedia,
        mediaSourcePreference,
      ),
    [isPlayableMedia, mediaSourcePreference, preview.media],
  );
  // Playable media errors must not enter the image retry coordinator: that
  // probe uses new Image(), which would send MP4/MP3 originals to WebKit's
  // image decoder.
  const mediaSource = useRetriedMediaSource(
    isPlayableMedia ? [] : mediaSources,
  );
  const playableSignature = mediaSources.join("\n");
  const [playableState, setPlayableState] = React.useState({
    sourceIndex: 0,
    loaded: false,
    failed: false,
  });
  React.useEffect(() => {
    setPlayableState({ sourceIndex: 0, loaded: false, failed: false });
  }, [playableSignature]);
  const playableSource = mediaSources[playableState.sourceIndex] ?? null;
  const onPlayableError = React.useCallback(() => {
    setPlayableState((current) =>
      current.sourceIndex + 1 < mediaSources.length
        ? {
            sourceIndex: current.sourceIndex + 1,
            loaded: false,
            failed: false,
          }
        : { ...current, loaded: false, failed: true },
    );
  }, [mediaSources.length]);
  const onPlayableLoad = React.useCallback(() => {
    setPlayableState((current) => ({
      ...current,
      loaded: true,
      failed: false,
    }));
  }, []);
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
    const onResize = () => {
      if (!naturalSize.width || !naturalSize.height) return;
      setScale(computeMediaFitScale(naturalSize.width, naturalSize.height));
      setPanOffset({ x: 0, y: 0 });
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
    };
  }, [closeMediaPreview, naturalSize.height, naturalSize.width]);

  React.useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (hasTauriRuntime()) {
      void listen<MediaDownloadProgressEvent>(
        "media-download-progress",
        (event) => {
          if (event.payload.operationId !== downloadOperationRef.current) return;
          setDownloadProgress(event.payload);
        },
      ).then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      });
    }
    return () => {
      disposed = true;
      unlisten?.();
      const targetOperationId = downloadOperationRef.current;
      if (targetOperationId) {
        void invokeTypedCommand("cancel_media_download", {
          request: { targetOperationId },
        });
      }
    };
  }, []);

  const resetZoom = () => {
    setScale(resetScale);
    setPanOffset({ x: 0, y: 0 });
  };
  const zoomBy = (delta: number) =>
    setScale((current) => clamp(current * delta, 0.1, 5));
  const startPan = (event: React.PointerEvent<HTMLDivElement>) => {
    if (isPlayableMedia || event.button !== 0) return;
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
  const mediaUrl = actionMediaSources[0] ?? preview.src;
  const suggestedFilename =
    preview.media.description ||
    filenameFromUrl(mediaUrl) ||
    preview.media.id ||
    "media";
  const runStatusAction = async (action: string) => {
    closeMediaPreview();
    await actionStatus(preview.status, action, true);
  };
  const download = async () => {
    const activeOperation = downloadOperationRef.current;
    if (activeOperation) {
      await invokeTypedCommand("cancel_media_download", {
        request: { targetOperationId: activeOperation },
      });
      return;
    }
    const operationId = crypto.randomUUID();
    downloadOperationRef.current = operationId;
    setDownloadProgress({
      operationId,
      phase: "selecting",
      downloadedBytes: 0,
    });
    try {
      await invokeTypedCommandWithOperationId(
        "download_media",
        { request: { url: mediaUrl, suggestedFilename } },
        operationId,
      );
    } catch (error) {
      if (!(error instanceof IpcAppError && error.code === "cancelled")) {
        useAppStore.setState({ error: String(error) });
      }
    } finally {
      if (downloadOperationRef.current === operationId) {
        downloadOperationRef.current = null;
        setDownloadProgress(undefined);
      }
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
  return (
    <Dialog
      open
      onClose={closeMediaPreview}
      label={t("Open media preview")}
      className="media-preview-chrome fixed inset-0 z-[10000] text-text"
      style={{ backgroundColor: "rgba(0, 0, 0, 0.72)" }}
      onClick={closeMediaPreview}
      onWheel={(event) => {
        event.preventDefault();
        zoomBy(event.deltaY < 0 ? 1.1 : 0.9);
      }}
    >
      <div className="pointer-events-none absolute left-7 top-7 z-20 text-xs font-semibold text-text">
        <div>
          {isAudio
            ? suggestedFilename
            : isVideo
              ? playableState.failed
                ? t("Media failed to load")
                : naturalSize.width && naturalSize.height
                  ? `${naturalSize.width} x ${naturalSize.height} px`
                  : t("Loading media")
              : mediaSource.failed
                ? t("Media failed to load")
                : mediaSource.retrying
                  ? t("Retrying media")
                  : naturalSize.width && naturalSize.height
                    ? `${naturalSize.width} x ${naturalSize.height} px`
                    : t("Loading media")}
        </div>
        {isAudio ? null : <div>{Math.round(scale * 100)}%</div>}
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
          className={`max-h-full max-w-full ${isPlayableMedia ? "" : "cursor-grab active:cursor-grabbing"}`}
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
            isPlayableMedia
              ? undefined
              : {
                  transform: `translate(${panOffset.x}px, ${panOffset.y}px)`,
                }
          }
        >
          {isAudio ? (
            <audio
              ref={setMediaElementRef}
              key={playableSource}
              src={playableSource ?? undefined}
              aria-label={preview.media.description ?? suggestedFilename}
              className="w-[min(36rem,calc(100vw-6rem))]"
              controls
              preload="metadata"
              onError={onPlayableError}
              onLoadedMetadata={onPlayableLoad}
            />
          ) : isVideo ? (
            <video
              ref={setMediaElementRef}
              key={playableSource}
              src={playableSource ?? undefined}
              aria-label={preview.media.description ?? suggestedFilename}
              className={`max-h-[calc(100vh-8rem)] max-w-[calc(100vw-6rem)] ${playableState.loaded ? "" : "opacity-0"}`}
              controls
              autoPlay
              playsInline
              preload="metadata"
              onError={onPlayableError}
              onLoadedMetadata={(event) => {
                onPlayableLoad();
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
          title={t(downloadOperationRef.current ? "Cancel" : "Download")}
          onClick={() => void download()}
        >
          {downloadOperationRef.current ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Download className="h-4 w-4" />
          )}
        </button>
        {downloadProgress ? (
          <span className="min-w-20 text-xs tabular-nums text-subtext1">
            {downloadProgress.totalBytes
              ? `${Math.min(100, Math.round((downloadProgress.downloadedBytes / downloadProgress.totalBytes) * 100))}%`
              : downloadProgress.phase === "selecting"
                ? t("Select destination")
                : `${(downloadProgress.downloadedBytes / (1024 * 1024)).toFixed(1)} MiB`}
          </span>
        ) : null}
        <button
          className="hover:text-text"
          title={t("Open in browser")}
          onClick={openMediaInBrowser}
        >
          <ExternalLink className="h-4 w-4" />
        </button>
      </div>
    </Dialog>
  );
}
