import React from "react";
import { Loader2 } from "lucide-react";
import type { ComposeMediaAttachment } from "../../types/app";
import { uniqueMediaSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";

export function ComposeAttachmentPreview({
  attachment,
}: {
  attachment: ComposeMediaAttachment;
}) {
  const sources = React.useMemo(
    () =>
      uniqueMediaSources([
        attachment.previewSrc,
        attachment.preview_url,
        attachment.url,
        attachment.remote_url,
      ]),
    [
      attachment.previewSrc,
      attachment.preview_url,
      attachment.remote_url,
      attachment.url,
    ],
  );
  const image = useRetriedMediaSource(sources);

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
