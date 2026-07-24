import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  invokeTypedCommand: vi.fn(),
  invokeRawCommand: vi.fn(),
}));

vi.mock("./tauri", () => api);

import { uploadBrowserFile } from "./mediaUpload";

describe("chunked compose media upload", () => {
  beforeEach(() => {
    api.invokeTypedCommand.mockReset();
    api.invokeRawCommand.mockReset();
  });

  it("never sends a browser file chunk larger than 256 KiB", async () => {
    const bytes = new Uint8Array(600_000);
    bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const file = new File([bytes], "image.png", { type: "image/png" });
    let written = 0;
    api.invokeTypedCommand.mockImplementation(
      async (command: string, _args?: Record<string, unknown>) => {
        if (command === "begin_compose_media_upload") {
          return { uploadId: "upload-1" };
        }
        if (command === "finish_compose_media_upload") {
          return { id: "media-1" };
        }
        throw new Error(`unexpected command ${command}`);
      },
    );
    api.invokeRawCommand.mockImplementation(
      async (command: string, body: Uint8Array, headers: HeadersInit) => {
        expect(command).toBe("append_compose_media_upload");
        expect(body).toBeInstanceOf(Uint8Array);
        expect(body.byteLength).toBeLessThanOrEqual(256 * 1024);
        expect(new Headers(headers).get("x-awayuki-upload-id")).toBe("upload-1");
        written += body.byteLength;
        return { written, total: file.size };
      },
    );

    const progress: number[] = [];
    const media = await uploadBrowserFile("alice@example.test", file, {
      onProgress: (value) => progress.push(value.written),
    });

    expect(media.id).toBe("media-1");
    expect(written).toBe(file.size);
    expect(progress[progress.length - 1]).toBe(file.size);
    expect(
      api.invokeRawCommand.mock.calls.length,
    ).toBeGreaterThan(1);
  });

  it("cancels the backend upload when its account scope is aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    api.invokeTypedCommand.mockImplementation(async (command: string) => {
      if (command === "begin_compose_media_upload") {
        return { uploadId: "upload-2" };
      }
      if (command === "cancel_compose_media_upload") return undefined;
      throw new Error(`unexpected command ${command}`);
    });

    await expect(
      uploadBrowserFile(
        "alice@example.test",
        new File([new Uint8Array([1])], "image.png", { type: "image/png" }),
        { signal: controller.signal },
      ),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(api.invokeTypedCommand).toHaveBeenCalledWith(
      "cancel_compose_media_upload",
      { request: { uploadId: "upload-2" } },
    );
  });
});
