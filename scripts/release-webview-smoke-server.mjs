import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const portFile = process.argv[2];
if (!portFile) throw new Error("usage: release-webview-smoke-server.mjs PORT_FILE");

const root = fileURLToPath(new URL("..", import.meta.url));
const image = readFileSync(resolve(root, "assets/icons/AppIcon.png"));
const video = Uint8Array.from(
  Buffer.from(
    readFileSync(resolve(root, "scripts/fixtures/security-smoke-video.mp4.b64"), "utf8").replace(/\s+/g, ""),
    "base64",
  ),
);
const sidecar = new TextEncoder().encode(
  "<!doctype html><meta charset=utf-8><title>Awayuki sidecar smoke</title><body>sidecar-ready</body>",
);

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request) {
    const path = new URL(request.url).pathname;
    if (
      path === "/emoji.png" ||
      /^\/(?:mastodon|misskey|paon|bluesky)-media\.png$/.test(path)
    ) {
      return new Response(image, { headers: { "content-type": "image/png", "cache-control": "no-store" } });
    }
    if (path === "/video.mp4") {
      return new Response(video, { headers: { "content-type": "video/mp4", "cache-control": "no-store" } });
    }
    if (path === "/sidecar.html") {
      return new Response(sidecar, { headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" } });
    }
    return new Response("not found", { status: 404 });
  },
});
writeFileSync(resolve(portFile), String(server.port));
console.log(`release WebView smoke server listening on 127.0.0.1:${server.port}`);
