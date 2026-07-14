import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const [outputArg = "build/dependency-metrics.json", baselineArg] = process.argv.slice(2);
const manifestPath = resolve("Cargo.toml");
const manifest = readFileSync(manifestPath, "utf8");
const manifestData = manifest
  .split("\n")
  .map((line) => line.replace(/^\s*#.*$|\s+#.*$/, ""))
  .join("\n");
const baselinePath = baselineArg ? resolve(baselineArg) : null;
const baseline = baselinePath
  ? JSON.parse(readFileSync(baselinePath, "utf8"))
  : null;

const metadata = cargoJson(["metadata", "--locked", "--format-version", "1"]);
const tree = cargoText(["tree", "--locked", "-e", "features", "--prefix", "none"]);
const direct = directFeatureSurface(manifestData);
const current = {
  tauriFeatures: direct.tauri.features,
  tokioFeatures: direct.tokio.features,
  tokioUsesFull: direct.tokio.features.includes("full"),
  reqwestFeatures: direct.reqwest.features,
  reqwestDefaultFeatures: direct.reqwest.defaultFeatures,
  unpinnedGitDependencies: [...manifestData.matchAll(/git\s*=\s*"[^"]+"(?![^\n}]*rev\s*=)/g)].length,
  updaterDependencyReferences: (manifestData.match(/sparkle-updater|winsparkle/gi) ?? []).length,
  enabledFeatureGraphLines: tree.split("\n").filter(Boolean).length,
  resolvedPackages: metadata.packages.length,
};

const required = {
  tauriDevtoolsEnabled: current.tauriFeatures.includes("devtools"),
  tokioFullDisabled: !current.tokioUsesFull,
  reqwestDefaultsDisabled: !current.reqwestDefaultFeatures,
  gitDependenciesPinned: current.unpinnedGitDependencies === 0,
  osStoreUpdaterDependenciesRemoved: current.updaterDependencyReferences === 0,
};
const failures = Object.entries(required)
  .filter(([, passed]) => !passed)
  .map(([name]) => `dependency feature policy failed: ${name}`);

const report = {
  schemaVersion: 1,
  fixtureId: "awayuki-dependency-features-v1",
  generatedAt: new Date().toISOString(),
  environment: {
    platform: process.platform,
    arch: process.arch,
    cargo: cargoText(["--version"]).trim(),
  },
  before: baseline?.before ?? baseline?.current ?? null,
  current,
  delta: baseline
    ? {
        tauriNamedFeatures:
          current.tauriFeatures.length -
          (baseline.before ?? baseline.current).tauriFeatures.length,
        tokioBroadFeatureRemoved:
          (baseline.before ?? baseline.current).tokioUsesFull &&
          !current.tokioUsesFull,
        reqwestDefaultsRemoved:
          (baseline.before ?? baseline.current).reqwestDefaultFeatures &&
          !current.reqwestDefaultFeatures,
        unpinnedGitDependencies:
          current.unpinnedGitDependencies -
          (baseline.before ?? baseline.current).unpinnedGitDependencies,
        updaterDependencyReferences:
          current.updaterDependencyReferences -
          (baseline.before ?? baseline.current).updaterDependencyReferences,
      }
    : null,
  required,
};
const output = resolve(outputArg);
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}

function directFeatureSurface(contents) {
  return Object.fromEntries(
    ["tauri", "tokio", "reqwest"].map((name) => {
      const line = contents.match(new RegExp(`^${name}\\s*=\\s*\\{([^\\n]+)\\}`, "m"))?.[1];
      if (!line) throw new Error(`missing direct dependency: ${name}`);
      const features = line
        .match(/features\s*=\s*\[([^\]]*)\]/)?.[1]
        ?.split(",")
        .map((value) => value.trim().replace(/^"|"$/g, ""))
        .filter(Boolean) ?? [];
      return [
        name,
        {
          features,
          defaultFeatures: !/default-features\s*=\s*false/.test(line),
        },
      ];
    }),
  );
}

function cargoJson(args) {
  return JSON.parse(cargoText(args));
}

function cargoText(args) {
  const result = spawnSync("cargo", args, {
    cwd: resolve("."),
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(
      `cargo ${args.join(" ")} failed: ${result.stderr || result.stdout}`,
    );
  }
  return result.stdout;
}
