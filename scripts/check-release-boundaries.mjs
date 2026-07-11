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

if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log("release workflow pinning and secret boundaries verified");
