import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ invokeCommand: vi.fn() }));

vi.mock("./tauri", () => api);

import { uploadBrowserFile } from "./mediaUpload";

describe("chunked compose media upload", () => {
  beforeEach(() => {
    api.invokeCommand.mockReset();
  });

  it("never sends a browser file chunk larger than 256 KiB", async () => {
    const bytes = new Uint8Array(600_000);
    bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const file = new File([bytes], "image.png", { type: "image/png" });
    let written = 0;
    api.invokeCommand.mockImplementation(
      async (command: string, args?: Record<string, unknown>) => {
        if (command === "begin_compose_media_upload") {
          return { uploadId: "upload-1" };
        }
        if (command === "append_compose_media_upload") {
          const request = args?.request as { data: number[] };
          expect(request.data.length).toBeLessThanOrEqual(256 * 1024);
          written += request.data.length;
          return { written, total: file.size };
        }
        if (command === "finish_compose_media_upload") {
          return { id: "media-1" };
        }
        throw new Error(`unexpected command ${command}`);
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
      api.invokeCommand.mock.calls.filter(
        ([command]) => command === "append_compose_media_upload",
      ).length,
    ).toBeGreaterThan(1);
  });

  it("cancels the backend upload when its account scope is aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    api.invokeCommand.mockImplementation(async (command: string) => {
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
    expect(api.invokeCommand).toHaveBeenCalledWith(
      "cancel_compose_media_upload",
      { request: { uploadId: "upload-2" } },
    );
  });
});
