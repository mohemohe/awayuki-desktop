import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const workflows = join(root, ".github", "workflows");
const failures = [];
const contentsByName = Object.fromEntries(
  readdirSync(workflows)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .map((name) => [name, readFileSync(join(workflows, name), "utf8")]),
);
const tauriConfig = JSON.parse(readFileSync(join(root, "tauri.conf.json"), "utf8"));
const releaseSecurityRuntime = readFileSync(
  join(root, "src/application/desktop/release_security_smoke.rs"),
  "utf8",
);
const packageSmoke = readFileSync(join(root, "scripts/package-smoke.sh"), "utf8");
const packageFixture = readFileSync(
  join(root, "scripts/package-db-fixture.mjs"),
  "utf8",
);
const webviewSmoke = readFileSync(
  join(root, "frontend/src/performance/ReleaseWebviewSmokeApp.tsx"),
  "utf8",
);
const webviewSmokeServer = readFileSync(
  join(root, "scripts/release-webview-smoke-server.mjs"),
  "utf8",
);

for (const [name, contents] of Object.entries(contentsByName)) {
  for (const match of contents.matchAll(/^\s*uses:\s*([^\s#]+)(?:\s*#.*)?$/gm)) {
    const reference = match[1];
    if (reference.startsWith("./")) continue;
    if (!/@[a-f0-9]{40}$/.test(reference)) {
      failures.push(`${name} has an unpinned action reference: ${reference}`);
    }
  }
}

const sharedBuild = contentsByName["build-artifacts.yml"] ?? "";
const signingJob = sharedBuild.match(/\n  macos-sign:\n([\s\S]*?)\n  macos-smoke:\n/)?.[1];
if (!signingJob) {
  failures.push("isolated macos-sign job is missing");
} else {
  for (const forbidden of [
    "actions/checkout",
    "scripts/",
    "cargo ",
    "bun ",
    "package-smoke",
  ]) {
    if (signingJob.includes(forbidden)) {
      failures.push(`macos-sign executes untrusted source/build input: ${forbidden}`);
    }
  }
  if (!signingJob.includes("environment: production-signing")) {
    failures.push("macos-sign is not protected by the signing environment");
  }
}

for (const name of ["release.yml", "manual-build.yml"]) {
  const contents = contentsByName[name] ?? "";
  if (!contents.includes("uses: ./.github/workflows/build-artifacts.yml")) {
    failures.push(`${name} bypasses the shared artifact workflow`);
  }
  if (/^  build-(?:macos|windows|linux):/m.test(contents)) {
    failures.push(`${name} duplicates a platform build job`);
  }
}

if (!sharedBuild.includes('git merge-base --is-ancestor "$SOURCE_REF"')) {
  failures.push("shared artifact workflow does not constrain source to main ancestry");
}

if (
  !tauriConfig.app?.windows?.length ||
  tauriConfig.app.windows.some((window) => window.devtools !== true)
) {
  failures.push("every packaged WebView window must keep DevTools available for bug reports");
}
for (const marker of [
  "AWAYUKI_RELEASE_SECURITY_REPORT",
  "release_build={}",
  "csp_deny_default={}",
  "csp_external_connect={}",
  "csp_remote_media={}",
]) {
  if (!releaseSecurityRuntime.includes(marker)) {
    failures.push(`release runtime security attestation is missing: ${marker}`);
  }
}
if (!packageSmoke.includes("AWAYUKI_RELEASE_SECURITY_SMOKE=1")) {
  failures.push("package smoke does not enable release runtime security attestation");
}
for (const expected of [
  "release_build=true",
  "csp_deny_default=true",
  "csp_external_connect=false",
  "csp_remote_media=true",
]) {
  if (!packageSmoke.includes(expected) || !sharedBuild.includes("package-smoke")) {
    failures.push(`package smoke does not require security state: ${expected}`);
  }
}
if (!packageFixture.includes("releaseSecurityAttested")) {
  failures.push("package report omits the release security attestation");
}
for (const marker of [
  "imageLoaded",
  "protocolMediaLoaded",
  "customEmojiLoaded",
  "videoLoaded",
  "sidecarCreated",
  "sidecarHiddenDuringPreview",
  "sidecarRestored",
  "sidecarClosed",
  "cspViolationCount",
]) {
  if (!webviewSmoke.includes(marker) || !packageSmoke.includes(marker)) {
    failures.push(`release WebView package smoke is missing: ${marker}`);
  }
}
for (const reportField of [
  "remoteImageLoaded",
  "protocolMediaLoaded",
  "customEmojiLoaded",
  "remoteVideoLoaded",
  "sidecarPreviewHideRestore",
  "cspViolationReportClean",
]) {
  if (!packageFixture.includes(reportField)) {
    failures.push(`package report omits WebView assertion: ${reportField}`);
  }
}
for (const route of [
  "mastodon|misskey|paon|bluesky",
  "/emoji.png",
  "/video.mp4",
  "/sidecar.html",
]) {
  if (!webviewSmokeServer.includes(route)) {
    failures.push(`release WebView smoke server is missing route: ${route}`);
  }
}
if (!sharedBuild.includes("package-smoke-summary")) {
  failures.push("release does not require the 3 OS package smoke summary");
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log("release workflow pinning and secret boundaries verified");
