/**
 * WebKit bug 308730: an async animated-image decoder can retain its worker
 * and image source after the element leaves the document. Repeated timeline
 * churn can exhaust threads and stall the main WebView. Explicit Sync bypasses
 * that async animated-image path in RenderBoxModelObject; it does not disable
 * animation. Apply before src, including detached retry images and HTML emoji.
 * URLs/MIME types cannot reliably identify animated WebP/PNG previews.
 * https://bugs.webkit.org/show_bug.cgi?id=308730
 */
export const IMAGE_DECODING = "sync" as const;
