import React from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

  afterEach(() => {
    vi.restoreAllMocks();
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

  it("revokes the local preview after a remote preview replaces it", async () => {
    mediaApi.uploadBrowserFile.mockResolvedValue({
      id: "uploaded-media",
      url: "https://one.example/media",
      preview_url: "https://one.example/preview",
      remote_url: null,
      media_type: "image",
    });
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError,
      }),
    );

    await act(async () => {
      await result.current.uploadFiles([
        new File(["image"], "photo.png", { type: "image/png" }),
      ]);
    });

    expect(result.current.attachments).toEqual([
      expect.objectContaining({
        id: "uploaded-media",
        previewSrc: "https://one.example/preview",
        uploading: false,
      }),
    ]);
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
    expect(onError).not.toHaveBeenCalled();
  });

  it("classifies local audio before upload completion so it never enters an image preview", async () => {
    mediaApi.uploadBrowserFile.mockImplementation(
      () => new Promise(() => undefined),
    );
    const { result, unmount } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError: vi.fn(),
      }),
    );

    act(() => {
      void result.current.uploadFiles([
        new File(["audio"], "voice.mp3", { type: "audio/mpeg" }),
      ]);
    });

    await waitFor(() =>
      expect(result.current.attachments[0]).toEqual(
        expect.objectContaining({
          filename: "voice.mp3",
          media_type: "audio",
          previewSrc: "blob:preview",
        }),
      ),
    );
    unmount();
  });

  it("preserves an uploaded GIF MIME so the animated original stays selected", async () => {
    mediaApi.uploadBrowserFile.mockResolvedValue({
      id: "uploaded-gif",
      url: "https://one.example/animation.gif",
      preview_url: "https://one.example/static-preview.png",
      remote_url: null,
      type: "image",
    });
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError: vi.fn(),
      }),
    );

    await act(async () => {
      await result.current.uploadFiles([
        new File(["gif"], "animation.gif", { type: "image/gif" }),
      ]);
    });

    expect(result.current.attachments[0]).toEqual(
      expect.objectContaining({
        media_type: "image/gif",
        type: "image/gif",
        url: "https://one.example/animation.gif",
        previewSrc: "https://one.example/animation.gif",
      }),
    );
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
  });

  it("preserves APNG MIME when the backend serializes its attachment kind as type", async () => {
    mediaApi.uploadBrowserFile.mockResolvedValue({
      id: "uploaded-apng",
      url: "https://one.example/animation.png",
      preview_url: "https://one.example/static-preview.png",
      remote_url: null,
      type: "image",
    });
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError: vi.fn(),
      }),
    );

    await act(async () => {
      await result.current.uploadFiles([
        new File(["apng"], "animation.png", { type: "image/apng" }),
      ]);
    });

    expect(result.current.attachments[0]).toEqual(
      expect.objectContaining({
        media_type: "image/apng",
        type: "image/apng",
        url: "https://one.example/animation.png",
        previewSrc: "https://one.example/animation.png",
      }),
    );
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
  });

  it("retains the local preview until clear when the server returns no URL", async () => {
    mediaApi.uploadBrowserFile.mockResolvedValue({
      id: "uploaded-media",
      url: null,
      preview_url: null,
      remote_url: null,
      media_type: "image",
    });
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError: vi.fn(),
      }),
    );

    await act(async () => {
      await result.current.uploadFiles([
        new File(["image"], "photo.png", { type: "image/png" }),
      ]);
    });

    expect(result.current.attachments[0]?.previewSrc).toBe("blob:preview");
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();

    act(() => result.current.clear());

    expect(result.current.attachments).toEqual([]);
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
  });

  it("reserves capacity before concurrent file additions create previews", async () => {
    let resolveUpload:
      | ((value: {
          id: string;
          url: string;
          preview_url: string;
          remote_url: null;
          media_type: string;
        }) => void)
      | undefined;
    mediaApi.uploadBrowserFile.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveUpload = resolve;
        }),
    );
    vi.mocked(URL.createObjectURL).mockImplementation(
      (blob) => `blob:${(blob as File).name}`,
    );
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 1,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError: vi.fn(),
      }),
    );

    let firstUpload: Promise<void> | undefined;
    await act(async () => {
      firstUpload = result.current.uploadFiles([
        new File(["first"], "first.png", { type: "image/png" }),
      ]);
      await result.current.uploadFiles([
        new File(["second"], "second.png", { type: "image/png" }),
      ]);
    });

    expect(mediaApi.uploadBrowserFile).toHaveBeenCalledTimes(1);
    expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
    expect(result.current.attachments).toHaveLength(1);

    resolveUpload?.({
      id: "uploaded-media",
      url: "https://one.example/media",
      preview_url: "https://one.example/preview",
      remote_url: null,
      media_type: "image",
    });
    await act(async () => firstUpload);

    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:first.png");
  });

  it("aborts an in-flight upload when its attachment is removed", async () => {
    let uploadSignal: AbortSignal | undefined;
    mediaApi.uploadBrowserFile.mockImplementation((_acct, _file, options) => {
      uploadSignal = options.signal;
      return new Promise((_resolve, reject) => {
        options.signal.addEventListener(
          "abort",
          () => reject(new DOMException("Upload cancelled", "AbortError")),
          { once: true },
        );
      });
    });
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError,
      }),
    );

    let upload: Promise<void> | undefined;
    act(() => {
      upload = result.current.uploadFiles([
        new File(["image"], "photo.png", { type: "image/png" }),
      ]);
    });
    await waitFor(() => expect(result.current.attachments).toHaveLength(1));

    await act(async () => {
      result.current.remove(0);
      await upload;
    });

    expect(uploadSignal?.aborted).toBe(true);
    expect(result.current.attachments).toEqual([]);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
    expect(onError).not.toHaveBeenCalled();
  });

  it("aborts all in-flight uploads when the queue is cleared", async () => {
    let uploadSignal: AbortSignal | undefined;
    mediaApi.uploadBrowserFile.mockImplementation((_acct, _file, options) => {
      uploadSignal = options.signal;
      return new Promise((_resolve, reject) => {
        options.signal.addEventListener(
          "abort",
          () => reject(new DOMException("Upload cancelled", "AbortError")),
          { once: true },
        );
      });
    });
    const onError = vi.fn();
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError,
      }),
    );

    let upload: Promise<void> | undefined;
    act(() => {
      upload = result.current.uploadFiles([
        new File(["image"], "photo.png", { type: "image/png" }),
      ]);
    });
    await waitFor(() => expect(result.current.attachments).toHaveLength(1));

    await act(async () => {
      result.current.clear();
      await upload;
    });

    expect(uploadSignal?.aborted).toBe(true);
    expect(result.current.attachments).toEqual([]);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
    expect(onError).not.toHaveBeenCalled();
  });

  it("aborts and releases an in-flight upload on unmount", async () => {
    let uploadSignal: AbortSignal | undefined;
    mediaApi.uploadBrowserFile.mockImplementation((_acct, _file, options) => {
      uploadSignal = options.signal;
      return new Promise((_resolve, reject) => {
        options.signal.addEventListener(
          "abort",
          () => reject(new DOMException("Upload cancelled", "AbortError")),
          { once: true },
        );
      });
    });
    const onError = vi.fn();
    const { result, unmount } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError,
      }),
    );

    let upload: Promise<void> | undefined;
    act(() => {
      upload = result.current.uploadFiles([
        new File(["image"], "photo.png", { type: "image/png" }),
      ]);
    });
    await waitFor(() => expect(result.current.attachments).toHaveLength(1));

    await act(async () => {
      unmount();
      await upload;
    });

    expect(uploadSignal?.aborted).toBe(true);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:preview");
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

  it("reorders and filters current attachments from plugin media ids", async () => {
    mediaApi.uploadBrowserFile.mockImplementation((_acct, file: File) => {
      const stem = file.name.replace(".png", "");
      return Promise.resolve({
        id: `media-${stem}`,
        url: `https://one.example/${stem}`,
        preview_url: `https://one.example/${stem}/preview`,
        remote_url: null,
        media_type: "image",
      });
    });
    const { result } = renderHook(() =>
      useComposeMediaQueue({
        activeAcct: "alice@one.example",
        editing: false,
        uploadSupported: true,
        maxAttachments: 4,
        dropTargetRef: React.createRef<HTMLElement>(),
        onError: vi.fn(),
      }),
    );
    await act(async () => {
      await result.current.uploadFiles(
        ["a", "b", "c"].map(
          (name) =>
            new File([name], `${name}.png`, { type: "image/png" }),
        ),
      );
    });

    act(() => {
      result.current.replaceWithIds([
        "media-c",
        "media-a",
        "unknown",
        "media-a",
      ]);
    });

    expect(result.current.attachments.map((attachment) => attachment.id)).toEqual([
      "media-c",
      "media-a",
    ]);
    expect(result.current.getCurrentAttachmentState()).toEqual({
      ids: ["media-c", "media-a"],
      uploading: false,
    });
  });
});
