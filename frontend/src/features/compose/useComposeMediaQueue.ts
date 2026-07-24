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

function revokePreview(attachment: ComposeMediaAttachment) {
  if (attachment.previewSrc.startsWith("blob:")) {
    URL.revokeObjectURL(attachment.previewSrc);
  }
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

  const clear = React.useCallback(() => {
    setAttachments((current) => {
      current.forEach(revokePreview);
      return [];
    });
  }, []);

  React.useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);

  React.useEffect(() => {
    activeAcctRef.current = activeAcct;
    if (previousActiveAcctRef.current === activeAcct) return;
    previousActiveAcctRef.current = activeAcct;
    generationRef.current += 1;
    for (const controller of controllersRef.current.values())
      controller.abort();
    controllersRef.current.clear();
    clear();
  }, [activeAcct, clear]);

  React.useEffect(
    () => () => {
      generationRef.current += 1;
      for (const controller of controllersRef.current.values())
        controller.abort();
      controllersRef.current.clear();
      attachmentsRef.current.forEach(revokePreview);
    },
    [],
  );

  const uploadFiles = React.useCallback(
    async (files: File[]) => {
      if (editing || !activeAcct || !uploadSupported) return;
      const actingAccountAcct = activeAcct;
      const generation = generationRef.current;
      const available = Math.max(
        0,
        maxAttachments - attachmentsRef.current.length,
      );
      for (const file of files.slice(0, available)) {
        const localId =
          crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
        const previewSrc = URL.createObjectURL(file);
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
              media_type: file.type.startsWith("video/") ? "video" : "image",
            },
          ].slice(0, maxAttachments),
        );
        try {
          const uploaded = await uploadBrowserFile(actingAccountAcct, file, {
            signal: controller.signal,
            onProgress: ({ written, total }) => {
              if (
                generation !== generationRef.current ||
                activeAcctRef.current !== actingAccountAcct
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
            activeAcctRef.current !== actingAccountAcct
          ) {
            URL.revokeObjectURL(previewSrc);
            continue;
          }
          setAttachments((current) =>
            current.map((attachment) =>
              attachment.id === localId
                ? {
                    ...attachment,
                    ...uploaded,
                    id: uploaded.id,
                    filename: file.name,
                    previewSrc:
                      uploaded.preview_url ??
                      uploaded.url ??
                      uploaded.remote_url ??
                      previewSrc,
                    uploading: false,
                    uploadProgress: 1,
                  }
                : attachment,
            ),
          );
        } catch (error) {
          URL.revokeObjectURL(previewSrc);
          setAttachments((current) =>
            current.filter((attachment) => attachment.id !== localId),
          );
          if (!controller.signal.aborted) onError(error);
        } finally {
          controllersRef.current.delete(localId);
        }
      }
    },
    [activeAcct, editing, maxAttachments, onError, uploadSupported],
  );

  const uploadDroppedPaths = React.useCallback(
    async (paths: string[]) => {
      if (editing || !activeAcct || !uploadSupported) return;
      const actingAccountAcct = activeAcct;
      const generation = generationRef.current;
      const available = Math.max(
        0,
        maxAttachments - attachmentsRef.current.length,
      );
      for (const path of paths.slice(0, available)) {
        const filename = filenameFromPath(path);
        const localId =
          crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
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
          ].slice(0, maxAttachments),
        );
        try {
          const uploaded = await uploadDroppedMediaPath(
            actingAccountAcct,
            path,
          );
          if (
            generation !== generationRef.current ||
            activeAcctRef.current !== actingAccountAcct
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
        } catch (error) {
          setAttachments((current) =>
            current.filter((attachment) => attachment.id !== localId),
          );
          if (
            generation === generationRef.current &&
            activeAcctRef.current === actingAccountAcct
          )
            onError(error);
        }
      }
    },
    [activeAcct, editing, maxAttachments, onError, uploadSupported],
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
    setAttachments((current) => {
      const target = current[index];
      if (target) revokePreview(target);
      return current.filter((_, itemIndex) => itemIndex !== index);
    });
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
