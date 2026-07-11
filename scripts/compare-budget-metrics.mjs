import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const [currentPath, baselinePath, outputPath] = process.argv.slice(2);
if (!currentPath || !baselinePath) {
  console.error(
    "usage: compare-budget-metrics.mjs CURRENT BASELINE [OUTPUT]",
  );
  process.exit(2);
}

const current = JSON.parse(readFileSync(resolve(currentPath), "utf8"));
const baseline = JSON.parse(readFileSync(resolve(baselinePath), "utf8"));
if (current.fixtureId !== baseline.fixtureId) {
  throw new Error(
    `metric fixtures do not match: ${current.fixtureId} != ${baseline.fixtureId}`,
  );
}

const comparisons = {};
const failures = [];
for (const [name, metric] of Object.entries(current.metrics ?? {})) {
  const baselineMetric = baseline.metrics?.[name];
  if (!baselineMetric) {
    throw new Error(`baseline metric is missing: ${name}`);
  }
  if (metric.unit !== baselineMetric.unit) {
    throw new Error(`metric unit changed for ${name}`);
  }

  const policy = metric.regression ?? {};
  const direction = policy.direction ?? "lower";
  const baselineMagnitude = Math.abs(baselineMetric.value);
  const noiseFloor = policy.noiseFloor ?? 0;
  const ratio =
    direction === "higher"
      ? baselineMetric.value / Math.max(metric.value, Number.EPSILON)
      : metric.value / Math.max(baselineMetric.value, Number.EPSILON);
  const ratioEnforced =
    policy.mode === "enforce" && baselineMagnitude >= noiseFloor;
  const ratioLimit = policy.maxRatio ?? 1.5;
  const passed = !ratioEnforced || ratio <= ratioLimit;
  comparisons[name] = {
    current: metric.value,
    baseline: baselineMetric.value,
    unit: metric.unit,
    direction,
    ratio: Number(ratio.toFixed(3)),
    ratioLimit,
    ratioEnforced,
    mode: policy.mode ?? "trend",
    passed,
  };
  if (!passed) {
    failures.push(
      `${name} regression ratio ${ratio.toFixed(3)} exceeds ${ratioLimit}`,
    );
  }
}

const report = {
  schemaVersion: 1,
  fixtureId: current.fixtureId,
  currentEnvironment: current.environment,
  baselineEnvironment: baseline.environment,
  sameRunnerClass:
    current.environment?.platform === baseline.environment?.platform &&
    current.environment?.arch === baseline.environment?.arch,
  comparisons,
};
const serialized = `${JSON.stringify(report, null, 2)}\n`;
if (outputPath) writeFileSync(resolve(outputPath), serialized);
console.log(serialized);

if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
}
