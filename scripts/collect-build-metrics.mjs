import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const [outputArg, platform, binaryArg, packageArg, compileMsArg, packageMsArg] =
  process.argv.slice(2);
if (!outputArg || !platform || !binaryArg) {
  console.error(
    "usage: collect-build-metrics.mjs OUTPUT PLATFORM BINARY [PACKAGE|-] [COMPILE_MS] [PACKAGE_MS]",
  );
  process.exit(2);
}

const output = resolve(outputArg);
const binary = resolve(binaryArg);
const packagePath = packageArg && packageArg !== "-" ? resolve(packageArg) : null;
if (!existsSync(binary)) throw new Error(`Rust binary is missing: ${binary}`);
if (packagePath && !existsSync(packagePath)) {
  throw new Error(`package is missing: ${packagePath}`);
}

const binaryBudget = 120 * 1024 * 1024;
const packageBudget = 300 * 1024 * 1024;
const compileMs = finiteNumber(compileMsArg);
const packageMs = finiteNumber(packageMsArg);
const bundlePath = resolve("build/bundle-metrics.json");
const bundle = existsSync(bundlePath)
  ? JSON.parse(readFileSync(bundlePath, "utf8"))
  : null;

const metrics = {
  rustBinaryBytes: metric(statSync(binary).size, "bytes", binaryBudget, {
    mode: "enforce",
    maxRatio: 1.15,
    noiseFloor: 1024 * 1024,
  }),
};
if (packagePath) {
  metrics.packageBytes = metric(statSync(packagePath).size, "bytes", packageBudget, {
    mode: "enforce",
    maxRatio: 1.15,
    noiseFloor: 1024 * 1024,
  });
}
if (compileMs !== null) {
  metrics.cleanCompileMs = metric(compileMs, "ms", null, {
    mode: "trend",
    maxRatio: 1.5,
    noiseFloor: 30_000,
  });
}
if (packageMs !== null) {
  metrics.packageBuildMs = metric(packageMs, "ms", null, {
    mode: "trend",
    maxRatio: 1.5,
    noiseFloor: 30_000,
  });
}
for (const name of [
  "initialRaw",
  "initialGzip",
  "initialBrotli",
  "totalJavaScriptRaw",
  "largestChunkRaw",
]) {
  if (!bundle?.summary || !Number.isFinite(bundle.summary[name])) continue;
  metrics[`bundle.${name}`] = metric(
    bundle.summary[name],
    "bytes",
    bundle.budgets?.[name] ?? null,
    { mode: "enforce", maxRatio: 1.15, noiseFloor: 32 * 1024 },
  );
}

const failures = Object.entries(metrics)
  .filter(([, value]) => value.absolute?.passed === false)
  .map(
    ([name, value]) =>
      `${name}: ${value.value}${value.unit} exceeds ${value.absolute.max}${value.unit}`,
  );

const report = {
  schemaVersion: 1,
  fixtureId: `awayuki-build-v1-${platform}`,
  environment: {
    platform,
    arch: process.arch,
    runtime: `bun ${Bun.version}`,
  },
  inputs: {
    binary,
    package: packagePath,
    bundleMetrics: bundle ? bundlePath : null,
  },
  metrics,
};
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}

function finiteNumber(value) {
  if (value === undefined || value === "" || value === "-") return null;
  const number = Number(value);
  if (!Number.isFinite(number) || number < 0) {
    throw new Error(`invalid duration: ${value}`);
  }
  return Math.round(number);
}

function metric(value, unit, absoluteMax, regression) {
  return {
    value,
    unit,
    absolute:
      absoluteMax === null
        ? { mode: "trend" }
        : { max: absoluteMax, passed: value <= absoluteMax },
    regression: { direction: "lower", ...regression },
  };
}
