import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const cargo = readFileSync(resolve(root, "Cargo.toml"), "utf8");
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const packageVersion = JSON.parse(
  readFileSync(resolve(root, "package.json"), "utf8"),
).version;
const tauriVersion = JSON.parse(
  readFileSync(resolve(root, "tauri.conf.json"), "utf8"),
).version;
const expected = (process.argv[2] ?? process.env.VERSION ?? "").replace(/^v/, "");

const versions = {
  "Cargo.toml": cargoVersion,
  "package.json": packageVersion,
  "tauri.conf.json": tauriVersion,
};
const distinct = new Set(Object.values(versions));
const failures = [];
if (distinct.size !== 1 || distinct.has(undefined)) {
  failures.push(`version manifests disagree: ${JSON.stringify(versions)}`);
}
if (expected && cargoVersion !== expected) {
  failures.push(`release version ${expected} does not match manifests ${cargoVersion}`);
}
if (!cargoVersion?.match(/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/)) {
  failures.push(`invalid semantic version: ${cargoVersion}`);
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(`version manifests agree: ${cargoVersion}`);
}
