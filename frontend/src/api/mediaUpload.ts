import { invokeRawCommand, invokeTypedCommand } from "./tauri";

const MAX_CHUNK_BYTES = 256 * 1024;

type ProgressResponse = { written: number; total: number };

export async function uploadBrowserFile(
  actingAccountAcct: string,
  file: File,
  options: {
    signal?: AbortSignal;
    onProgress?: (progress: ProgressResponse) => void;
  } = {},
) {
  const { uploadId } = await invokeTypedCommand(
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
      const progress = await invokeRawCommand(
        "append_compose_media_upload",
        chunk,
        { "x-awayuki-upload-id": uploadId },
      );
      options.onProgress?.(progress);
    }
    const result = await invokeTypedCommand(
      "finish_compose_media_upload",
      { request: { uploadId } },
    );
    finished = true;
    return result;
  } finally {
    if (!finished) {
      await invokeTypedCommand("cancel_compose_media_upload", {
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
  const { capability } = await invokeTypedCommand(
    "claim_dropped_media_path",
    { request: { path } },
  );
  return invokeTypedCommand("upload_compose_media_path", {
    request: { actingAccountAcct, path, capability },
  });
}

function throwIfAborted(signal?: AbortSignal) {
  if (signal?.aborted) throw new DOMException("Upload cancelled", "AbortError");
}
