import { resolve } from "node:path";
import { defineConfig } from "vite";
import { latlngWasmPlugin } from "@latlng/wasm/vite-plugin";

export default defineConfig({
  plugins: [latlngWasmPlugin()],
  server: {
    fs: {
      allow: [resolve(__dirname), resolve(__dirname, "..", "wasm")],
    },
  },
  build: {
    assetsInlineLimit: 0,
    target: "es2022",
    sourcemap: false,
  },
});
