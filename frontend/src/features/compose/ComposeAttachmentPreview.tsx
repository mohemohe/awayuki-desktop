import React from "react";
import { FileAudio, FileQuestion, Loader2 } from "lucide-react";
import type { ComposeMediaAttachment } from "../../types/app";
import {
  isAnimatedImageMedia,
  isAudioMedia,
  isLikelyNonImageMediaUrl,
  isVideoMedia,
  mediaKind,
  uniqueMediaSources,
} from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";

export function composeVideoPoster(attachment: ComposeMediaAttachment) {
  const [preview] = uniqueMediaSources([attachment.preview_url]);
  if (!preview) return undefined;
  const originalSources = new Set(
    uniqueMediaSources([attachment.url, attachment.remote_url]),
  );
  return originalSources.has(preview) || isLikelyNonImageMediaUrl(preview)
    ? undefined
    : preview;
}

function composeVideoSources(attachment: ComposeMediaAttachment) {
  const poster = composeVideoPoster(attachment);
  return uniqueMediaSources([
    attachment.previewSrc !== poster ? attachment.previewSrc : null,
    attachment.url,
    attachment.remote_url,
  ]);
}

function releaseVideo(video: HTMLVideoElement) {
  video.pause();
  video.removeAttribute("src");
  video.load();
}

export function ComposeAttachmentPreview({
  attachment,
}: {
  attachment: ComposeMediaAttachment;
}) {
  const isVideo = isVideoMedia(attachment);
  const isAudio = isAudioMedia(attachment);
  const isImage = mediaKind(attachment) === "image";
  const isAnimatedImage = isAnimatedImageMedia(attachment);
  const sources = React.useMemo(
    () =>
      isImage
        ? isAnimatedImage
          ? uniqueMediaSources([
              attachment.url,
              attachment.remote_url,
              attachment.previewSrc,
              attachment.preview_url,
            ])
          : uniqueMediaSources([
              attachment.previewSrc,
              attachment.preview_url,
              attachment.url,
              attachment.remote_url,
            ])
        : [],
    [
      attachment.previewSrc,
      attachment.preview_url,
      attachment.remote_url,
      attachment.url,
      isAnimatedImage,
      isImage,
    ],
  );
  const image = useRetriedMediaSource(sources);
  const videoSources = React.useMemo(
    () => (isVideo ? composeVideoSources(attachment) : []),
    [attachment, isVideo],
  );
  const videoSource = videoSources[0];
  const videoPoster = React.useMemo(
    () => (isVideo ? composeVideoPoster(attachment) : undefined),
    [attachment, isVideo],
  );
  const videoRef = React.useRef<HTMLVideoElement | null>(null);
  const setVideoRef = React.useCallback((video: HTMLVideoElement | null) => {
    const previous = videoRef.current;
    if (previous && previous !== video) releaseVideo(previous);
    videoRef.current = video;
  }, []);

  if (isVideo) {
    return videoSource ? (
      <video
        ref={setVideoRef}
        key={videoSource}
        src={videoSource}
        poster={videoPoster}
        aria-label={attachment.filename}
        className="h-full w-full object-cover"
        draggable={false}
        muted
        playsInline
        preload="metadata"
      />
    ) : (
      <AttachmentPlaceholder filename={attachment.filename} />
    );
  }

  if (isAudio) {
    return (
      <AttachmentPlaceholder
        filename={attachment.filename}
        icon={<FileAudio className="h-4 w-4" />}
      />
    );
  }

  if (!isImage) {
    return (
      <AttachmentPlaceholder
        filename={attachment.filename}
        icon={<FileQuestion className="h-4 w-4" />}
      />
    );
  }

  return (
    <>
      {image.src && !image.failed ? (
        <img
          key={image.key}
          src={image.src}
          alt={attachment.filename}
          className={`h-full w-full object-cover ${image.loaded ? "" : "opacity-0"}`}
          draggable={false}
          onLoad={image.onLoad}
          onError={image.onError}
        />
      ) : null}
      {!image.loaded ? (
        <div className="absolute inset-0 grid place-items-center px-1 text-center text-[10px] text-subtext0">
          {image.retrying ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <span className="line-clamp-2 break-anywhere">
              {attachment.filename}
            </span>
          )}
        </div>
      ) : null}
    </>
  );
}

function AttachmentPlaceholder({
  filename,
  icon,
}: {
  filename: string;
  icon?: React.ReactNode;
}) {
  return (
    <div className="absolute inset-0 grid place-items-center px-1 text-center text-[10px] text-subtext0">
      <span className="grid min-w-0 place-items-center gap-0.5">
        {icon}
        <span className="line-clamp-2 break-anywhere">{filename}</span>
      </span>
    </div>
  );
}
