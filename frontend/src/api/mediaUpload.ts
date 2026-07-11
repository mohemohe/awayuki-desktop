import type { MediaAttachment } from "../types/app";
import { invokeCommand } from "./tauri";

const MAX_CHUNK_BYTES = 256 * 1024;

type BeginResponse = { uploadId: string };
type ProgressResponse = { written: number; total: number };

export async function uploadBrowserFile(
  actingAccountAcct: string,
  file: File,
  options: {
    signal?: AbortSignal;
    onProgress?: (progress: ProgressResponse) => void;
  } = {},
) {
  const { uploadId } = await invokeCommand<BeginResponse>(
    "begin_compose_media_upload",
    {
      request: {
        actingAccountAcct,
        filename: file.name,
        mimeType: file.type || "application/octet-stream",
        size: file.size,
      },
    },
  );
  let finished = false;
  try {
    for (let offset = 0; offset < file.size; offset += MAX_CHUNK_BYTES) {
      throwIfAborted(options.signal);
      const chunk = new Uint8Array(
        await readBlobAsArrayBuffer(
          file.slice(offset, offset + MAX_CHUNK_BYTES),
        ),
      );
      throwIfAborted(options.signal);
      const progress = await invokeCommand<ProgressResponse>(
        "append_compose_media_upload",
        { request: { uploadId, data: Array.from(chunk) } },
      );
      options.onProgress?.(progress);
    }
    const result = await invokeCommand<MediaAttachment>(
      "finish_compose_media_upload",
      { request: { uploadId } },
    );
    finished = true;
    return result;
  } finally {
    if (!finished) {
      await invokeCommand("cancel_compose_media_upload", {
        request: { uploadId },
      }).catch(() => undefined);
    }
  }
}

function readBlobAsArrayBuffer(blob: Blob) {
  if (typeof blob.arrayBuffer === "function") return blob.arrayBuffer();
  return new Promise<ArrayBuffer>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Media read failed"));
    reader.onload = () => resolve(reader.result as ArrayBuffer);
    reader.readAsArrayBuffer(blob);
  });
}

export async function uploadDroppedMediaPath(
  actingAccountAcct: string,
  path: string,
) {
  const { capability } = await invokeCommand<{ capability: string }>(
    "claim_dropped_media_path",
    { request: { path } },
  );
  return invokeCommand<MediaAttachment>("upload_compose_media_path", {
    request: { actingAccountAcct, path, capability },
  });
}

function throwIfAborted(signal?: AbortSignal) {
  if (signal?.aborted) throw new DOMException("Upload cancelled", "AbortError");
}
