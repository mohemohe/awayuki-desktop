import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const failures = [];
const adrDirectory = resolve(root, "docs/adr");
const index = readFileSync(resolve(adrDirectory, "README.md"), "utf8");
const adrFiles = readdirSync(adrDirectory).filter((name) => /^\d{4}-.+\.md$/.test(name));
for (const adr of adrFiles) {
  if (!index.includes(`(${adr})`)) failures.push(`ADR index does not link ${adr}`);
}

const base = process.argv[2]?.trim();
if (base && /^[0-9a-f]{7,40}$/i.test(base)) {
  let changedFiles = [];
  try {
    changedFiles = execFileSync("git", ["diff", "--name-only", `${base}...HEAD`], {
      cwd: root,
      encoding: "utf8",
    })
      .split(/\r?\n/)
      .filter(Boolean);
  } catch (error) {
    failures.push(`could not inspect architecture diff from ${base}: ${error.message}`);
  }
  const architectureChanged = changedFiles.some((path) =>
    [
      /^migrations\//,
      /^src\/(?:api|application|auth|db|domain|ipc|services|state)\//,
      /^frontend\/src\/(?:api|domain|store)\//,
      /^(?:Cargo\.toml|package\.json|tauri\.conf\.json)$/,
    ].some((pattern) => pattern.test(path)),
  );
  const adrChanged = changedFiles.some((path) => /^docs\/adr\/\d{4}-.+\.md$/.test(path));
  if (architectureChanged && !adrChanged) {
    failures.push("architecture-sensitive changes require an ADR update in docs/adr");
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log(
  base
    ? `ADR index and architecture diff verified from ${base}`
    : `ADR index verified (${adrFiles.length} records; no comparison base)`,
);
