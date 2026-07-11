import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const [currentPath, baselinePath, outputPath] = process.argv.slice(2);
if (!currentPath || !baselinePath) {
  console.error("usage: compare-performance.mjs CURRENT BASELINE [OUTPUT]");
  process.exit(2);
}

const current = JSON.parse(readFileSync(resolve(currentPath), "utf8"));
const baseline = JSON.parse(readFileSync(resolve(baselinePath), "utf8"));
if (JSON.stringify(current.dataset) !== JSON.stringify(baseline.dataset)) {
  const report = {
    schemaVersion: 1,
    compatible: false,
    skippedReason: "performance datasets do not match",
    currentDataset: current.dataset,
    baselineDataset: baseline.dataset,
  };
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (outputPath) writeFileSync(resolve(outputPath), serialized);
  console.log(serialized);
  process.exit(0);
}

const latencyRatioLimit = 1.5;
const sizeRatioLimit = 1.25;
const comparisons = Object.fromEntries(
  Object.entries(current.results).map(([name, result]) => {
    const baselineResult = baseline.results[name];
    if (!baselineResult) throw new Error(`baseline result is missing: ${name}`);
    const ratio = result.p95Ms / Math.max(baselineResult.p95Ms, 0.001);
    const ratioEnforced = baselineResult.p95Ms >= 5;
    return [
      name,
      {
        currentP95Ms: result.p95Ms,
        baselineP95Ms: baselineResult.p95Ms,
        ratio: Number(ratio.toFixed(3)),
        ratioLimit: latencyRatioLimit,
        ratioEnforced,
        passed: !ratioEnforced || ratio <= latencyRatioLimit,
      },
    ];
  }),
);

const databaseSizeRatio = current.databaseBytes / baseline.databaseBytes;
const report = {
  schemaVersion: 1,
  compatible: true,
  currentEnvironment: current.environment,
  baselineEnvironment: baseline.environment,
  sameEnvironment:
    current.environment.platform === baseline.environment.platform &&
    current.environment.arch === baseline.environment.arch,
  comparisons,
  databaseSize: {
    currentBytes: current.databaseBytes,
    baselineBytes: baseline.databaseBytes,
    ratio: Number(databaseSizeRatio.toFixed(3)),
    ratioLimit: sizeRatioLimit,
    passed: databaseSizeRatio <= sizeRatioLimit,
  },
};

const serialized = `${JSON.stringify(report, null, 2)}\n`;
if (outputPath) writeFileSync(resolve(outputPath), serialized);
console.log(serialized);

const failures = [
  ...Object.entries(comparisons)
    .filter(([, comparison]) => !comparison.passed)
    .map(
      ([name, comparison]) =>
        `${name} p95 ratio ${comparison.ratio} exceeds ${comparison.ratioLimit}`,
    ),
  ...(report.databaseSize.passed
    ? []
    : [
        `database size ratio ${report.databaseSize.ratio} exceeds ${report.databaseSize.ratioLimit}`,
      ]),
];
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}
