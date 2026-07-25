import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const config = JSON.parse(readFileSync(resolve(root, "tauri.conf.json"), "utf8"));
const policy = readFileSync(resolve(root, "docs/security/csp-policy.md"), "utf8");
const csp = config?.app?.security?.csp ?? "";
const directives = new Map(
  csp
    .split(";")
    .map((directive) => directive.trim())
    .filter(Boolean)
    .map((directive) => {
      const [name, ...sources] = directive.split(/\s+/);
      return [name, sources];
    }),
);
const failures = [];
for (const directive of ["base-uri", "object-src", "form-action", "frame-src"]) {
  if (directives.get(directive)?.join(" ") !== "'none'") {
    failures.push(`${directive} must remain 'none'`);
  }
}
if (directives.get("script-src")?.join(" ") !== "'self'") {
  failures.push("script-src must allow only 'self'");
}
const connect = directives.get("connect-src") ?? [];
if (
  connect.length !== 2 ||
  !connect.includes("ipc:") ||
  !connect.includes("http://ipc.localhost")
) {
  failures.push("connect-src must contain only the two Tauri IPC sources");
}
const expectedMediaSources = {
  "img-src": ["'self'", "http:", "https:", "data:", "blob:"],
  "media-src": ["'self'", "http:", "https:", "blob:"],
  "font-src": ["'self'"],
};
for (const [directive, expected] of Object.entries(expectedMediaSources)) {
  const sources = directives.get(directive) ?? [];
  if (JSON.stringify(sources) !== JSON.stringify(expected)) {
    failures.push(`${directive} must equal ${expected.join(" ")}`);
  }
}
for (const phrase of ["削除条件", "CSP report", "Sidecar", "credential", "SQLite"]) {
  if (!policy.includes(phrase)) failures.push(`CSP policy is missing review evidence: ${phrase}`);
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log("main WebView CSP and exception-removal policy verified");
