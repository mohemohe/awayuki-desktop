# macOS WebView blank-screen investigation

## Incident evidence (2026-09-28, Asia/Tokyo)

The reported state retained the four timeline scrollbars and the native X
sidecar, while the main document (including the title bar, composer, column
headers and status bar) disappeared after a long idle period. The user reported
that the app had still worked immediately after waking from sleep.

The macOS unified log identifies the failed boundary. These are excerpts with
addresses and unrelated metadata omitted:

```text
12:10:26 WebContent[40978] RemoteImageBufferSetProxyFlusher::flushAndCollectHandlers - failed
12:10:26 WebContent[40978] RemoteRenderingBackendProxy::didBecomeUnresponsive
12:10:26 GPUProcessProxy::didBecomeUnresponsive: GPUProcess with PID 40977 became unresponsive, terminating it
12:10:26 GPUProcessProxy::gpuProcessExited: reason=Unresponsive
...
12:22:21 WebContent[40978] RemoteRenderingBackendProxy::didBecomeUnresponsive
12:22:36 WebContent[40978] RemoteImageBufferSetProxyFlusher::flushAndCollectHandlers - failed
12:24:35 WebContent[40978] Current memory footprint: 2312 MB
12:24:56 [PID=40978] WebProcessProxy::didBecomeUnresponsive
12:25:08 [PID=40978] WebPageProxy::processDidBecomeUnresponsive
12:26:43 WebContent[71145] Current memory footprint: 707 MB
12:26:53 App termination approved
```

There were 19 GPU-process exits with `reason=Unresponsive` between 12:10 and
12:25. The main WebContent process remained present instead of terminating;
the sidecar continued responding. This explains why Awayuki's existing
`on_web_content_process_terminate` callback could not recover this incident.
High memory usage was observed, but the record does **not** establish an OOM
kill, a particular image, or an application memory leak as the GPU-hang trigger.
The environment was macOS 27.0 (26A428), Tauri 2.11.2 and wry 0.55.1.

An operational scrollbar is not a JavaScript/render-thread health check.
[WebKit's macOS scrolling explanation](https://trac.webkit.org/wiki/WhatsChangingwithscrollingonmacOS)
documents separate scrolling-thread commits and scrollbar layers, including
when the main thread is unresponsive.

## Boundaries checked

- The composer, title bar and status bar are outside the virtual timeline;
  a Virtuoso-only rendering failure does not explain their disappearance.
- The root/workspace has no shared opacity, transform, filter,
  `content-visibility` or paint-containment rule that hides the document.
- React's existing error boundary displays a visible fallback. JavaScript
  errors in event handlers/promises are now forwarded through the same bounded,
  redacted log path, since they do not reach that boundary.
- Streamed entities/canonical indexes are pruned. Visible/deferred timeline
  retention, media retry queues, caches and render-metric samples are bounded.
  No idle-stream leak explaining this incident was demonstrated.
- The main and sidecar views are separate native WebViews. The main view is
  opaque; changing CSS z-index cannot repair the native rendering backend.
- The termination callback was already installed and connected through Tauri
  to WKWebView. Adding another crash-only reload would miss the observed hang.

## Recovery design

Recovery is owned by the native application, so it can run while the main
document's JavaScript or animation frames are stalled. The watchdog observes
both eval completion and a bounded animation-frame heartbeat. It is not a
pixel comparison and cannot detect every possible compositor corruption.

Only the main WebView is monitored. Visibility/occlusion, minimization and
sleep/scheduling gaps must reset the timeout instead of treating normal
background throttling as a failure. A prolonged visible stall requests a
native WKWebView reload after 45 seconds, with a 15-second startup/resume grace
period and a 120-second cooldown. Stale-callback generations and one outstanding
evaluation prevent queued probes from accumulating against a hung process.

If reload still has not recovered after another 60 visible seconds, the native
application recreates the main WebView once. It preserves the NSWindow and
sidecar WebViews, places the replacement below the sidecars, and restores its
resize, dropped-file and appropriate keyboard-focus behavior. Old delegates
are disconnected before the `main` label is reused. No private WebKit APIs or
external process termination are used.

Draft recovery data belongs to the native process's memory only. It must not
be logged or create a second persistent store. Restoring a draft does not
submit it. Transient media or other state that cannot safely survive a reload
must block automatic recovery. The backend, database and network services
remain running during a main-view reload.

The frontend restores once per document and receives an opaque generation token.
All checkpoint writes include that token. Reserving recovery invalidates it
under the same lock that protects the checkpoint: a delayed old-document write
cannot reopen the recovery gate or falsely acknowledge submission protection.
The new document stays protected until it checkpoints its restored state.
Writes are coalesced, limited to 1 MiB, and acknowledge submission protection
before a post/edit request starts. Settings, login, in-flight submissions and
attachments that still depend on a local blob prevent automatic recovery.
The second-stage recreation consumes the same recovery reservation once. As
soon as a new document restores, that reservation loses permission to recreate
it. Conversely, animation frames from an old document alone cannot cancel the
fallback while its checkpoint generation remains invalidated.

This repairs the missing recovery path around a WebKit failure; it does not
claim to repair macOS/WebKit's GPU implementation. Real fault-injection and
long-idle observation have different proof boundaries from unit tests.

The real WKWebView tests also exposed a limit of `reload()`: the initial
reload-only implementation recovered stopped animation frames, but an infinite
JavaScript loop never loaded a new document within the initial 150-second deadline.
That failure requires a second recovery stage rather than treating a successful
`reload()` return as proof of recovery.

## Repeating the native fault-injection test

The macOS debug build includes an opt-in test harness. The release build excludes
the module and its environment-variable handling. Build the frontend and debug
binary, then run from a fresh directory with both variables set:

```sh
AWAYUKI_WEBVIEW_RECOVERY_SMOKE=frames \
AWAYUKI_WEBVIEW_RECOVERY_SMOKE_CWD="$PWD" \
  /absolute/path/to/debug/awayuki
```

`frames` stops animation-frame callbacks; `finite` runs JavaScript for 70 seconds;
`infinite` hangs JavaScript until the native test deadline. The harness requires
an explicit matching working directory and creates a new private `debug_data`
directory; it rejects existing storage. It never opens normal account data.

After healthy frame progress and at least 25 seconds, the harness injects the
fault. Passing requires exactly one ordinary watchdog recovery with the expected
stall reason, a newly loaded document, and resumed JavaScript/frame progress.
The process writes `webview-recovery-smoke.json` and exits; an independent native
210-second deadline bounds failed tests. A small native sidecar fixture also
checks that the window and sidecar survive and the replacement main view stays
below it. This proves the WKWebView recovery path,
not that a spontaneous GPU hang was reproduced or that every painted pixel is
correct.

## Verification (2026-09-28)

- Frontend: 78 test files / 428 tests passed; typecheck, lint and Vite build
  passed. The existing large-chunk warning remains.
- Rust: 449 tests passed, 1 benchmark test ignored. The generated IPC contract,
  portable-storage, startup, architecture and CSP checks passed.
- The ordinary ARM64 release app was built separately at
  `build/renderer-recovery/release/Awayuki.app`. Deep/strict code-signature
  verification passed. The debug fault-injection entry is absent from that
  release executable. The running user's app was not replaced or restarted.
- The final stopped-animation-frame test passed in 100.6 seconds with one reload
  and no recreation, also preserving the native window, sidecar and ordering.
  Evidence is under `build/renderer-recovery/smoke-frames-final/`.
- The final 70-second JavaScript-stop test passed in 125.6 seconds with one
  reload and no recreation. The native window, sidecar and ordering were
  preserved. Evidence is under `build/renderer-recovery/smoke-finite-final/`.
- The final infinite-JavaScript test passed in 192.1 seconds: one reload,
  one main-WebView recreation, a new document with live JavaScript and frame
  progress, the same NSWindow and sidecar, and main below the sidecar.
  Evidence is under `build/renderer-recovery/smoke-infinite-final/`.

The controlled WKWebView tests use isolated debug app instances. The initial
login view was inspected visually, but Computer Use timed out when capturing
the recovered view; final pixel appearance was not visually verified. They do not
reproduce the original long-idle GPU fault itself, prove that its upstream cause
has disappeared, or constitute a long-duration soak test. The fix covers the
demonstrated missing recovery path and preserves resumable composition state.

Unrelated existing gates remain failing: `docs:check` references removed workflow
files from `docs/tasks/refactoring/08-implementation-report.md`; `release:check`
rejects the existing unpinned/duplicated workflow setup. Those workflow files and
the release checker are unchanged from HEAD.

## Collecting a recurrence

Use the app's normal debug log for `awayuki::webview` watchdog events and
`[awayuki][ui]` errors. Logs record health counters/reasons, never recovery
draft contents. OS evidence can be inspected without changing app settings:

```sh
log show --style compact --last 30m --predicate \
  'process == "awayuki" AND (eventMessage CONTAINS[c] "Unresponsive" OR eventMessage CONTAINS[c] "flushAndCollectHandlers" OR eventMessage CONTAINS[c] "gpuProcessExited")'
```

Record whether the window was visible, the last successful eval/frame,
whether recovery was blocked by transient state, and whether the native reload
completed. A crash report alone is insufficient: the observed main process
did not crash.
