import React from "react";
import { invokeTypedCommand } from "../api/tauri";
import type { MediaAttachment } from "../types/app";
import { LruCache } from "./lru";
import { thumbnailSourceNeedsProbe } from "./media";

const verdicts = new LruCache<string, boolean>(1024, { ttlMs: 10 * 60_000 });
const inFlight = new Map<string, Promise<boolean>>();

function probeThumbnailSource(source: string) {
  let flight = inFlight.get(source);
  if (!flight) {
    flight = invokeTypedCommand("probe_media_image", {
      request: { url: source },
    })
      .then((isImage) => {
        verdicts.set(source, isImage);
        return isImage;
      })
      // Fail closed without caching: an unverified poster may be the video
      // itself, while a transient failure must not outlive this mount.
      .catch(() => false)
      .finally(() => {
        inFlight.delete(source);
      });
    inFlight.set(source, flight);
  }
  return flight;
}

export function useVerifiedThumbnailSources(
  media: MediaAttachment,
  sources: string[],
  enabled: boolean,
) {
  const [candidate] = sources;
  const target =
    enabled && candidate && thumbnailSourceNeedsProbe(media, candidate)
      ? candidate
      : null;
  const [settled, setSettled] = React.useState<{
    source: string;
    isImage: boolean;
  } | null>(null);
  const isImage = !target
    ? undefined
    : settled?.source === target
      ? settled.isImage
      : verdicts.get(target);

  React.useEffect(() => {
    if (!target || isImage !== undefined) return;
    let active = true;
    void probeThumbnailSource(target).then((result) => {
      if (active) setSettled({ source: target, isImage: result });
    });
    return () => {
      active = false;
    };
  }, [isImage, target]);

  return {
    sources: !target || isImage ? sources : [],
    pending: target !== null && isImage === undefined,
  };
}
