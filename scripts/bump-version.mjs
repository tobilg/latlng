import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import process from "node:process";

const version = process.argv[2];

if (!version) {
  fail("Usage: node scripts/bump-version.mjs x.y.z");
}

if (!isValidVersion(version)) {
  fail(`Invalid version "${version}". Expected semver like 0.1.0 or 1.2.3-beta.1`);
}

const repoRoot = process.cwd();

updateCargoWorkspaceVersion(resolve(repoRoot, "Cargo.toml"), version);
updatePackageVersion(resolve(repoRoot, "packages", "sdk", "package.json"), version);
updatePackageLockVersion(
  resolve(repoRoot, "packages", "sdk", "package-lock.json"),
  "@latlng/sdk",
  version,
);
updatePackageVersion(resolve(repoRoot, "packages", "wasm", "package.json"), version);
updatePackageLockVersion(
  resolve(repoRoot, "packages", "wasm", "package-lock.json"),
  "@latlng/wasm",
  version,
);

console.log(`Bumped workspace and package versions to ${version}`);

function updateCargoWorkspaceVersion(filePath, nextVersion) {
  const source = readFileSync(filePath, "utf8");
  const lines = source.split("\n");
  let inWorkspacePackage = false;
  let updatedVersion = false;

  const updated = lines
    .map((line) => {
      const trimmed = line.trim();

      if (trimmed === "[workspace.package]") {
        inWorkspacePackage = true;
        return line;
      }

      if (inWorkspacePackage && trimmed.startsWith("[") && trimmed !== "[workspace.package]") {
        inWorkspacePackage = false;
      }

      if (inWorkspacePackage && trimmed.startsWith('version = "')) {
        updatedVersion = true;
        return line.replace(/version = "[^"]+"/, `version = "${nextVersion}"`);
      }

      return line;
    })
    .join("\n");

  if (!updatedVersion) {
    fail(`Could not find [workspace.package] version in ${filePath}`);
  }

  writeFileSync(filePath, updated);
}

function updatePackageVersion(filePath, nextVersion) {
  const json = JSON.parse(readFileSync(filePath, "utf8"));
  json.version = nextVersion;
  writeJson(filePath, json);
}

function updatePackageLockVersion(filePath, packageName, nextVersion) {
  const json = JSON.parse(readFileSync(filePath, "utf8"));
  json.name = packageName;
  json.version = nextVersion;
  json.packages ??= {};
  json.packages[""] ??= {};
  json.packages[""].name = packageName;
  json.packages[""].version = nextVersion;
  writeJson(filePath, json);
}

function writeJson(filePath, json) {
  writeFileSync(filePath, `${JSON.stringify(json, null, 2)}\n`);
}

function isValidVersion(value) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(value);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
