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

verifyExternrefTableExport(wasmOutput, "wasm-bindgen output");

const beforeBytes = statSync(wasmOutput).size;
const wasmOptArgs = [
  "-Oz",
  "--enable-reference-types",
  "--enable-bulk-memory",
  "--enable-multivalue",
  "--enable-mutable-globals",
  "--enable-nontrapping-float-to-int",
  "--enable-sign-ext",
  wasmOutput,
  "-o",
  optimizedWasmOutput,
];
try {
  run("wasm-opt", wasmOptArgs, packageRoot);
  verifyExternrefTableExport(optimizedWasmOutput, "wasm-opt output");
  renameSync(optimizedWasmOutput, wasmOutput);
  const afterBytes = statSync(wasmOutput).size;
  console.log(
    `wasm-opt ${wasmOptArgs.slice(0, -3).join(" ")} reduced latlng_core_bg.wasm from ${formatBytes(beforeBytes)} to ${formatBytes(afterBytes)}`,
  );
} catch (error) {
  rmSync(optimizedWasmOutput, { force: true });
  console.warn(
    `wasm-opt output failed validation; keeping wasm-bindgen output (${formatBytes(beforeBytes)}).`,
  );
  console.warn(error instanceof Error ? error.message : String(error));
}

verifyExternrefTableExport(wasmOutput, "final wasm");

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

function verifyExternrefTableExport(filePath: string, artifact: string): void {
  const wasm = readFileSync(filePath);
  const parser = createWasmParser(wasm);
  const tables: WasmTableType[] = [];
  let externrefsTableIndex: number | null = null;

  parser.expectMagicAndVersion();

  while (!parser.done()) {
    const sectionId = parser.readByte();
    const sectionEnd = parser.readU32() + parser.offset;

    if (sectionId === 2) {
      parseImportSection(parser, tables);
    } else if (sectionId === 4) {
      const count = parser.readU32();
      for (let index = 0; index < count; index += 1) {
        tables.push(parser.readTableType());
      }
    } else if (sectionId === 7) {
      const count = parser.readU32();
      for (let index = 0; index < count; index += 1) {
        const name = parser.readName();
        const kind = parser.readByte();
        const exportIndex = parser.readU32();
        if (name === "__wbindgen_externrefs") {
          if (kind !== 1) {
            throw new Error(
              `__wbindgen_externrefs is exported as kind ${kind}; expected table export`,
            );
          }
          externrefsTableIndex = exportIndex;
        }
      }
    }

    parser.seek(sectionEnd);
  }

  if (externrefsTableIndex === null) {
    throw new Error(`${artifact} is missing __wbindgen_externrefs table export`);
  }

  const table = tables[externrefsTableIndex];
  if (!table) {
    throw new Error(
      `${artifact} exports __wbindgen_externrefs as table[${externrefsTableIndex}], but only ${tables.length} tables were found`,
    );
  }

  if (table.elementType !== "externref") {
    throw new Error(
      `${artifact} exports __wbindgen_externrefs as table[${externrefsTableIndex}] (${table.elementType}); expected externref. Re-check wasm-opt flags.`,
    );
  }

  console.log(`verified ${artifact} __wbindgen_externrefs -> table[${externrefsTableIndex}] externref`);
}

function parseImportSection(parser: WasmParser, tables: WasmTableType[]): void {
  const count = parser.readU32();
  for (let index = 0; index < count; index += 1) {
    parser.readName();
    parser.readName();
    const kind = parser.readByte();
    if (kind === 0) {
      parser.readU32();
    } else if (kind === 1) {
      tables.push(parser.readTableType());
    } else if (kind === 2) {
      parser.readLimits();
    } else if (kind === 3) {
      parser.readByte();
      parser.readByte();
    } else {
      throw new Error(`Unsupported wasm import kind ${kind}`);
    }
  }
}

interface WasmTableType {
  elementType: string;
  min: number;
  max: number | null;
}

interface WasmParser {
  readonly offset: number;
  done(): boolean;
  seek(offset: number): void;
  expectMagicAndVersion(): void;
  readByte(): number;
  readU32(): number;
  readName(): string;
  readTableType(): WasmTableType;
  readLimits(): Pick<WasmTableType, "min" | "max">;
}

function createWasmParser(wasm: Buffer): WasmParser {
  let offset = 0;

  function done(): boolean {
    return offset >= wasm.length;
  }

  function seek(nextOffset: number): void {
    if (nextOffset < offset || nextOffset > wasm.length) {
      throw new Error(`Invalid wasm parser seek to ${nextOffset}`);
    }
    offset = nextOffset;
  }

  function expectMagicAndVersion(): void {
    const magic = [0x00, 0x61, 0x73, 0x6d];
    const version = [0x01, 0x00, 0x00, 0x00];
    for (const expected of [...magic, ...version]) {
      const actual = readByte();
      if (actual !== expected) {
        throw new Error("Invalid wasm binary header");
      }
    }
  }

  function readByte(): number {
    const byte = wasm[offset];
    if (byte === undefined) {
      throw new Error("Unexpected end of wasm binary");
    }
    offset += 1;
    return byte;
  }

  function readU32(): number {
    let result = 0;
    let shift = 0;

    for (;;) {
      const byte = readByte();
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) {
        return result >>> 0;
      }
      shift += 7;
      if (shift > 35) {
        throw new Error("Invalid wasm LEB128 integer");
      }
    }
  }

  function readName(): string {
    const length = readU32();
    const end = offset + length;
    if (end > wasm.length) {
      throw new Error("Unexpected end of wasm name");
    }
    const value = wasm.subarray(offset, end).toString("utf8");
    offset = end;
    return value;
  }

  function readTableType(): WasmTableType {
    const elementType = readReferenceType();
    const limits = readLimits();
    return { elementType, ...limits };
  }

  function readLimits(): Pick<WasmTableType, "min" | "max"> {
    const flags = readU32();
    const min = readU32();
    const max = (flags & 1) === 1 ? readU32() : null;
    return { min, max };
  }

  function readReferenceType(): string {
    const value = readByte();
    if (value === 0x6f) {
      return "externref";
    }
    if (value === 0x70) {
      return "funcref";
    }
    return `0x${value.toString(16)}`;
  }

  return {
    get offset() {
      return offset;
    },
    done,
    seek,
    expectMagicAndVersion,
    readByte,
    readU32,
    readName,
    readTableType,
    readLimits,
  };
}

function run(command: string, args: string[], cwd: string): void {
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    env: process.env,
  });
  if (result.error) {
    throw new Error(
      `failed to run ${command}: ${result.error.message}${installHint(command)}`,
      { cause: result.error },
    );
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with ${result.status}`);
  }
}

function installHint(command: string): string {
  if (command === "wasm-bindgen") {
    return "\nInstall wasm-bindgen-cli 0.2.106 and ensure wasm-bindgen is on PATH.";
  }
  if (command === "wasm-opt") {
    return "\nInstall Binaryen and ensure wasm-opt is on PATH.";
  }
  return "";
}
