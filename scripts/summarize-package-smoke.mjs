import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const [outputArg = "build/package-smoke-summary.json", ...reportArgs] =
  process.argv.slice(2);
if (!reportArgs.length) {
  console.error(
    "usage: summarize-package-smoke.mjs OUTPUT REPORT [REPORT ...]",
  );
  process.exit(2);
}

const reports = reportArgs.map((path) =>
  JSON.parse(readFileSync(resolve(path), "utf8")),
);
const requiredPlatforms = new Set(["macos", "windows", "linux"]);
for (const report of reports) {
  if (report.fixtureId !== "awayuki-package-smoke-v1") {
    throw new Error(`unexpected package fixture: ${report.fixtureId}`);
  }
  if (report.result !== "passed") {
    throw new Error(`${report.platform} package smoke did not pass`);
  }
  requiredPlatforms.delete(report.platform);
}
if (requiredPlatforms.size) {
  throw new Error(
    `package smoke was not executed for: ${[...requiredPlatforms].join(", ")}`,
  );
}

const summary = {
  schemaVersion: 1,
  fixtureId: "awayuki-package-smoke-summary-v1",
  result: "passed",
  platforms: Object.fromEntries(
    reports
      .sort((left, right) => left.platform.localeCompare(right.platform))
      .map((report) => [
        report.platform,
        {
          result: report.result,
          artifact: report.artifact,
          tests: report.tests,
          database: report.database,
          buildMetrics: report.buildMetrics,
        },
      ]),
  ),
};
const output = resolve(outputArg);
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(summary, null, 2)}\n`);

const markdown = [
  "## Package smoke matrix",
  "",
  "| OS | fresh DB | legacy upgrade | restart | uninstall binary | DB retained | SQLite-only | CSP policy | media | sidecar preview | CSP report |",
  "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
  ...Object.entries(summary.platforms).map(([platform, value]) => {
    const pass = (condition) => (condition ? "pass" : "fail");
    return `| ${platform} | ${pass(value.tests.freshDatabaseLaunch)} | ${pass(
      value.tests.legacyDatabaseUpgrade,
    )} | ${pass(value.tests.upgradedDatabaseRestart)} | ${pass(
      value.tests.uninstallRemovedBinary,
    )} | ${pass(value.tests.uninstallPreservedDatabase)} | ${pass(
      value.tests.sqliteOnlyStatePreserved,
    )} | ${pass(value.tests.releaseSecurityAttested)} | ${pass(
      value.tests.remoteImageLoaded &&
        value.tests.protocolMediaLoaded &&
        value.tests.customEmojiLoaded &&
        value.tests.remoteVideoLoaded,
    )} | ${pass(value.tests.sidecarPreviewHideRestore)} | ${pass(
      value.tests.cspViolationReportClean,
    )} |`;
  }),
  "",
].join("\n");
writeFileSync(output.replace(/\.json$/, ".md"), markdown);
console.log(markdown);
