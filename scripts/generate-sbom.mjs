import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { basename, resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const [outputArg = "build/awayuki.spdx.json", versionArg, commitArg] =
  process.argv.slice(2);
const output = resolve(outputArg);
const version = versionArg ?? JSON.parse(readFileSync(resolve(root, "package.json"))).version;
const commit = commitArg ?? "unknown";
const packages = inTreePackages().concat(cargoPackages(), npmPackages());
const documentId = `https://github.com/mohemohe/awayuki-desktop/sbom/${version}/${commit}`;

const document = {
  spdxVersion: "SPDX-2.3",
  dataLicense: "CC0-1.0",
  SPDXID: "SPDXRef-DOCUMENT",
  name: `Awayuki-${version}`,
  documentNamespace: documentId,
  creationInfo: {
    created: new Date(
      Number(process.env.SOURCE_DATE_EPOCH ?? 0) * 1000,
    ).toISOString(),
    creators: ["Tool: awayuki-generate-sbom"],
  },
  packages,
  relationships: packages.map((pkg) => ({
    spdxElementId: "SPDXRef-DOCUMENT",
    relationshipType: "DESCRIBES",
    relatedSpdxElement: pkg.SPDXID,
  })),
};

writeFileSync(output, `${JSON.stringify(document, null, 2)}\n`);
console.log(`wrote ${output} with ${packages.length} packages`);

function inTreePackages() {
  const starryEyesRevision = "a2c4c9b68287c9058d82a15cd28c6615863a626f";
  const starryEyesOrigin = "https://github.com/karno/StarryEyes";
  return [
    {
      name: "StarryEyes Krile Query (KQ)",
      SPDXID: "SPDXRef-Package-StarryEyes-KQ",
      versionInfo: starryEyesRevision,
      downloadLocation: `git+https://github.com/karno/StarryEyes.git@${starryEyesRevision}#StarryEyes/Filters`,
      filesAnalyzed: false,
      licenseConcluded: "MIT",
      licenseDeclared: "MIT",
      copyrightText: "Copyright (c) 2013 Karno.",
      homepage: starryEyesOrigin,
      sourceInfo: `Awayuki's in-tree KQ implementation is derived from Karno and StarryEyes contributors at audited commit ${starryEyesRevision}.`,
    },
  ];
}

function cargoPackages() {
  const lock = readFileSync(resolve(root, "Cargo.lock"), "utf8");
  return lock
    .split("[[package]]")
    .slice(1)
    .map((block) => {
      const name = field(block, "name");
      const packageVersion = field(block, "version");
      const source = field(block, "source");
      const checksum = field(block, "checksum");
      return spdxPackage("cargo", name, packageVersion, source, checksum);
    });
}

function npmPackages() {
  const manifest = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
  return Object.entries({
    ...manifest.dependencies,
    ...manifest.devDependencies,
  }).map(([name, packageVersion]) =>
    spdxPackage("npm", name, String(packageVersion), "npm", undefined),
  );
}

function spdxPackage(ecosystem, name, packageVersion, source, checksum) {
  const identity = `${ecosystem}:${name}:${packageVersion}:${source ?? "local"}`;
  const digest = createHash("sha256").update(identity).digest("hex").slice(0, 20);
  return {
    name,
    SPDXID: `SPDXRef-Package-${digest}`,
    versionInfo: packageVersion,
    downloadLocation: source ?? "NOASSERTION",
    filesAnalyzed: false,
    licenseConcluded: "NOASSERTION",
    licenseDeclared: "NOASSERTION",
    externalRefs: [
      {
        referenceCategory: "PACKAGE-MANAGER",
        referenceType: "purl",
        referenceLocator: `pkg:${ecosystem}/${encodeURIComponent(name)}@${encodeURIComponent(packageVersion)}`,
      },
    ],
    ...(checksum
      ? { checksums: [{ algorithm: "SHA256", checksumValue: checksum }] }
      : {}),
  };
}

function field(block, name) {
  const value = block.match(new RegExp(`^${name} = "([^"]+)"`, "m"))?.[1];
  if (!value && (name === "name" || name === "version")) {
    throw new Error(`Cargo.lock package is missing ${name}`);
  }
  return value;
}
