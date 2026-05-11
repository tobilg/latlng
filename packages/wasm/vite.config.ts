import { resolve } from "node:path";
import { defineConfig } from "vite";
import { latlngWasmPlugin } from "./src/vite-plugin/index.js";

export default defineConfig({
  base: "./",
  plugins: [
    latlngWasmPlugin({
      wasmPath: resolve(__dirname, "pkg", "latlng_core_bg.wasm"),
    }),
  ],
  build: {
    lib: {
      entry: {
        index: resolve(__dirname, "src/index.ts"),
        "vite-plugin/index": resolve(__dirname, "src/vite-plugin/index.ts"),
      },
      formats: ["es"],
      fileName: (_format, entryName) => `${entryName}.js`,
    },
    outDir: "dist",
    assetsInlineLimit: 0,
    sourcemap: false,
    target: "es2022",
    rollupOptions: {
      external: ["node:fs", "node:path", "node:url"],
    },
  },
});
