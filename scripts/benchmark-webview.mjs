import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

if (process.platform !== "darwin") {
  throw new Error("the local WebView performance fixture currently requires macOS");
}

const output = resolve(process.argv[2] ?? "build/webview-performance.json");
const benchmarkMode = process.env.AWAYUKI_BENCHMARK_MODE === "startup" ? "startup" : "full";
const version = /^version = "([^"]+)"/m.exec(readFileSync("Cargo.toml", "utf8"))?.[1];
if (!version) throw new Error("Cargo package version is missing");

const scratch = mkdtempSync(join(tmpdir(), "awayuki-webview-performance-"));
const scratchBuild = join(scratch, "build");
const copiedApp = join(scratchBuild, "Awayuki.app");
let launcher;
let appPid = 0;

try {
  const build = Bun.spawn(["bash", "scripts/build-app-bundle.sh"], {
    cwd: process.cwd(),
    env: {
      ...process.env,
      VERSION: version,
      VITE_PERFORMANCE_SMOKE: benchmarkMode === "startup" ? "startup" : "1",
      BUILD_DIR: scratchBuild,
    },
    stdout: "inherit",
    stderr: "inherit",
  });
  if ((await build.exited) !== 0) throw new Error("performance app build failed");

  const executableDirectory = join(copiedApp, "Contents", "MacOS");
  writeFileSync(join(executableDirectory, "PORTABLE"), "");
  const executable = join(executableDirectory, "awayuki");
  const executableRealPath = realpathSync(executable);
  const stdoutPath = join(scratch, "awayuki.stdout.log");
  const stderrPath = join(scratch, "awayuki.stderr.log");
  writeFileSync(stdoutPath, "");
  writeFileSync(stderrPath, "");
  launcher = Bun.spawn([
    "open",
    "-W",
    "-n",
    "-F",
    "-a",
    copiedApp,
    "-o",
    stdoutPath,
    "--stderr",
    stderrPath,
    "--env",
    "AWAYUKI_PERFORMANCE_SMOKE=1",
    "--env",
    "RUST_LOG=awayuki=info",
  ], {
    cwd: executableDirectory,
    stdout: "inherit",
    stderr: "inherit",
  });

  let peakRssBytes = 0;
  const timeoutAt = Date.now() + 120_000;
  let marker;
  while (Date.now() < timeoutAt) {
    const status = await Promise.race([
      launcher.exited.then((code) => ({ exited: true, code })),
      Bun.sleep(50).then(() => ({ exited: false, code: 0 })),
    ]);
    if (appPid === 0) {
      const pgrep = Bun.spawnSync(["pgrep", "-n", "-f", executableRealPath]);
      appPid = Number(new TextDecoder().decode(pgrep.stdout).trim()) || 0;
      if (appPid === 0) {
        const byName = Bun.spawnSync(["pgrep", "-n", "-x", "awayuki"]);
        appPid = Number(new TextDecoder().decode(byName.stdout).trim()) || 0;
      }
    }
    if (appPid !== 0) {
      const rss = Bun.spawnSync(["ps", "-o", "rss=", "-p", String(appPid)]);
      const rssKiB = Number(new TextDecoder().decode(rss.stdout).trim());
      if (Number.isFinite(rssKiB)) peakRssBytes = Math.max(peakRssBytes, rssKiB * 1024);
    }
    const combined = `${readFileSync(stdoutPath, "utf8")}\n${readFileSync(stderrPath, "utf8")}`;
    marker = combined.match(/AWAYUKI_PERFORMANCE_REPORT (\{[^\r\n]+\})/);
    if (marker) break;
    if (status.exited) {
      break;
    }
  }
  const combined = `${readFileSync(stdoutPath, "utf8")}\n${readFileSync(stderrPath, "utf8")}`;
  mkdirSync(resolve("build"), { recursive: true });
  writeFileSync(resolve("build/webview-performance.log"), combined);
  if (appPid !== 0) process.kill(appPid, "SIGKILL");
  launcher.kill();
  if (!marker) {
    throw new Error(`performance report marker was not emitted:\n${combined.slice(-4_000)}`);
  }
  const frontend = JSON.parse(marker[1]);
  const report = {
    schemaVersion: 1,
    fixtureId: frontend.fixtureId,
    environment: {
      platform: process.platform,
      arch: process.arch,
      appVersion: version,
      webviewUserAgent: frontend.userAgent,
    },
    process: { peakRssBytes },
    startup: {
      ...frontend.startup,
      firstInteractiveAfterVisibilityMs: Math.max(
        0,
        frontend.startup.firstInteractiveMs - frontend.stream.visibilityWaitMs,
      ),
    },
    ...(frontend.render ? { render: frontend.render } : {}),
    stream: frontend.stream,
  };

  const failures = [];
  if (benchmarkMode === "full") {
    if (report.stream.displayedStatuses !== 1_000) {
      failures.push("timeline performance fixture retention changed");
    }
    if (report.render.timelineStream.frameSampleCount === 0) {
      failures.push("stream frame samples are missing");
    }
    if (report.render.timelineStream.commits === 0) failures.push("stream commits are missing");
    if (report.render.timelineStream.frameP95DurationMs > 50) {
      failures.push(`stream frame p95 ${report.render.timelineStream.frameP95DurationMs}ms exceeds 50ms`);
    }
    if (report.render.timelineScroll.frameSampleCount === 0) {
      failures.push("scroll frame samples are missing");
    }
    if (report.render.profileOpen.commits === 0) failures.push("profile commits are missing");
    if (report.render.profileOpen.frameSampleCount === 0) {
      failures.push("profile frame samples are missing");
    }
  }
  if (report.startup.firstReactCommitMs === 0 || report.startup.firstInteractiveMs === 0) {
    failures.push("startup interactive milestones are missing");
  }
  if (peakRssBytes === 0) failures.push("process RSS was not observed");

  mkdirSync(resolve(output, ".."), { recursive: true });
  writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
  if (failures.length) throw new Error(failures.join("\n"));
} finally {
  if (appPid !== 0) {
    try {
      process.kill(appPid, "SIGKILL");
    } catch {
      // The benchmark app has already exited.
    }
  }
  launcher?.kill();
  rmSync(scratch, { recursive: true, force: true });
  const restore = Bun.spawnSync(["bun", "run", "build"], {
    cwd: process.cwd(),
    stdout: "inherit",
    stderr: "inherit",
  });
  if (restore.exitCode !== 0) {
    console.error("failed to restore the normal frontend build after performance smoke");
  }
}
