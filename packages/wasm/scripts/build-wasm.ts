import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const packageRoot = process.cwd();
const repoRoot = resolve(packageRoot, "..", "..");
const outDir = resolve(packageRoot, "pkg");
const wasmOutput = resolve(outDir, "latlng_core_bg.wasm");
const optimizedWasmOutput = resolve(outDir, "latlng_core_bg.opt.wasm");
const wasmInput = resolve(
  repoRoot,
  "target",
  "wasm32-unknown-unknown",
  "release",
  "latlng_core.wasm",
);

run("cargo", [
  "build",
  "--release",
  "--target",
  "wasm32-unknown-unknown",
  "-p",
  "latlng-core",
  "--features",
  "wasm-browser-bindings",
], repoRoot);

if (!existsSync(wasmInput)) {
  throw new Error(`Missing Rust wasm output: ${wasmInput}`);
}

rmSync(outDir, { force: true, recursive: true });
mkdirSync(outDir, { recursive: true });

run("wasm-bindgen", [
  "--target",
  "web",
  "--out-dir",
  outDir,
  "--out-name",
  "latlng_core",
  wasmInput,
], packageRoot);

const beforeBytes = statSync(wasmOutput).size;
run("wasm-opt", ["-Oz", wasmOutput, "-o", optimizedWasmOutput], packageRoot);
renameSync(optimizedWasmOutput, wasmOutput);
const afterBytes = statSync(wasmOutput).size;
console.log(
  `wasm-opt -Oz reduced latlng_core_bg.wasm from ${formatBytes(beforeBytes)} to ${formatBytes(afterBytes)}`,
);

const generatedJs = resolve(outDir, "latlng_core.js");
const source = readFileSync(generatedJs, "utf8");
writeFileSync(
  generatedJs,
  source.replace(
    "        module_or_path = new URL('latlng_core_bg.wasm', import.meta.url);\n",
    "        throw new Error('latlng wasm initialization requires an explicit wasm URL');\n",
  ),
);

function formatBytes(bytes: number): string {
  return `${(bytes / 1024).toFixed(1)} KiB`;
}

function run(command: string, args: string[], cwd: string): void {
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    env: process.env,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with ${result.status}`);
  }
}
