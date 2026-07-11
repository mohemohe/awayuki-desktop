import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const read = (path) => readFileSync(resolve(root, path), "utf8");
const failures = [];

const cargo = read("Cargo.toml");
const cargoLock = read("Cargo.lock");
for (const dependency of [
  "keyring",
  "secret-service",
  "security-framework",
  "sparkle-updater",
  "winsparkle-updater",
]) {
  if (new RegExp(`^${dependency}\\s*=`, "m").test(cargo)) {
    failures.push(`forbidden OS-state dependency in Cargo.toml: ${dependency}`);
  }
}
for (const packageName of [
  "keyring",
  "secret-service",
  "windows-credentials",
  "sparkle-updater",
  "sparkle-sys",
  "winsparkle-sys",
]) {
  if (new RegExp(`^name = "${packageName}"$`, "m").test(cargoLock)) {
    failures.push(`forbidden OS credential package in Cargo.lock: ${packageName}`);
  }
}
if (/windows-updater\s*=/.test(cargo)) {
  failures.push("registry-backed Windows updater feature must not be restored");
}

if (existsSync(resolve(root, "src/updater.rs"))) {
  failures.push("an updater module may persist state outside awayuki.db");
}
const infoPlist = read("resources/Info.plist");
if (/SUFeedURL|SUPublicEDKey|SUEnableAutomaticChecks/.test(infoPlist)) {
  failures.push("Sparkle preferences must not be configured in Info.plist");
}

const databasePool = read("src/db/pool.rs");
if (/VACUUM\s+INTO|create_pre_migration_backup|backup_path/.test(databasePool)) {
  failures.push("automatic migration backup/recovery files are forbidden");
}
if (existsSync(resolve(root, "docs/database-recovery.md"))) {
  failures.push("OS/pre-release recovery documentation must not be restored");
}

const credentialStore = read("src/auth/credential_store.rs");
const loginModel = read("src/db/models.rs");
const settingsQueries = read("src/db/queries/settings.rs");
for (const column of ["access_token", "app_password"]) {
  if (!loginModel.includes(column) || !settingsQueries.includes(column)) {
    failures.push(`SQLite credential column is missing from the active model/query: ${column}`);
  }
}
if (!credentialStore.includes("CredentialStore::sqlite") && !credentialStore.includes("pub fn sqlite")) {
  failures.push("credential store is not explicitly SQLite-backed");
}

for (const path of [
  "README.md",
  "docs/architecture.md",
  "docs/adr/0001-sqlite-only-portable-state.md",
]) {
  const contents = read(path);
  if (!contents.includes("awayuki.db") || !/SQLite/i.test(contents)) {
    failures.push(`portable SQLite contract is missing from ${path}`);
  }
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exit(1);
}

console.log("SQLite-only portable-state contract verified");
