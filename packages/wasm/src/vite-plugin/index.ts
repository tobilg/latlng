/**
 * Vite plugin for @latlng/wasm browser builds.
 *
 * The package worker resolves the wasm file at `../wasm/latlng_core_bg.wasm`
 * relative to the emitted worker asset. This plugin copies the packaged wasm
 * file into that location for Vite app builds.
 *
 * @packageDocumentation
 */

import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Plugin, ResolvedConfig } from "vite";

/** Options for `latlngWasmPlugin`. */
export interface LatLngWasmPluginOptions {
  /**
   * Source wasm file path.
   *
   * Defaults to the wasm file bundled with `@latlng/wasm`.
   */
  wasmPath?: string;
  /**
   * Directory inside Vite's output directory where the wasm file is copied.
   *
   * @default "wasm"
   */
  wasmDirName?: string;
  /**
   * Output wasm file name.
   *
   * @default "latlng_core_bg.wasm"
   */
  wasmFileName?: string;
}

/** Returns the absolute path to the wasm file bundled with `@latlng/wasm`. */
export function getLatLngWasmPath(): string {
  const currentDir = dirname(fileURLToPath(import.meta.url));
  return resolve(currentDir, "..", "wasm", "latlng_core_bg.wasm");
}

/** Copies `latlng_core_bg.wasm` into Vite's build output for worker runtime loading. */
export function latlngWasmPlugin(options: LatLngWasmPluginOptions = {}): Plugin {
  const {
    wasmPath = getLatLngWasmPath(),
    wasmDirName = "wasm",
    wasmFileName = "latlng_core_bg.wasm",
  } = options;
  let config: ResolvedConfig;

  return {
    name: "latlng-wasm-plugin",
    apply: "build",
    configResolved(resolvedConfig) {
      config = resolvedConfig;
    },
    writeBundle(outputOptions) {
      const outDir = outputOptions.dir ?? resolve(config.root, config.build.outDir);
      const targetDir = resolve(outDir, wasmDirName);
      const targetPath = resolve(targetDir, wasmFileName);

      if (!existsSync(wasmPath)) {
        this.error(`latlng wasm file not found: ${wasmPath}`);
      }

      mkdirSync(targetDir, { recursive: true });
      copyFileSync(wasmPath, targetPath);
    },
  };
}

export default latlngWasmPlugin;
