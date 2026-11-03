# WKWebView image decoder lifetime fixture

This macOS-only experiment uses an independent WKWebView with a nonpersistent
data store, a loopback HTTP server, and generated GIFs. It never opens Awayuki's
database, accounts, or running WebViews. Requires Python 3 and Xcode command line
tools; no Python packages are needed.

Run the variants **sequentially**, keeping the fixture window visible:

```sh
rtk proxy python3 scripts/fixtures/image-decoder/run-probe.py auto animated
rtk proxy python3 scripts/fixtures/image-decoder/run-probe.py sync animated
rtk proxy python3 scripts/fixtures/image-decoder/run-probe.py auto static
```

`--output-root /absolute/path` selects another artifact directory. The default is
`build/image-decoder-investigation/`. Compilation, module caches, synthetic GIFs,
screenshots, and process samples remain in that ignored build directory. macOS
process inspection and opening the native window require permission outside a
restricted execution sandbox.

Each run displays three batches of 50 distinct image URLs, removes each batch,
then leaves an empty document for 15 seconds. There are never more than 50 image
elements in the DOM. GIFs are 256 by 256 pixels; the animated variant has eight
frames. Sampling counts `org.webkit.ImageDecoder` work queues, rather than all
libdispatch threads. This distinction matters: idle reusable dispatch threads
alone are not evidence of an image decoder leak. The fixture deliberately stays
well below the 512-thread exhaustion point and has a 90-second native deadline
plus an independent 95-second supervisor deadline.

`<mode>-<kind>/results.json` records DOM count, visibility, decoder queues,
physical footprint, and WebContent PID at each stage. Raw `.sample.txt` files
contain the stack and OS/WebKit versions. The process is selected only when
exactly one new WebContent process appears after starting this fixture; an
ambiguous selection fails instead of sampling an unrelated application.

Two native WKWebView snapshots are taken 220 ms apart during the first displayed
batch. `*-animation-pixels.json` compares decoded RGBA pixels, excluding PNG
metadata differences. A nonzero result for the animated sync variant, alongside
zero image decoder queues, verifies that synchronous decoding still animates.
The static control should have zero changed pixels. The fixture does not call
`HTMLImageElement.decode()`, which can select a different decoding path.

The lifetime defect is timing dependent: not every removal leaves a worker.
Counts left after an empty DOM are the useful result; exact counts are not an
assertion. This experiment tests image worker lifetime on the installed WebKit,
not a 65-hour soak or a forced GPU-process failure.
