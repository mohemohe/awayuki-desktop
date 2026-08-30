import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import { brotliCompressSync, gzipSync } from "node:zlib";

const root = new URL("..", import.meta.url).pathname;
const dist = join(root, "frontend", "dist");
const manifest = JSON.parse(
  readFileSync(join(dist, ".vite", "manifest.json"), "utf8"),
);

const budgets = {
  initialRaw: 650 * 1024,
  initialGzip: 210 * 1024,
  initialBrotli: 185 * 1024,
  // The full Unicode emoji catalog is intentionally deferred and compresses
  // to less than 70 KiB, but its minified raw representation is ~940 KiB.
  // Account-scoped Bluesky feed discovery adds a typed IPC surface and a
  // reusable provider-resource selector to the deferred Settings bundle.
  totalJavaScriptRaw: 2110 * 1024,
  largestChunkRaw: 1000 * 1024,
};

const files = [];
function visit(directory) {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) visit(path);
    else if (name.endsWith(".js") || name.endsWith(".css")) files.push(path);
  }
}
visit(dist);

const metrics = Object.fromEntries(
  files.map((path) => {
    const contents = readFileSync(path);
    return [
      relative(dist, path),
      {
        raw: contents.byteLength,
        gzip: gzipSync(contents, { level: 9 }).byteLength,
        brotli: brotliCompressSync(contents).byteLength,
      },
    ];
  }),
);

const initialFiles = new Set();
function addManifestEntry(key) {
  const entry = manifest[key];
  if (!entry || initialFiles.has(entry.file)) return;
  initialFiles.add(entry.file);
  for (const imported of entry.imports ?? []) addManifestEntry(imported);
  for (const css of entry.css ?? []) initialFiles.add(css);
}
for (const [key, entry] of Object.entries(manifest)) {
  if (entry.isEntry) addManifestEntry(key);
}

const sum = (field, selected) =>
  [...selected].reduce((total, file) => total + (metrics[file]?.[field] ?? 0), 0);
const javascriptFiles = Object.keys(metrics).filter((file) => file.endsWith(".js"));
const summary = {
  initialFiles: [...initialFiles].sort(),
  initialRaw: sum("raw", initialFiles),
  initialGzip: sum("gzip", initialFiles),
  initialBrotli: sum("brotli", initialFiles),
  totalJavaScriptRaw: sum("raw", javascriptFiles),
  largestChunkRaw: Math.max(0, ...javascriptFiles.map((file) => metrics[file].raw)),
};

mkdirSync(join(root, "build"), { recursive: true });
writeFileSync(
  join(root, "build", "bundle-metrics.json"),
  `${JSON.stringify({ budgets, summary, files: metrics }, null, 2)}\n`,
);

const failures = Object.entries(budgets)
  .filter(([name, limit]) => summary[name] > limit)
  .map(
    ([name, limit]) =>
      `${name}: ${summary[name]} bytes exceeds the ${limit} byte budget`,
  );
const productionMockMarkers = [
  "Mock IPC command is not implemented",
  "placehold.co/",
  "mock-account",
];
for (const path of files.filter((file) => file.endsWith(".js"))) {
  const contents = readFileSync(path, "utf8");
  for (const marker of productionMockMarkers) {
    if (contents.includes(marker)) {
      failures.push(
        `${relative(dist, path)} contains development mock marker ${JSON.stringify(marker)}`,
      );
    }
  }
}
console.log(JSON.stringify(summary, null, 2));
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}
