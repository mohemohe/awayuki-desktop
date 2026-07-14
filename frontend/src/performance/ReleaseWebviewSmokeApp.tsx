import React from "react";
import { Webview } from "@tauri-apps/api/webview";
import { invokeTypedCommand } from "../api/tauri";
import { sidecarWebviewLabel } from "../domain/sidecar";

const SIDECAR_ID = "release-security-smoke";
const REPORT_PREFIX = "AWAYUKI_WEBVIEW_SECURITY_REPORT";

type SmokeReport = {
  imageLoaded: boolean;
  protocolMediaLoaded: boolean;
  customEmojiLoaded: boolean;
  videoLoaded: boolean;
  sidecarCreated: boolean;
  sidecarHiddenDuringPreview: boolean;
  sidecarRestored: boolean;
  sidecarClosed: boolean;
  cspViolationCount: number;
};

export function ReleaseWebviewSmokeApp({ baseUrl }: { baseUrl: string }) {
  const fixtureRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    const fixture = fixtureRef.current;
    if (!fixture) return;
    let cancelled = false;
    void runSmoke(fixture, baseUrl).catch((error) => {
      if (!cancelled) console.error("AWAYUKI_WEBVIEW_SECURITY_ERROR", error);
    });
    return () => {
      cancelled = true;
      void invokeTypedCommand("close_sidecar_webview", { sidecarId: SIDECAR_ID }).catch(
        () => undefined,
      );
    };
  }, [baseUrl]);

  return (
    <div
      ref={fixtureRef}
      data-testid="release-webview-smoke"
      className="fixed inset-0 z-[9999] bg-base-100 p-6"
    >
      <h1 className="text-xl font-bold">Awayuki release WebView smoke</h1>
    </div>
  );
}

async function runSmoke(fixture: HTMLDivElement, baseUrl: string) {
  const report: SmokeReport = {
    imageLoaded: false,
    protocolMediaLoaded: false,
    customEmojiLoaded: false,
    videoLoaded: false,
    sidecarCreated: false,
    sidecarHiddenDuringPreview: false,
    sidecarRestored: false,
    sidecarClosed: false,
    cspViolationCount: 0,
  };
  let cspViolationCount = 0;
  const recordCspViolation = () => {
    cspViolationCount += 1;
  };
  window.addEventListener("securitypolicyviolation", recordCspViolation);
  try {
    const protocolImages = await Promise.all(
      ["mastodon", "misskey", "paon", "bluesky"].map((protocol) =>
        loadImage(`${baseUrl}/${protocol}-media.png`, `${protocol} remote image`),
      ),
    );
    fixture.append(...protocolImages);
    const image = protocolImages[0];
    report.imageLoaded = true;
    report.protocolMediaLoaded = true;

    const emoji = await loadImage(`${baseUrl}/emoji.png`, "custom emoji");
    emoji.className = "custom-emoji h-8 w-8";
    fixture.append(emoji);
    report.customEmojiLoaded = true;

    const video = await loadVideo(`${baseUrl}/video.mp4`);
    fixture.append(video);
    report.videoLoaded = true;

    await invokeTypedCommand("create_sidecar_webview", {
      request: {
        sidecarId: SIDECAR_ID,
        url: `${baseUrl}/sidecar.html`,
        userStyle: "",
        x: Math.max(0, window.innerWidth - 360),
        y: 80,
        width: 320,
        height: 240,
      },
    });
    report.sidecarCreated = true;
    const sidecar = await waitForSidecar();
    await sidecar.show();

    const preview = document.createElement("div");
    preview.dataset.releaseMediaPreview = "open";
    preview.className = "fixed inset-0 z-[10000] bg-black";
    preview.append(image.cloneNode(true));
    document.body.append(preview);
    await sidecar.hide();
    report.sidecarHiddenDuringPreview = true;
    await nextPaint();

    preview.remove();
    await sidecar.show();
    report.sidecarRestored = true;
    await nextPaint();

    await invokeTypedCommand("close_sidecar_webview", { sidecarId: SIDECAR_ID });
    report.sidecarClosed = true;
    await nextPaint();
    report.cspViolationCount = cspViolationCount;
    await invokeTypedCommand("report_release_webview_smoke", { report });
    console.info(`${REPORT_PREFIX} ${JSON.stringify(report)}`);
  } finally {
    window.removeEventListener("securitypolicyviolation", recordCspViolation);
    if (!report.sidecarClosed) {
      await invokeTypedCommand("close_sidecar_webview", { sidecarId: SIDECAR_ID }).catch(
        () => undefined,
      );
    }
  }
}

function loadImage(src: string, alt: string) {
  return new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image();
    image.alt = alt;
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`image failed to load: ${src}`));
    image.src = src;
  });
}

function loadVideo(src: string) {
  return new Promise<HTMLVideoElement>((resolve, reject) => {
    const video = document.createElement("video");
    video.muted = true;
    video.preload = "auto";
    video.playsInline = true;
    video.onloadeddata = () => resolve(video);
    video.onerror = () => reject(new Error(`video failed to load: ${src}`));
    video.src = src;
    video.load();
  });
}

async function waitForSidecar() {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const webview = await Webview.getByLabel(sidecarWebviewLabel(SIDECAR_ID));
    if (webview) return webview;
    await new Promise((resolve) => window.setTimeout(resolve, 20));
  }
  throw new Error("native sidecar WebView was not observable");
}

function nextPaint() {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}
