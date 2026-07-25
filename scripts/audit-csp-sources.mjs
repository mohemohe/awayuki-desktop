import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const [outputArg = "build/csp-source-inventory.json"] = process.argv.slice(2);
const root = resolve(new URL("..", import.meta.url).pathname);
const config = JSON.parse(readFileSync(resolve(root, "tauri.conf.json"), "utf8"));
const csp = config?.app?.security?.csp ?? "";
const directives = Object.fromEntries(
  csp
    .split(";")
    .map((value) => value.trim())
    .filter(Boolean)
    .map((value) => {
      const [name, ...sources] = value.split(/\s+/);
      return [name, sources];
    }),
);

const evidence = [
  sourceEvidence(
    "connect-src",
    "ipc:",
    "frontend/src/api/tauri.ts",
    /tauriInvoke(?:<|\()/,
  ),
  sourceEvidence(
    "connect-src",
    "http://ipc.localhost",
    "frontend/src/api/tauri.ts",
    /@tauri-apps\/api\/core/,
  ),
  sourceEvidence(
    "img-src",
    "http: https:",
    "frontend/src/utils/media.ts",
    /media\.remote_url[\s\S]*media\.preview_url[\s\S]*media\.url/,
  ),
  sourceEvidence(
    "img-src",
    "data:",
    "frontend/src/utils/blurhash.ts",
    /canvas\.toDataURL\("image\/png"\)/,
  ),
  sourceEvidence(
    "img-src/media-src",
    "blob:",
    "frontend/src/features/compose/useComposeMediaQueue.ts",
    /URL\.createObjectURL\(file\)/,
  ),
  sourceEvidence(
    "media-src",
    "http: https:",
    "frontend/src/utils/media.ts",
    /if \(video\)[\s\S]*media\.remote_url[\s\S]*media\.url/,
  ),
  sourceEvidence(
    "style-src",
    "'unsafe-inline'",
    "frontend/src/features/timeline/TimelineMedia.tsx",
    /style=\{placeholderStyle\}/,
  ),
  sourceEvidence(
    "sidecar",
    "separate native WebView",
    "src/application/desktop.rs",
    /WebviewBuilder::new\(label\.clone\(\), WebviewUrl::External\(url\)\)/,
  ),
];

const expected = {
  "default-src": ["'self'"],
  "base-uri": ["'none'"],
  "object-src": ["'none'"],
  "form-action": ["'none'"],
  "frame-src": ["'none'"],
  "img-src": ["'self'", "http:", "https:", "data:", "blob:"],
  "media-src": ["'self'", "http:", "https:", "blob:"],
  "font-src": ["'self'"],
  "style-src": ["'self'", "'unsafe-inline'"],
  "script-src": ["'self'"],
  "connect-src": ["ipc:", "http://ipc.localhost"],
};

const failures = [];
for (const [directive, sources] of Object.entries(expected)) {
  const actual = directives[directive] ?? [];
  if (JSON.stringify(actual) !== JSON.stringify(sources)) {
    failures.push(
      `${directive} source inventory changed: ${actual.join(" ")} != ${sources.join(" ")}`,
    );
  }
}
for (const forbidden of ["asset:", "data:"]) {
  if (forbidden === "asset:") {
    for (const directive of ["img-src", "media-src"]) {
      if ((directives[directive] ?? []).includes(forbidden)) {
        failures.push(`${directive} retains unused ${forbidden}`);
      }
    }
  } else if ((directives["media-src"] ?? []).includes(forbidden)) {
    failures.push("media-src retains unused data:");
  }
}
if ((directives["font-src"] ?? []).includes("data:")) {
  failures.push("font-src retains unused data:");
}
for (const item of evidence) {
  if (!item.matched) failures.push(`missing source evidence: ${item.file}`);
}

const report = {
  schemaVersion: 1,
  fixtureId: "awayuki-csp-source-inventory-v1",
  csp,
  directives,
  evidence,
  removedUnusedSources: {
    imageAssetProtocol: true,
    mediaAssetProtocol: true,
    mediaDataUrl: true,
    fontDataUrl: true,
  },
  mainWebviewExternalConnect: false,
  sidecarSharesMainDocumentCsp: false,
};
const output = resolve(outputArg);
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));

if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}

function sourceEvidence(directive, source, file, pattern) {
  const content = readFileSync(resolve(root, file), "utf8");
  return { directive, source, file, matched: pattern.test(content) };
}
