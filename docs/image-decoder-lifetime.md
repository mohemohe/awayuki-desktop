# Animated-image decoder lifetime investigation

## Incident: 2026-11-03 (JST)

The main Awayuki WebView had run for about 65 hours. Its document stopped painting
reliably while scrollbars and the separate X sidecar remained usable. A live
five-second `sample` of main WebContent PID 37578 at 17:29:49 found:

- 511 serial `org.webkit.ImageDecoder` queues blocked in
  `WebCore::ImageFrameWorkQueue::start()` → `WTF::Condition::waitUntilUnchecked()`.
- The dispatch worker limit of 512 reached in all 398 samples.
- Physical footprint 2.8 GB, peak 3.0 GB. RSS alone understated this footprint.

The unified log also recorded repeated main-view
`RemoteRenderingBackendProxy::didBecomeUnresponsive` messages and GPU-process
termination/recreation. These are observations of the failing native rendering
boundary, not evidence of an application JavaScript heap leak or an OOM kill.

The existing watchdog requested reload at 17:29:33, recreated the main WebView at
17:30:33, and reported resumed JavaScript/frame progress at 17:30:58. Nevertheless,
a second sample at 17:34:13 still found the old PID 37578 alive, with 511 decoder
queues, the dispatch limit reached in all 56 samples, and a 2.8 GB footprint.
The replacement had a new WebContent PID. Restored responsiveness therefore did
not prove reclamation of the old native resources; stronger stall detection
alone would not address the worker-lifetime problem.

The queues were waiting for another image-frame request, rather than downloading
an image or actively spending those five seconds decoding one. Their native
ownership explains why bounded timeline entities and removal of DOM nodes do not
by themselves bound the decoder workers.

Environment: macOS 27.0.1 (26A434), WebKit 22625.1.29.11.28. An exact public source
tag for this installed binary was not found. The source analysis below uses
immutable upstream commit `7b43a563bdda9384d9d6c5b6cd73daa7ea822907`; its functions
match the sampled stack. The same ownership/stop design is also present in
Apple's published `WebKit-7624.2.5.11.4` source. This is not a claim that either
source revision exactly reproduces the installed binary.

## Why the waiting workers survive image removal

1. `ImageFrameWorkQueue::start()` submits one long-lived dispatch work item for
   each asynchronously decoded image. It repeatedly dequeues frame requests,
   blocking when none remain. The closure holds strong references to the queue,
   image source, decoder, and request queue. `stop()` must close that request
   queue to wake the worker and let the closure finish. An empty queue is not
   automatically a completed dispatch work item.
   ([ImageFrameWorkQueue.cpp](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/ImageFrameWorkQueue.cpp#L55-L140))

2. Removing the last rendered client invokes `CachedImage::allClientsRemoved()`.
   It calls `resetAnimation()`; it does not close the decoding queue.
   ([CachedImage.cpp](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/loader/cache/CachedImage.cpp#L229-L235))

3. `BitmapImageSource::resetAnimation()` delegates to the animator. Its reset
   clears the animation timer and frame counters. Its decoded-data cleanup
   returns immediately below a 30 MiB decoded-animation threshold. Crucially,
   this path is different from `BitmapImageSource::stopAnimation()`, which tries
   to stop the work queue.
   ([ImageFrameAnimator.cpp](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/ImageFrameAnimator.cpp#L64-L75),
   [animation reset](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/ImageFrameAnimator.cpp#L172-L187),
   [BitmapImageSource.cpp](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/BitmapImageSource.cpp#L294-L309))

4. The completion path has an additional cleanup opportunity:
   `CachedImage::imageFrameAvailable()` calls `stopAnimation()` if no client is
   visible. However, `BitmapImageSource::stopAnimation()` returns immediately
   when the animator has no timer. `resetAnimation()` has already cleared that
   timer, so a completion after removal can miss this cleanup as well.
   ([frame notification](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/loader/cache/CachedImage.cpp#L696-L716),
   [timer state](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/ImageFrameAnimator.h#L54-L58))

5. Even an explicit `stopDecodingWorkQueue()` returns while decoding requests
   remain pending. On completion, the source stops idle queues for requests
   marked non-animating; an animation-frame completion does not perform that
   cleanup. Depending on removal/decode timing, an animated image can therefore
   leave a waiting worker whose strong references retain native resources after
   its DOM/JavaScript references disappear. This does not imply that every image
   removal leaks a worker. JavaScript garbage collection cannot release the
   retained native closure.
   ([queue stop](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/BitmapImageSource.cpp#L344-L357),
   [completion](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/BitmapImageSource.cpp#L439-L462))

This mechanism agrees with [WebKit bug 308730](https://bugs.webkit.org/show_bug.cgi?id=308730),
where a WebKit engineer confirmed the same dispatch-limit failure. As checked on
2026-11-03, the bug remained open. The proposed
[upstream change #77264](https://github.com/WebKit/WebKit/pull/77264) replaces the
indefinite per-image worker with finite requests and a bounded shared scheduler;
it was still open. Neither a proposed upstream fix nor a matching signature
establishes which particular image first triggered this incident.

## Application-side decoding boundary

`<img decoding="sync">` is honored before WebKit's automatic animated-image
policy. In the inspected source, automatic decoding becomes asynchronous for an
animated image whose intrinsic width × height × 4 exceeds 100 KiB. Rendered CSS
size does not determine that threshold; an emoji displayed at 20 px may still
have a much larger source image.
([RenderBoxModelObject.cpp](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/rendering/RenderBoxModelObject.cpp#L339-L363),
[size threshold](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/BitmapImageSource.cpp#L338-L342))

For synchronous painting, `ImageFrameAnimator::startAnimation()` still schedules
the animation timer, but does not enqueue the next frame on `ImageFrameWorkQueue`.
This bypasses the problematic worker lifetime while retaining animation. It
trades asynchronous frame decoding for synchronous decode work during painting;
it is not a fix to the system WebKit scheduler.
([ImageFrameAnimator.cpp](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/ImageFrameAnimator.cpp#L123-L156))

Coverage must include avatars, React custom emoji, HTML-generated inline emoji,
timeline thumbnails, full-size image previews, and temporary retry images.
Extension-based GIF detection alone misses animated WebP/AVIF and URLs without
extensions. `HTMLImageElement.decode()` takes a separate asynchronous source
path even when the painting hint is synchronous; the current retry probe uses
load/error events instead of `decode()`.
([explicit decode path](https://github.com/WebKit/WebKit/blob/7b43a563bdda9384d9d6c5b6cd73daa7ea822907/Source/WebCore/platform/graphics/BitmapImageSource.cpp#L372-L403))

## Runtime validation

An isolated native WKWebView on the same host loaded three batches of 50 distinct
256 × 256, eight-frame GIFs (150 total), then removed each batch. At most 50 image
nodes existed at once. Images were synthetic, served locally; no account data or
normal Awayuki storage was used. Each stage was sampled with macOS `sample`.

| Variant | Decoder queues while showing each batch | After each removal | After 15 seconds with no images |
| --- | --- | --- | --- |
| Animated, default decoding | 50 / 50 / 63 | 0 / 13 / 17 | 17 |
| Same animated images, `sync` | 0 / 0 / 0 | 0 / 0 / 0 | 0 |
| Static images, default decoding | 0 / 0 / 0 | 0 / 0 / 0 | 0 |

The automatic variant reproduced accumulation beyond the live DOM count without
reaching the system thread limit. Its retained stacks matched the incident's
`ImageFrameWorkQueue::start()` waiting path. The synchronous variant was repeated
with the same zero-queue result. Two native snapshots 220 ms apart changed
319,984 of 585,000 pixels, confirming ongoing animation with no decoder queues. The static control changed
zero pixels and retained zero decoder queues.
This isolates decoding policy as the relevant difference; it does not identify
the first production image involved or replace a 65-hour soak test.

Raw samples, counts, and synthetic snapshots are local ignored artifacts under
`build/image-decoder-investigation/`. See the [native fixture instructions](../scripts/fixtures/image-decoder/README.md)
for repeating all three variants. The native fixture uses ImageIO-generated GIFs
and requires no downloaded image assets or third-party Python modules.

The application now sets its shared `IMAGE_DECODING` policy before `src` for all
nine image-creation paths, including serialized HTML emoji and detached retry
probes. Tests preserve animated URLs, retry behavior and assignment ordering.
Synchronous decoding can increase an individual paint's latency for large
images; it avoids the demonstrated unbounded native worker lifetime rather than
claiming that all image decoding becomes faster.

## Change verification (2026-11-03)

- Frontend: 44 focused regression tests across seven files, TypeScript checking,
  ESLint, and production Vite build passed.
- Rust: 22 watchdog tests and 15 smoke/storage tests passed.
- The isolated `intermittent` native smoke passed in 122.1 seconds: three short
  stalls caused one reload with `repeated-slow-javascript`, a fresh document and
  resumed frame progress. The window, sidecar and main-below-sidecar ordering
  were preserved. This verifies the secondary recovery fix, not image lifetime.
  Report: `build/renderer-recovery/2026-11-03-intermittent/webview-recovery-smoke.json`.
- The ARM64 release bundle was built at
  `build/image-decoder-fix-release/Awayuki.app`; deep/strict ad-hoc signature
  verification passed. The running normal app was not replaced by this build.
- `git diff --check` passed. `docs:check` still reports pre-existing missing
  workflow links in unchanged `docs/tasks/refactoring/08-implementation-report.md`.

The synthetic image test proves the worker-creation difference on this installed
WebKit and verifies animated pixels. It is not a 65-hour run of the repaired
Awayuki application. Large-image paint latency and other WebKit failure modes
remain distinct from this demonstrated native worker-retention defect.
