import React from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import {
  uploadBrowserFile,
  uploadDroppedMediaPath,
} from "../../api/mediaUpload";
import { hasTauriRuntime } from "../../api/tauri";
import { t } from "../../i18n";
import type { ComposeMediaAttachment } from "../../types/app";
import { filenameFromPath } from "../../utils/format";
import { moveQueueItem } from "./mediaQueue";

const mimeExtensionMap: Record<string, string> = {
  "image/gif": "gif",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/webp": "webp",
};

function pastedImageFilename(file: File, index: number) {
  if (file.name.trim()) return file.name;
  const extension = mimeExtensionMap[file.type] ?? "png";
  return `pasted-image-${Date.now()}-${index + 1}.${extension}`;
}

function browserFileMediaType(file: File) {
  if (file.type.startsWith("video/")) return "video";
  if (file.type.startsWith("audio/")) return "audio";
  if (file.type === "image/gif" || file.type === "image/apng") {
    return file.type;
  }
  if (file.type.startsWith("image/")) return "image";
  return "unknown";
}

function revokeOwnedPreview(previewSrc: string, ownedPreviews: Set<string>) {
  if (!ownedPreviews.delete(previewSrc)) return;
  URL.revokeObjectURL(previewSrc);
}

type ComposeMediaQueueOptions = {
  activeAcct: string | null;
  editing: boolean;
  uploadSupported: boolean;
  maxAttachments: number;
  dropTargetRef: React.RefObject<HTMLElement | null>;
  onError: (error: unknown) => void;
};

export function useComposeMediaQueue({
  activeAcct,
  editing,
  uploadSupported,
  maxAttachments,
  dropTargetRef,
  onError,
}: ComposeMediaQueueOptions) {
  const [attachments, setAttachments] = React.useState<
    ComposeMediaAttachment[]
  >([]);
  const [announcement, setAnnouncement] = React.useState("");
  const generationRef = React.useRef(1);
  const activeAcctRef = React.useRef<string | null>(activeAcct);
  const previousActiveAcctRef = React.useRef<string | null>(activeAcct);
  const controllersRef = React.useRef(new Map<string, AbortController>());
  const attachmentsRef = React.useRef<ComposeMediaAttachment[]>([]);
  const occupiedSlotsRef = React.useRef(0);
  const pendingSlotsRef = React.useRef(new Set<string>());
  const ownedPreviewsRef = React.useRef(new Set<string>());
  const previewsPendingRevokeRef = React.useRef(new Set<string>());

  const releasePendingSlot = React.useCallback((localId: string) => {
    if (!pendingSlotsRef.current.delete(localId)) return false;
    occupiedSlotsRef.current = Math.max(0, occupiedSlotsRef.current - 1);
    return true;
  }, []);

  const resetQueueResources = React.useCallback(() => {
    generationRef.current += 1;
    for (const controller of controllersRef.current.values()) {
      controller.abort();
    }
    controllersRef.current.clear();
    pendingSlotsRef.current.clear();
    occupiedSlotsRef.current = 0;
    previewsPendingRevokeRef.current.clear();
    for (const previewSrc of [...ownedPreviewsRef.current]) {
      revokeOwnedPreview(previewSrc, ownedPreviewsRef.current);
    }
    attachmentsRef.current = [];
  }, []);

  const clear = React.useCallback(() => {
    resetQueueResources();
    setAttachments([]);
  }, [resetQueueResources]);

  React.useLayoutEffect(() => {
    attachmentsRef.current = attachments;
    for (const previewSrc of previewsPendingRevokeRef.current) {
      revokeOwnedPreview(previewSrc, ownedPreviewsRef.current);
    }
    previewsPendingRevokeRef.current.clear();
  }, [attachments]);

  React.useEffect(() => {
    activeAcctRef.current = activeAcct;
    if (previousActiveAcctRef.current === activeAcct) return;
    previousActiveAcctRef.current = activeAcct;
    clear();
  }, [activeAcct, clear]);

  React.useEffect(
    () => () => resetQueueResources(),
    [resetQueueResources],
  );

  const uploadFiles = React.useCallback(
    async (files: File[]) => {
      if (editing || !activeAcct || !uploadSupported) return;
      const actingAccountAcct = activeAcct;
      const generation = generationRef.current;
      const available = Math.max(
        0,
        maxAttachments - occupiedSlotsRef.current,
      );
      const queuedFiles = files.slice(0, available).map((file) => ({
        file,
        localId: crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`,
      }));
      for (const { localId } of queuedFiles) {
        pendingSlotsRef.current.add(localId);
      }
      occupiedSlotsRef.current += queuedFiles.length;

      for (const { file, localId } of queuedFiles) {
        if (
          generation !== generationRef.current ||
          activeAcctRef.current !== actingAccountAcct ||
          !pendingSlotsRef.current.has(localId)
        ) {
          continue;
        }
        const previewSrc = URL.createObjectURL(file);
        ownedPreviewsRef.current.add(previewSrc);
        const controller = new AbortController();
        controllersRef.current.set(localId, controller);
        setAttachments((current) =>
          [
            ...current,
            {
              id: localId,
              filename: file.name,
              previewSrc,
              uploading: true,
              media_type: browserFileMediaType(file),
            },
          ],
        );
        try {
          const uploaded = await uploadBrowserFile(actingAccountAcct, file, {
            signal: controller.signal,
            onProgress: ({ written, total }) => {
              if (
                generation !== generationRef.current ||
                activeAcctRef.current !== actingAccountAcct ||
                !pendingSlotsRef.current.has(localId)
              )
                return;
              setAttachments((current) =>
                current.map((attachment) =>
                  attachment.id === localId
                    ? {
                        ...attachment,
                        uploadProgress: total > 0 ? written / total : 0,
                      }
                    : attachment,
                ),
              );
            },
          });
          if (
            generation !== generationRef.current ||
            activeAcctRef.current !== actingAccountAcct ||
            !pendingSlotsRef.current.has(localId)
          ) {
            revokeOwnedPreview(previewSrc, ownedPreviewsRef.current);
            continue;
          }
          const uploadedMediaType =
            uploaded.media_type ?? uploaded.type ?? browserFileMediaType(file);
          const preserveAnimatedOriginal =
            file.type === "image/gif" || file.type === "image/apng";
          const remotePreview = (
            preserveAnimatedOriginal
              ? [uploaded.url, uploaded.remote_url, uploaded.preview_url]
              : [uploaded.preview_url, uploaded.url, uploaded.remote_url]
          ).find(
            (candidate): candidate is string =>
              typeof candidate === "string" && candidate.trim().length > 0,
          );
          const effectiveMediaType =
            uploadedMediaType === "image" && preserveAnimatedOriginal
              ? file.type
              : uploadedMediaType;
          if (remotePreview && remotePreview !== previewSrc) {
            previewsPendingRevokeRef.current.add(previewSrc);
          }
          setAttachments((current) =>
            current.map((attachment) =>
              attachment.id === localId
                ? {
                    ...attachment,
                    ...uploaded,
                    id: uploaded.id,
                    filename: file.name,
                    media_type: effectiveMediaType,
                    type: effectiveMediaType,
                    previewSrc: remotePreview ?? previewSrc,
                    uploading: false,
                    uploadProgress: 1,
                  }
                : attachment,
            ),
          );
          pendingSlotsRef.current.delete(localId);
        } catch (error) {
          revokeOwnedPreview(previewSrc, ownedPreviewsRef.current);
          const releasedSlot = releasePendingSlot(localId);
          if (
            releasedSlot &&
            generation === generationRef.current &&
            activeAcctRef.current === actingAccountAcct
          ) {
            setAttachments((current) =>
              current.filter((attachment) => attachment.id !== localId),
            );
          }
          if (
            !controller.signal.aborted &&
            generation === generationRef.current &&
            activeAcctRef.current === actingAccountAcct
          ) {
            onError(error);
          }
        } finally {
          controllersRef.current.delete(localId);
        }
      }
    },
    [
      activeAcct,
      editing,
      maxAttachments,
      onError,
      releasePendingSlot,
      uploadSupported,
    ],
  );

  const uploadDroppedPaths = React.useCallback(
    async (paths: string[]) => {
      if (editing || !activeAcct || !uploadSupported) return;
      const actingAccountAcct = activeAcct;
      const generation = generationRef.current;
      const available = Math.max(
        0,
        maxAttachments - occupiedSlotsRef.current,
      );
      const queuedPaths = paths.slice(0, available).map((path) => ({
        path,
        localId: crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`,
      }));
      for (const { localId } of queuedPaths) {
        pendingSlotsRef.current.add(localId);
      }
      occupiedSlotsRef.current += queuedPaths.length;

      for (const { path, localId } of queuedPaths) {
        if (
          generation !== generationRef.current ||
          activeAcctRef.current !== actingAccountAcct ||
          !pendingSlotsRef.current.has(localId)
        ) {
          continue;
        }
        const filename = filenameFromPath(path);
        setAttachments((current) =>
          [
            ...current,
            {
              id: localId,
              filename,
              previewSrc: "",
              uploading: true,
              media_type: "unknown",
            },
          ],
        );
        try {
          const uploaded = await uploadDroppedMediaPath(
            actingAccountAcct,
            path,
          );
          if (
            generation !== generationRef.current ||
            activeAcctRef.current !== actingAccountAcct ||
            !pendingSlotsRef.current.has(localId)
          )
            continue;
          setAttachments((current) =>
            current.map((attachment) =>
              attachment.id === localId
                ? {
                    ...attachment,
                    ...uploaded,
                    id: uploaded.id,
                    filename,
                    previewSrc:
                      uploaded.preview_url ??
                      uploaded.url ??
                      uploaded.remote_url ??
                      "",
                    uploading: false,
                  }
                : attachment,
            ),
          );
          pendingSlotsRef.current.delete(localId);
        } catch (error) {
          const releasedSlot = releasePendingSlot(localId);
          if (
            releasedSlot &&
            generation === generationRef.current &&
            activeAcctRef.current === actingAccountAcct
          ) {
            setAttachments((current) =>
              current.filter((attachment) => attachment.id !== localId),
            );
          }
          if (
            releasedSlot &&
            generation === generationRef.current &&
            activeAcctRef.current === actingAccountAcct
          ) {
            onError(error);
          }
        }
      }
    },
    [
      activeAcct,
      editing,
      maxAttachments,
      onError,
      releasePendingSlot,
      uploadSupported,
    ],
  );

  React.useEffect(() => {
    if (!hasTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const pendingDropUploads = new Set<number>();
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const rect = dropTargetRef.current?.getBoundingClientRect();
        if (rect && "position" in event.payload) {
          const x = event.payload.position.x / window.devicePixelRatio;
          const y = event.payload.position.y / window.devicePixelRatio;
          if (
            x < rect.left ||
            x > rect.right ||
            y < rect.top ||
            y > rect.bottom
          )
            return;
        }
        const paths = [...event.payload.paths];
        // Tauri forwards the native drop to JavaScript before the remaining
        // Rust WebView listeners run. Starting IPC from inside this callback
        // re-enters the backend and prevents the trusted path registration
        // listener from observing the drop. A macrotask lets native dispatch
        // finish before the capability is claimed.
        const timer = window.setTimeout(() => {
          pendingDropUploads.delete(timer);
          void uploadDroppedPaths(paths);
        }, 0);
        pendingDropUploads.add(timer);
      })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(onError);
    return () => {
      disposed = true;
      for (const timer of pendingDropUploads) window.clearTimeout(timer);
      pendingDropUploads.clear();
      unlisten?.();
    };
  }, [dropTargetRef, onError, uploadDroppedPaths]);

  const handlePaste = React.useCallback(
    (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
      if (editing) return;
      const images = Array.from(event.clipboardData.items)
        .filter(
          (item) => item.kind === "file" && item.type.startsWith("image/"),
        )
        .map((item, index) => {
          const file = item.getAsFile();
          if (!file) return null;
          return file.name.trim()
            ? file
            : new File([file], pastedImageFilename(file, index), {
                type: file.type,
                lastModified: file.lastModified,
              });
        })
        .filter((file): file is File => Boolean(file));
      if (images.length === 0) return;
      event.preventDefault();
      void uploadFiles(images);
    },
    [editing, uploadFiles],
  );

  const remove = React.useCallback((index: number) => {
    const target = attachmentsRef.current[index];
    if (!target) return;
    const controller = controllersRef.current.get(target.id);
    controller?.abort();
    controllersRef.current.delete(target.id);
    pendingSlotsRef.current.delete(target.id);
    occupiedSlotsRef.current = Math.max(0, occupiedSlotsRef.current - 1);
    previewsPendingRevokeRef.current.delete(target.previewSrc);
    revokeOwnedPreview(target.previewSrc, ownedPreviewsRef.current);
    const next = attachmentsRef.current.filter(
      (_, itemIndex) => itemIndex !== index,
    );
    attachmentsRef.current = next;
    setAttachments(next);
  }, []);

  const move = React.useCallback(
    (from: number, to: number, announce = false) => {
      if (from === to) return;
      setAttachments((current) => moveQueueItem(current, from, to));
      if (announce)
        setAnnouncement(t("a11y.media.moved", { position: to + 1 }));
    },
    [],
  );

  return {
    attachments,
    announcement,
    uploading: attachments.some((attachment) => attachment.uploading),
    uploadFiles,
    handlePaste,
    remove,
    move,
    clear,
  };
}
