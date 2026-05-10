# latlng TypeScript SDK

TypeScript SDK for the `latlng` server.

## Scope

This package targets the current operational `latlng` HTTP and WebSocket surfaces.

Current scope:

- typed HTTP client for CRUD, search, admin/info, hooks, and channels
- typed WebSocket client for `auth`, `subscribe`, `psubscribe`, `ping`, and `quit`
- optional configuration-driven read routing across leader and follower URLs
- browser and Node support

Explicit non-goals:

- no Cap'n Proto client
- no server-side replication management
- no Worker-specific runtime bundle

## Install

```sh
npm install @latlng/sdk
```

## Quickstart

```ts
import { LatLngClient, point } from "@latlng/sdk";

const client = new LatLngClient({
  leaderUrl: "http://127.0.0.1:7421",
  token: "dev-token",
});

await client.setObject("fleet", "truck-1", point(52.52, 13.405));
const object = await client.get("fleet", "truck-1");
const nearby = await client.nearby("fleet", {
  lat: 52.52,
  lon: 13.405,
  meters: 500,
});
```

Collections can also be created explicitly and remain present even when empty until they are dropped:

```ts
await client.createCollection("fleet");
const collection = await client.getCollection("fleet");
```

## Client Configuration

```ts
const client = new LatLngClient({
  leaderUrl: "http://127.0.0.1:7421",
  token: "dev-token",
  timeoutMs: 5_000,
  headers: { "x-client": "worker-a" },
});
```

Supported options:

| Option | Purpose |
| --- | --- |
| `leaderUrl` | Base HTTP URL for the leader server. |
| `token` | Bearer token or JWT sent to HTTP and WebSocket requests. |
| `timeoutMs` | Per-request HTTP timeout. |
| `headers` | Extra headers merged into every HTTP request. |
| `fetch` | Custom fetch implementation for tests or custom runtimes. |
| `webSocketFactory` | Custom WebSocket factory for Node or tests. |
| `readReplicas` | Optional follower URLs used for eligible reads. |
| `readPreference` | One of `leader`, `leaderPreferred`, `followerPreferred`, or `roundRobinFollowers`. |
| `replicaStatusTtlMs` | Cache TTL for follower health/status probes. |

## API Surface

The SDK exposes the native HTTP API as typed methods on `LatLngClient`.

Connection and server status:

| Method | Description |
| --- | --- |
| `ping()` | Check that the leader is reachable and authenticated. |
| `healthz()` | Read the health endpoint. |
| `server()` | Read server status, replication role, sequence, and version metadata. |
| `info(section?)` | Read the broader server info response. |
| `metrics()` | Read Prometheus text exposition metrics. |

Collections:

| Method | Description |
| --- | --- |
| `collections(matchPattern?)` | List visible collections. |
| `createCollection(collection)` | Create an empty collection. |
| `getCollection(collection)` | Read collection metadata or `null`. |
| `rename(collection, newName, options?)` | Rename a collection, optionally with `nx`. |
| `dropCollection(collection)` | Drop a collection explicitly. |
| `bounds(collection)` | Read collection bounds. |
| `stats(collection)` | Read collection statistics. |

Objects, fields, TTLs, and JSON paths:

| Method | Description |
| --- | --- |
| `setObject(collection, id, object, options?)` | Store point, bounds, geohash, GeoJSON, or string objects. |
| `setPoint(collection, id, coordinates, options?)` | Convenience wrapper for point writes. |
| `get(collection, id, options?)` | Read an object or `null`; can include fields and alternate output formats. |
| `delete(collection, id)` | Delete one object. |
| `deleteMatching(collection, options?)` | Delete objects matching an ID pattern. |
| `setFields(collection, id, fields, options?)` | Update one or more fields. |
| `getField(collection, id, field)` | Read one field value. |
| `expire(collection, id, seconds)` | Set or replace an object TTL. |
| `persist(collection, id)` | Remove an object TTL. |
| `ttl(collection, id)` | Read remaining TTL seconds or `null`. |
| `setJson(collection, id, path, value, options?)` | Set a JSON path value. |
| `getJson(collection, id, path)` | Read a JSON path value. |
| `deleteJson(collection, id, path)` | Delete a JSON path value. |

Search:

| Method | Description |
| --- | --- |
| `nearby(collection, query)` | Search around a point/radius. |
| `within(collection, request)` | Search objects contained by an area. |
| `intersects(collection, request)` | Search objects intersecting an area. |
| `scan(collection, options?)` | Scan IDs/objects with match and field filters. |
| `search(collection, options?)` | Text/string search with match and field filters. |

Search options include `cursor`, `limit`, `nofields`, `matchPattern`, `sort`,
`whereFilters`, `whereInFilters`, `whereExprFilters`, `clip`, and output formats
such as `objects`, `points`, `bounds`, `ids`, `count`, or geohashes.

```ts
const trucks = await client.scan("fleet", {
  limit: 100,
  nofields: true,
  whereFilters: [
    { field: "speed", comparison: { type: "range", min: 10, max: 80 } },
  ],
  output: "ids",
});
```

Channels, hooks, and geofence events:

| Method | Description |
| --- | --- |
| `setChannel(request)` | Create or replace an in-server geofence channel. |
| `channels(matchPattern?)` | List channel names. |
| `getChannel(name)` | Read a full channel definition or `null`. |
| `deleteChannel(name)` | Delete a channel. |
| `setHook(request)` | Create or replace a webhook hook. |
| `hooks(matchPattern?)` | List hook summaries. |
| `getHook(name)` | Read a full hook definition or `null`. |
| `deleteHook(name)` | Delete a hook. |
| `webhookQueue()` | Read durable webhook queue stats. |
| `connectWebSocket(options?)` | Connect to the leader WebSocket endpoint for channel subscriptions. |

Admin and runtime operations:

| Method | Description |
| --- | --- |
| `configGet(name)` | Read a runtime config value. |
| `configSet(name, value)` | Update a runtime config value. |
| `configRewrite()` | Persist current runtime config to disk. |
| `readonly(enabled)` | Toggle server read-only mode. |
| `timeout(request)` | Configure per-command server timeouts. |
| `aofshrink()` | Compact AOF persistence and return before/after stats. |
| `gc()` | Trigger server maintenance. |
| `flushdb()` | Delete all data. |

## Value Helpers

The package exports constructors for common wire shapes:

```ts
import {
  bounds,
  fieldEntries,
  geojson,
  hash,
  jsonField,
  numberField,
  point,
  stringObject,
  textField,
} from "@latlng/sdk";

await client.setObject("fleet", "truck-1", point(52.52, 13.405), {
  fields: {
    speed: numberField(42),
    status: textField("moving"),
    payload: jsonField(JSON.stringify({ driver: "a" })),
  },
});

await client.setObject("zones", "berlin", bounds({
  min_lat: 52.3,
  min_lon: 13.0,
  max_lat: 52.7,
  max_lon: 13.8,
}));

const fields = fieldEntries({ status: textField("ready") });
await client.setFields("fleet", "truck-1", fields);

await client.setObject("messages", "msg-1", stringObject("dispatch ready"));
await client.setObject(
  "areas",
  "geojson-1",
  geojson({ type: "Point", coordinates: [13.405, 52.52] }),
);
await client.setObject("cells", "u33dc", hash("u33dc"));
```

## WebSocket Example

```ts
const ws = await client.connectWebSocket();
const subscription = await ws.psubscribe(["fleet*"]);

subscription.on("event", (event) => {
  console.log(event.detect, event.id);
});
```

Subscriptions are also async iterables:

```ts
for await (const event of subscription) {
  console.log(event.channel, event.detect, event.id);
}
```

`LatLngWebSocketClient` exposes `ping()`, `subscribe(channels)`,
`psubscribe(patterns)`, `quit()`, and `close()`. The helper
`parseGeofenceEvent()` is exported for callers that need to parse event payloads
outside the WebSocket client.

## Read Replica Routing

```ts
const client = new LatLngClient({
  leaderUrl: "http://leader:7421",
  readReplicas: [
    "http://follower-1:7421",
    "http://follower-2:7421",
  ],
  readPreference: "followerPreferred",
  token: "dev-token",
});
```

Followers are only used for reads when they report:

- `leader === false`
- `caught_up_once === true`

Writes and admin calls always target the configured leader URL.

Read routing only applies to normal read methods. Leader-only status/admin calls,
WebSocket connections, writes, config updates, compaction, and webhook queue
inspection target the leader.

## Authentication

The SDK transports bearer tokens. It does not mint, refresh, or introspect tokens.

In practice:

- local/dev commonly uses the static bearer token
- production deployments typically use JWTs issued by an external service or IdP
- claims-based authorization is enforced by the server, not by the SDK

The current auth/authz model, claim schema, and config reference are documented in [docs/auth.md](https://github.com/tobilg/latlng/blob/main/docs/auth.md).

## Errors

The SDK exports typed error classes:

| Error | Meaning |
| --- | --- |
| `LatLngError` | Base SDK error. |
| `TimeoutError` | Request exceeded `timeoutMs`. |
| `HttpError` | Non-success HTTP response with status and payload context. |
| `AuthError` | Authentication or authorization failure. |
| `ServerUnavailableError` | Server unavailable response. |

```ts
import { AuthError, LatLngClient } from "@latlng/sdk";

const client = new LatLngClient({
  leaderUrl: "http://127.0.0.1:7421",
  token: "dev-token",
});

try {
  await client.ping();
} catch (error) {
  if (error instanceof AuthError) {
    console.error("invalid token");
  }
}
```

## API Reference

Run `npm run docs:api` to generate TypeDoc reference pages for all exported
classes, helpers, request types, response types, search models, geofence event
models, and error classes.

## Development

```sh
npm ci
npm run build
npm run docs:api
npm run test:unit
npm run test:integration
```

The integration suite starts local `latlng-server` processes and exercises the SDK against the real HTTP, WebSocket, and leader/follower surfaces.

## Release Notes

Current package toolchain:

- `typescript@6.0.3`
- `vite@8.0.11`
- `vitest@4.1.5`

Subdirectory publish flow:

```sh
npm ci
npm run test
npm run docs:api
npm publish --access public
```
