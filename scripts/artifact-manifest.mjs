import {
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { basename, join, relative, resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const [mode, artifactDirectoryArg, manifestPathArg, versionArg, commitArg] =
  process.argv.slice(2);
const artifactDirectory = resolve(artifactDirectoryArg ?? "artifacts");
const manifestPath = resolve(manifestPathArg ?? "artifact-manifest.json");

if (mode === "generate") generate();
else if (mode === "verify") verify();
else fail("usage: artifact-manifest.mjs generate|verify ARTIFACT_DIR MANIFEST VERSION COMMIT");

function generate() {
  if (!versionArg || !commitArg) fail("generate requires VERSION and COMMIT");
  const files = walk(artifactDirectory)
    .filter((path) => resolve(path) !== manifestPath)
    .sort();
  if (!files.length) fail("no release artifacts found");

  const manifest = {
    schemaVersion: 1,
    product: "Awayuki",
    version: versionArg.replace(/^v/, ""),
    sourceCommit: commitArg,
    toolchain: {
      rust: rustToolchain(),
      bun: JSON.parse(readFileSync(join(root, "package.json"), "utf8"))
        .packageManager,
      cargoLockSha256: sha256(join(root, "Cargo.lock")),
      bunLockSha256: sha256(join(root, "bun.lock")),
    },
    artifacts: files.map((path) => ({
      name: basename(path),
      path: relative(artifactDirectory, path),
      size: statSync(path).size,
      sha256: sha256(path),
      signature: signaturePolicy(path),
    })),
  };
  assertManifest(manifest);
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`wrote ${manifestPath} with ${manifest.artifacts.length} artifacts`);
}

function verify() {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  assertManifest(manifest);
  for (const artifact of manifest.artifacts) {
    const path = resolve(artifactDirectory, artifact.path);
    if (!existsSync(path)) fail(`missing artifact: ${artifact.path}`);
    if (statSync(path).size !== artifact.size) fail(`size mismatch: ${artifact.path}`);
    if (sha256(path) !== artifact.sha256) fail(`digest mismatch: ${artifact.path}`);
  }
  console.log(`verified ${manifest.artifacts.length} artifact digests`);
}

function assertManifest(manifest) {
  if (manifest.schemaVersion !== 1) fail("unsupported artifact manifest schema");
  if (!manifest.version?.match(/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/)) {
    fail(`invalid artifact version: ${manifest.version}`);
  }
  if (versionArg && manifest.version !== versionArg.replace(/^v/, "")) {
    fail(`manifest version ${manifest.version} does not match ${versionArg}`);
  }
  if (commitArg && manifest.sourceCommit !== commitArg) {
    fail(`manifest commit ${manifest.sourceCommit} does not match ${commitArg}`);
  }
  const names = new Set();
  for (const artifact of manifest.artifacts ?? []) {
    if (names.has(artifact.name)) fail(`duplicate artifact name: ${artifact.name}`);
    names.add(artifact.name);
    if (!artifact.sha256?.match(/^[a-f0-9]{64}$/)) fail(`invalid digest: ${artifact.name}`);
    if (!Number.isSafeInteger(artifact.size) || artifact.size < 1) {
      fail(`invalid size: ${artifact.name}`);
    }
    if (
      /\.(?:dmg|zip|AppImage|tar\.gz)$/.test(artifact.name) &&
      !artifact.name.includes(`-${manifest.version}-`)
    ) {
      fail(`artifact filename does not contain manifest version: ${artifact.name}`);
    }
    if (artifact.name.endsWith(".dmg") && artifact.signature !== "apple-codesign-notarization") {
      fail("macOS DMG must declare code signing and notarization");
    }
    if (artifact.name.includes("windows") && artifact.signature !== "disabled-unsigned") {
      fail("unsigned Windows package must declare the disabled-unsigned signature policy");
    }
  }
}

function signaturePolicy(path) {
  const name = basename(path).toLowerCase();
  if (name.endsWith(".dmg")) return "apple-codesign-notarization";
  if (name.includes("windows")) return "disabled-unsigned";
  return "sha256-manifest";
}

function rustToolchain() {
  const contents = readFileSync(join(root, "rust-toolchain.toml"), "utf8");
  return contents.match(/^channel\s*=\s*"([^"]+)"/m)?.[1] ?? "unknown";
}

function walk(directory) {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
