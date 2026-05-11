# @latlng/wasm

Browser Web Worker and WebAssembly package for the `latlng` geospatial object engine.

This package is intentionally browser-only and worker-only. The wasm engine runs in a dedicated module worker; the main thread talks to it through async TypeScript methods and receives object mutation/geofence events as DOM `CustomEvent`s.

## Install

```sh
npm install @latlng/wasm
```

## Quickstart

```ts
import { createLatLng } from "@latlng/wasm";

const db = await createLatLng();

await db.createCollection("fleet");
await db.setPoint("fleet", "truck-1", { lat: 52.52, lon: 13.405 });

const nearby = await db.nearby("fleet", {
  lat: 52.52,
  lon: 13.405,
  meters: 1_000,
});

console.log(nearby.results);
db.close();
```

If your bundler or CDN serves the wasm file from a custom location, pass it explicitly:

```ts
const db = await createLatLng({
  wasmUrl: "/assets/latlng_core_bg.wasm",
});
```

## Events

Mutation events are emitted for collection, object, and hook changes.

```ts
const unsubscribe = db.on("mutation", (event) => {
  console.log(event.detail.type, event.detail.collection, event.detail.id);
});

await db.setPoint("fleet", "truck-1", { lat: 52.52, lon: 13.405 });
unsubscribe();
```

Browser hooks emit geofence events as JavaScript events. They do not send HTTP POST requests.

```ts
db.addEventListener("geofence:enter", (event) => {
  console.log("entered", event.detail.hook, event.detail.id);
});

await db.setHook("berlin", {
  collection: "fleet",
  query: {
    type: "within",
    area: {
      type: "circle",
      lat: 52.52,
      lon: 13.405,
      meters: 1_000,
    },
  },
  detect: ["enter", "exit"],
  commands: ["set", "del"],
});

await db.setPoint("fleet", "truck-1", { lat: 52.52, lon: 13.405 });
```

Supported geofence event names:

- `geofence`
- `geofence:inside`
- `geofence:outside`
- `geofence:enter`
- `geofence:exit`
- `geofence:cross`
- `geofence:roam`

## API

All methods are asynchronous because calls cross the Web Worker boundary.

### Lifecycle

```ts
const db = await createLatLng();
db.close();
```

`createLatLng()` starts the bundled worker and waits until the wasm engine is ready. `close()` terminates the worker and rejects pending requests.

`wasmUrl` accepts a `string` or `URL` and must point to `latlng_core_bg.wasm`. If omitted, the worker loads `../wasm/latlng_core_bg.wasm` relative to the emitted worker file.

`workerFactory` is available for tests and advanced bundler setups that need to
construct the worker themselves:

```ts
const db = await createLatLng({
  workerFactory: () => new Worker(new URL("./latlng.worker.js", import.meta.url), {
    type: "module",
  }),
});
```

`LatLngWasmClient` is exported for low-level integration code. Application code
should usually prefer `createLatLng()`, which constructs the worker and awaits
`ready()` for you.

Complete client method surface:

| Area | Methods |
| --- | --- |
| Lifecycle | `ready()`, `close()` |
| Events | `addEventListener()`, `removeEventListener()`, `on()` |
| Collections | `createCollection()`, `dropCollection()`, `collections()`, `collectionInfo()`, `bounds()`, `stats()` |
| Objects | `setObject()`, `setPoint()`, `getObject()`, `deleteObject()`, `expire()`, `persist()`, `ttl()` |
| Queries | `nearby()`, `within()`, `intersects()`, `scan()`, `search()` |
| Hooks | `setHook()`, `deleteHook()`, `hooks()`, `getHook()` |
| Metadata | `serverInfo()` |

### Collections

```ts
await db.createCollection("fleet");
await db.collections();
await db.collections("fl*");
await db.collectionInfo("fleet");
await db.bounds("fleet");
await db.stats(["fleet"]);
await db.dropCollection("fleet");
```

Collection creation is explicit, but object writes still implicitly create missing collections.

### Objects

```ts
await db.setPoint("fleet", "truck-1", { lat: 52.52, lon: 13.405 });

await db.setObject(
  "fleet",
  "truck-2",
  { type: "geojson", value: { type: "Point", coordinates: [13.4, 52.5] } },
  {
    fields: {
      status: { type: "text", value: "active" },
      speed: { type: "number", value: 42 },
    },
    expireSeconds: 300,
    condition: "always",
  },
);

const object = await db.getObject("fleet", "truck-1");
await db.expire("fleet", "truck-1", 60);
const ttl = await db.ttl("fleet", "truck-1");
await db.persist("fleet", "truck-1");
await db.deleteObject("fleet", "truck-1");
```

`condition` can be `always`, `nx`, or `xx`.
`getObject()` accepts `{ withFields?: boolean }` and includes fields by default.
Stored object shapes are `point`, `bounds`, `hash`, `geojson`, and `string`.
Field values can be `number`, `text`, or `json`.

### Queries

```ts
await db.nearby("fleet", {
  lat: 52.52,
  lon: 13.405,
  meters: 2_000,
  options: { limit: 20, output: "objects" },
});

await db.within("fleet", {
  area: { type: "circle", lat: 52.52, lon: 13.405, meters: 2_000 },
});

await db.intersects("fleet", {
  area: {
    type: "bounds",
    bounds: {
      min_lat: 52.4,
      min_lon: 13.2,
      max_lat: 52.6,
      max_lon: 13.6,
    },
  },
});

await db.scan("fleet", { limit: 100 });
await db.search("fleet", { matchPattern: "truck-*" });
```

Query options support:

- `cursor` and `limit`
- `nofields`
- `matchPattern`
- `sort`: `asc` or `desc`
- `whereFilters`, `whereInFilters`, and `whereExprFilters`
- `clip`
- `output`: `objects`, `points`, `bounds`, `ids`, `count`, or geohashes

Area queries support `circle`, `bounds`, `hash`, `geojson`, `tile`, `quadkey`,
`sector`, and `reference` areas.

### Hooks

```ts
await db.setHook("near-alex", {
  collection: "fleet",
  query: {
    type: "nearby",
    lat: 52.5219,
    lon: 13.4132,
    meters: 500,
  },
  detect: ["enter", "exit"],
  commands: ["set", "del"],
});

await db.hooks();
await db.getHook("near-alex");
await db.deleteHook("near-alex");
```

Hook queries support `nearby`, `within`, `intersects`, `scan`, `search`, and
`roam`. Hooks are persisted only inside the in-browser wasm engine lifetime.
They emit JS geofence events and do not use webhook delivery. The browser wasm
package does not expose the server channel API.

### Metadata

```ts
const info = await db.serverInfo();
console.log(info.version, info.num_collections, info.num_objects);
```

## Runtime Notes

- The package has no network, auth, replication, Durable Object, AOF, or webhook queue APIs.
- The wasm state is in-memory inside the worker and is lost when the worker is terminated or the page reloads.
- Serve `.wasm` files with `application/wasm` for fastest startup. The fallback still works but is slower.
- Bundlers must preserve the emitted worker asset and make `latlng_core_bg.wasm` available to the worker.
- Package builds run `wasm-bindgen` and attempt `wasm-opt -Oz` when Binaryen is
  available. The final `.wasm` is always checked to ensure
  `__wbindgen_externrefs` still points at an `externref` table; if optimization
  corrupts that export, the build keeps the validated wasm-bindgen output.

## Bundling

### Vite

For Vite apps, use the package plugin. It copies `latlng_core_bg.wasm` to `dist/wasm/latlng_core_bg.wasm`, matching the worker's default runtime URL.

```ts
import { defineConfig } from "vite";
import { latlngWasmPlugin } from "@latlng/wasm/vite-plugin";

export default defineConfig({
  plugins: [latlngWasmPlugin()],
});
```

Custom output names are supported:

```ts
latlngWasmPlugin({
  wasmPath: "/absolute/path/to/latlng_core_bg.wasm",
  wasmDirName: "assets",
  wasmFileName: "latlng.wasm",
});
```

When using a custom location, pass the matching URL at runtime:

```ts
const db = await createLatLng({
  wasmUrl: "/assets/latlng.wasm",
});
```

The Vite plugin entrypoint also exports `getLatLngWasmPath()` for build systems
that need the absolute path to the packaged wasm file.

### Rollup

Copy `node_modules/@latlng/wasm/dist/wasm/latlng_core_bg.wasm` into your public output and pass its URL:

```ts
const db = await createLatLng({
  wasmUrl: new URL("/wasm/latlng_core_bg.wasm", location.href),
});
```

If your Rollup setup emits workers under `assets/`, copying the wasm file to `dist/wasm/latlng_core_bg.wasm` lets the default URL work without `wasmUrl`.

### Webpack

Use `copy-webpack-plugin` or an equivalent asset-copy step to copy:

```text
node_modules/@latlng/wasm/dist/wasm/latlng_core_bg.wasm -> dist/wasm/latlng_core_bg.wasm
```

Then either rely on the default worker-relative URL or pass:

```ts
const db = await createLatLng({
  wasmUrl: "/wasm/latlng_core_bg.wasm",
});
```

### esbuild or tsup

esbuild and tsup do not automatically understand this package's worker/wasm asset relationship. Keep the worker bundle as an external emitted asset, copy the wasm file into your served output, and pass `wasmUrl` explicitly.

```ts
const db = await createLatLng({
  wasmUrl: "/static/latlng_core_bg.wasm",
});
```

### CDN or Static Hosting

Publish or copy `latlng_core_bg.wasm` to a stable URL and pass that URL to `createLatLng`.

```ts
const db = await createLatLng({
  wasmUrl: "https://cdn.example.com/latlng/latlng_core_bg.wasm",
});
```
