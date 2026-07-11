import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const files = [join(root, "README.md"), join(root, "CLAUDE.md"), ...markdownFiles(join(root, "docs"))];
const failures = [];

for (const file of files) {
  const contents = readFileSync(file, "utf8");
  for (const match of contents.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)) {
    const target = match[1].trim().replace(/^<|>$/g, "");
    if (/^(?:https?:|mailto:|#)/.test(target)) continue;
    const path = decodeURIComponent(target.split("#", 1)[0]);
    if (!path) continue;
    if (!existsSync(resolve(dirname(file), path))) {
      failures.push(`${file.slice(root.length + 1)}: missing link target ${target}`);
    }
  }
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(`validated links in ${files.length} Markdown files`);
}

function markdownFiles(directory) {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) return markdownFiles(path);
    return name.endsWith(".md") ? [path] : [];
  });
}
