# @latlng/example-wasm

Static Vite 8 + TypeScript site that showcases the browser-only `@latlng/wasm`
package.

## Local Development

```sh
cd packages/example-wasm
npm install
npm run dev
```

The package depends on `@latlng/wasm` through `file:../wasm`. The production
build script rebuilds `packages/wasm` first so the Vite app can consume the
current local package output.

## Build

```sh
npm run typecheck
npm run build
```

The Vite plugin from `@latlng/wasm/vite-plugin` copies
`latlng_core_bg.wasm` into `dist/wasm/latlng_core_bg.wasm`.

## Cloudflare Pages

```sh
npm run build
npm run pages:dev
npm run deploy
```

`wrangler.toml` configures `latlng-wasm-example` with `dist` as the Pages build
output directory.
