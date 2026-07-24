import React from "react";
import { Eye, EyeOff, Loader2 } from "lucide-react";
import { t } from "../../i18n";
import type { TimelineStatus } from "../../types/app";
import { blurHashToDataUrl } from "../../utils/blurhash";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";

export function MediaThumbnail({
  media,
  sources,
  sensitive,
  visible,
  onToggle,
  onOpen,
}: {
  media: TimelineStatus["media"][number];
  sources: string[];
  sensitive: boolean;
  visible: boolean;
  onToggle: () => void;
  onOpen: () => void;
}) {
  const image = useRetriedMediaSource(sources);
  const blurhashUrl = React.useMemo(
    () => (media.blurhash ? blurHashToDataUrl(media.blurhash) : null),
    [media.blurhash],
  );
  const shouldHide = sensitive && !visible;
  const placeholderStyle = blurhashUrl
    ? { backgroundImage: `url(${blurhashUrl})` }
    : undefined;

  return (
    <div className="relative h-28 w-full overflow-hidden rounded border border-transparent bg-base-200 hover:border-blue">
      <button
        type="button"
        className="h-full w-full"
        onClick={shouldHide ? onToggle : onOpen}
        title={shouldHide ? t("Reveal media") : t("Open media preview")}
      >
        {shouldHide ? (
          <div
            className="h-full w-full bg-surface0 bg-cover bg-center"
            style={placeholderStyle}
          />
        ) : (
          <>
            {!image.loaded ? (
              <div
                className="absolute inset-0 grid place-items-center bg-surface0 bg-cover bg-center text-xs text-overlay0"
                style={placeholderStyle}
              >
                {image.retrying ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : image.failed ? (
                  <span>{t("Media unavailable")}</span>
                ) : null}
              </div>
            ) : null}
            {image.src ? (
              <img
                key={image.key}
                src={image.src}
                alt={media.description ?? ""}
                className={`h-full w-full object-cover ${image.loaded ? "" : "opacity-0"}`}
                onLoad={image.onLoad}
                onError={image.onError}
              />
            ) : null}
          </>
        )}
      </button>
      {sensitive ? (
        <button
          type="button"
          className="absolute left-1 top-1 grid h-7 w-7 place-items-center rounded bg-base/80 text-text shadow hover:bg-base"
          onClick={onToggle}
          title={shouldHide ? t("Reveal media") : t("Hide media")}
        >
          {shouldHide ? (
            <Eye className="h-3.5 w-3.5" />
          ) : (
            <EyeOff className="h-3.5 w-3.5" />
          )}
        </button>
      ) : null}
    </div>
  );
}

export function statusDisplayCreatedAt(status: TimelineStatus) {
  return status.originalCreatedAt ?? status.createdAt;
}
