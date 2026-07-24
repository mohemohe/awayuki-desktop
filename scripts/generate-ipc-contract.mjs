import { spawnSync } from "node:child_process";
import process from "node:process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const check = process.argv.slice(2).includes("--check");
const args = [
  "run",
  "--quiet",
  "--locked",
  "--bin",
  "generate-ipc-contract",
  "--",
  ...(check ? ["--check"] : []),
];
const result = spawnSync("cargo", args, {
  cwd: repositoryRoot,
  stdio: "inherit",
});

if (result.error) {
  process.stderr.write(
    `failed to run IPC contract generator: ${result.error.message}\n`,
  );
  process.exit(1);
}
process.exit(result.status ?? 1);
