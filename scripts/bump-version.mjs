import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import process from "node:process";

const rawVersion = process.argv[2];
const version = rawVersion?.replace(/^v/, "");

if (!rawVersion) {
  fail("Usage: node scripts/bump-version.mjs x.y.z");
}

if (!isValidVersion(version)) {
  fail(`Invalid version "${version}". Expected semver like 0.1.0 or 1.2.3-beta.1`);
}

const repoRoot = process.cwd();

updateCargoWorkspaceVersion(resolve(repoRoot, "Cargo.toml"), version);
updateCargoLockWorkspaceVersions(
  resolve(repoRoot, "Cargo.lock"),
  readWorkspacePackageNames(repoRoot),
  version,
);
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
updatePackageVersion(resolve(repoRoot, "packages", "example-wasm", "package.json"), version);
updatePackageLockVersion(
  resolve(repoRoot, "packages", "example-wasm", "package-lock.json"),
  "@latlng/example-wasm",
  version,
  [{ path: "../wasm", name: "@latlng/wasm" }],
);

console.log(`Bumped workspace and package versions to ${version}`);

function readWorkspacePackageNames(rootDir) {
  const cargoToml = readFileSync(resolve(rootDir, "Cargo.toml"), "utf8");
  const membersMatch = cargoToml.match(/members\s*=\s*\[([\s\S]*?)\]/);
  if (!membersMatch) {
    fail("Could not find [workspace] members in Cargo.toml");
  }

  return membersMatch[1]
    .split("\n")
    .map((line) => line.replace(/#.*/, "").trim())
    .map((line) => line.match(/^"([^"]+)"/)?.[1])
    .filter(Boolean)
    .map((memberPath) => {
      const memberToml = readFileSync(resolve(rootDir, memberPath, "Cargo.toml"), "utf8");
      const name = memberToml.match(/^name\s*=\s*"([^"]+)"/m)?.[1];
      if (!name) {
        fail(`Could not find package name in ${memberPath}/Cargo.toml`);
      }
      return name;
    });
}

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

function updateCargoLockWorkspaceVersions(filePath, packageNames, nextVersion) {
  const workspaceNames = new Set(packageNames);
  const source = readFileSync(filePath, "utf8");
  const blocks = source.split(/(?=\[\[package\]\]\n)/);
  let updatedCount = 0;

  const updated = blocks
    .map((block) => {
      const name = block.match(/^name = "([^"]+)"/m)?.[1];
      if (!name || !workspaceNames.has(name)) {
        return block;
      }

      updatedCount += 1;
      return block.replace(/^version = "[^"]+"/m, `version = "${nextVersion}"`);
    })
    .join("");

  if (updatedCount !== workspaceNames.size) {
    fail(
      `Updated ${updatedCount} Cargo.lock workspace packages, expected ${workspaceNames.size}`,
    );
  }

  writeFileSync(filePath, updated);
}

function updatePackageVersion(filePath, nextVersion) {
  const json = JSON.parse(readFileSync(filePath, "utf8"));
  json.version = nextVersion;
  writeJson(filePath, json);
}

function updatePackageLockVersion(filePath, packageName, nextVersion, linkedPackages = []) {
  const json = JSON.parse(readFileSync(filePath, "utf8"));
  json.name = packageName;
  json.version = nextVersion;
  json.packages ??= {};
  json.packages[""] ??= {};
  json.packages[""].name = packageName;
  json.packages[""].version = nextVersion;

  for (const linkedPackage of linkedPackages) {
    if (!json.packages[linkedPackage.path]) {
      continue;
    }
    json.packages[linkedPackage.path].name = linkedPackage.name;
    json.packages[linkedPackage.path].version = nextVersion;
  }

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
