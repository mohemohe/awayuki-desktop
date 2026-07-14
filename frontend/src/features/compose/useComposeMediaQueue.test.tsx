import React from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mediaApi = vi.hoisted(() => ({
  uploadBrowserFile: vi.fn(),
  uploadDroppedMediaPath: vi.fn(),
}));
const tauriRuntime = vi.hoisted(() => ({
  enabled: false,
  handler: undefined as
    | ((event: {
        payload: {
          type: "drop";
          paths: string[];
          position: { x: number; y: number };
        };
      }) => void)
    | undefined,
  onDragDropEvent: vi.fn(),
}));

vi.mock("../../api/mediaUpload", () => mediaApi);
vi.mock("../../api/tauri", () => ({
  hasTauriRuntime: () => tauriRuntime.enabled,
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: tauriRuntime.onDragDropEvent,
  }),
}));

import { useComposeMediaQueue } from "./useComposeMediaQueue";

describe("useComposeMediaQueue", () => {
  beforeEach(() => {
    mediaApi.uploadBrowserFile.mockReset();
    mediaApi.uploadDroppedMediaPath.mockReset();
    tauriRuntime.enabled = false;
    tauriRuntime.handler = undefined;
    tauriRuntime.onDragDropEvent.mockReset();
    tauriRuntime.onDragDropEvent.mockImplementation(async (handler) => {
      tauriRuntime.handler = handler;
      return vi.fn();
    });
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:preview");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
  });

  it("defers dropped-path IPC until native drop dispatch has returned", async () => {
    tauriRuntime.enabled = true;
    mediaApi.uploadDroppedMediaPath.mockResolvedValue({
      id: "dropped-media",
      url: "https://one.example/media",
      preview_url: "https://one.example/preview",
      remote_url: null,
      media_type: "image",
    });
    const dropTargetRef = { current: null };
    const onError = vi.fn();
    renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef,
        onError,
      }),
    );
    await waitFor(() => expect(tauriRuntime.handler).toBeTypeOf("function"));

    act(() => {
      tauriRuntime.handler?.({
        payload: {
          type: "drop",
          paths: ["/tmp/dropped-image.png"],
          position: { x: 1, y: 1 },
        },
      });
      expect(mediaApi.uploadDroppedMediaPath).not.toHaveBeenCalled();
    });

    await waitFor(() =>
      expect(mediaApi.uploadDroppedMediaPath).toHaveBeenCalledWith(
        "alice@one.example",
        "/tmp/dropped-image.png",
      ),
    );
    expect(onError).not.toHaveBeenCalled();
  });

  it("aborts the previous acting account upload and ignores its late result", async () => {
    let resolveUpload:
      | ((value: {
          id: string;
          url: string;
          preview_url: string;
          remote_url: null;
          media_type: string;
        }) => void)
      | undefined;
    let uploadSignal: AbortSignal | undefined;
    mediaApi.uploadBrowserFile.mockImplementation((_acct, _file, options) => {
      uploadSignal = options.signal;
      return new Promise((resolve) => {
        resolveUpload = resolve;
      });
    });
    const dropTargetRef = React.createRef<HTMLElement>();
    const onError = vi.fn();
    const { result, rerender } = renderHook(
      ({ activeAcct }) =>
        useComposeMediaQueue({
          activeAcct,
          editing: false,
          uploadSupported: true,
          maxAttachments: 4,
          dropTargetRef,
          onError,
        }),
      { initialProps: { activeAcct: "alice@one.example" } },
    );

    let upload: Promise<void> | undefined;
    act(() => {
      upload = result.current.uploadFiles([
        new File(["image"], "photo.png", { type: "image/png" }),
      ]);
    });
    await waitFor(() => expect(result.current.attachments).toHaveLength(1));
    expect(mediaApi.uploadBrowserFile).toHaveBeenCalledWith(
      "alice@one.example",
      expect.any(File),
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );

    rerender({ activeAcct: "bob@two.example" });
    await waitFor(() => expect(result.current.attachments).toEqual([]));
    expect(uploadSignal?.aborted).toBe(true);

    resolveUpload?.({
      id: "late-media",
      url: "https://one.example/media",
      preview_url: "https://one.example/preview",
      remote_url: null,
      media_type: "image",
    });
    await act(async () => upload);

    expect(result.current.attachments).toEqual([]);
    expect(onError).not.toHaveBeenCalled();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
  });
});
