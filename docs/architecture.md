# Architecture

This document describes the current `latlng` architecture as implemented in the repository today.

## Design Goals

- keep the command engine portable across native and `wasm32-unknown-unknown`
- isolate transport and runtime concerns from the core data model
- allow the same engine to run embedded, as a native server, and inside a browser Web Worker wasm runtime
- keep storage pluggable behind a small backend trait
- route geofence subscriptions and hooks through one shared event pipeline
- make outbound webhook delivery durable and replayable from the primary log
- support native-only single-leader follower replication from committed storage entries

## Runtime Shapes

`latlng` currently runs in three different forms.

Embedded library:

- application code constructs `LatLng<P, S>` directly
- storage backend is chosen by the embedding application
- no HTTP, WebSocket, or Cap'n Proto layer is required

Native server:

- `latlng-server` owns one `Arc<LatLngNative<S>>`
- request-style native HTTP, WebSocket, and Cap'n Proto handlers call the synchronous core through a dedicated bounded native executor
- `latlng-core` owns a portable global control gate plus a catalog of per-collection cells
- read-only operations use shared global control and then read the specific collection cells they need
- steady-state collection-local mutations use shared global control and a per-collection write lock
- namespace/admin operations and conservative fallback cases still use exclusive global control
- long-lived subscription bridges and background outbox maintenance stay outside the request executor so they cannot starve request throughput
- a replication manager can switch the node into follower mode, maintain a Cap'n Proto replication stream, and gate read/write availability until catch-up
- `latlng-config` owns the persisted runtime config model for the native process
- `latlng-auth` is reused by HTTP, WebSocket, and Cap'n Proto for bearer/JWT validation
- `latlng-http`, `latlng-ws`, and `latlng-capnp` all adapt requests into shared core types
- `latlng-http` also owns the canonical generated OpenAPI v3 document exposed at `GET /api-docs`
- a durable webhook outbox tails the primary log, materializes jobs into SQLite, and POSTs due events

Browser wasm package:

- portable crates compile to `wasm32-unknown-unknown`
- `latlng-core` exposes browser-safe wasm bindings
- `packages/wasm` wraps those bindings behind an asynchronous browser Web Worker API
- state is intentionally in-memory and local to the browser worker
- native transports, admin operations, durable storage, replication, auth, and outbound webhook delivery are not part of the browser package

TypeScript SDK package:

- `packages/sdk` is a transport companion, not a fourth runtime shape
- it targets the public HTTP and WebSocket server surfaces
- it stays configuration-driven for leader plus optional follower reads and does not depend on Cap'n Proto

## Component Graph

```text
packages/sdk
    |
    v
HTTP/JSON        WebSocket        Cap'n Proto RPC         Embedded Rust       Browser Worker API
   |                 |                  |                      |                   |
   v                 v                  v                      v                   v
latlng-http      latlng-ws        latlng-capnp           latlng-core API      packages/wasm
   \                 |                  /                      |                   |
    \                |                 /                       |                   |
     +---------- latlng-auth + latlng-config -----------------+                   |
                                      |                                           |
                                      v                                           v
                              Arc<LatLngNative<S>>                           latlng-core
                                      |                                           |
                                      v                                           |
        native executor + replication manager + global control + catalog          |
                                      |                                           |
                                      v                                           |
                                 latlng-core <-----------------------------------+
                          /             |             \
                         v              v              v
                  latlng-index     latlng-geofence   latlng-storage
                         |                              /   |    \
                         v                             v    v     v
                    latlng-geo                     memory AOF  SQLite
```

## Request Flow

All native transports follow the same high-level pattern.

1. Accept a request or subscription from the transport layer.
2. Validate auth for that transport.
3. Decode the transport payload into shared core request types.
4. On native, hand request-style synchronous core work to the dedicated bounded executor.
5. Let `latlng-core` resolve the operation through its global control gate and collection catalog:
   - read-only operations take shared global access, then read the required collection cells
   - steady-state collection-local writes take shared global access, then a per-collection write lock
   - admin/namespace operations and conservative fallback cases take exclusive global access
6. Serialize the core result back into the transport-specific response shape.

Follower-specific flow:

1. A node enters follower mode through startup config or `FOLLOW host port`.
2. `latlng-server` starts a native replication loop that connects to the leader over Cap'n Proto.
3. The follower authenticates with a dedicated replication credential, validates the remote server identity, and rejects self-follow or following another follower.
4. The follower either resumes from its current last committed sequence after checksum verification or resets local state and full-resyncs from sequence `0`.
5. The follower opens a long-lived Cap'n Proto replication stream from its local last sequence.
6. The leader returns available committed `StorageEntry` values immediately. If no entries are available, the stream waits for the next durable mutation and is woken without polling.
7. `replication_batch_size` limits the number of committed storage entries returned per stream response; it is not a sync interval.
8. `replication_reconnect_backoff_ms` applies after stream/connect failures, not between healthy replicated writes.
9. Replicated `StorageEntry` values are appended to local durable storage and applied to live state through the same persisted-entry machinery used for startup replay.
10. While following, writes are rejected and normal reads are gated until the follower has caught up once.

Examples:

- `latlng-http` maps JSON bodies and route params into `SetRequest`, `NearbyQuery`, `GeofenceDef`, and other core types.
- `latlng-ws` supports `auth`, `subscribe`, `psubscribe`, `ping`, and `quit` commands on one connection and streams events as JSON frames once subscribed.
- `latlng-capnp` uses generated bindings from `latlng-schema`, implements the generated `lat_lng::Server` trait, and returns a `GeofenceStream` capability for event subscriptions.

## Core Engine

`latlng-core` is the center of the system.

Responsibilities:

- collection lifecycle and object storage
- geometry-aware commands such as `SET`, `GET`, `DEL`, `NEARBY`, `WITHIN`, `INTERSECTS`, `SCAN`, and `SEARCH`
- metadata field operations such as `FSET` and `FGET`
- JSON subdocument operations such as `JSET`, `JGET`, and `JDEL`
- server config and status such as `readonly`, per-command `timeout`, `flushdb`, `gc`, and `aofshrink`
- geofence registration and event emission
- persistence through the storage backend trait

Current concurrency model:

- the catalog maps collection names to versioned collection cells
- collection-local reads and many steady-state writes avoid whole-engine exclusivity
- writes that would change namespace membership, or that need conservative cross-collection geofence planning, still fall back to the exclusive global path
- when compiled with the `parallel` feature, large native `NEARBY`, `WITHIN`, `INTERSECTS`, `SCAN`, and `SEARCH` queries snapshot only their prefiltered candidate set, release the collection read lock, and then use rayon-backed candidate evaluation in `latlng-index`
- final ordering, cursor behavior, clipping, and output shaping stay deterministic and unchanged across serial and parallel query paths
- wasm uses the same logic through `latlng-platform`, but its lock types collapse to single-threaded `RefCell`/`Rc`

Query-shaping note:

- `latlng-index` only applies `CLIP` for `INTERSECTS` when the caller requests `OBJECTS` output. Other output modes ignore `clip`, and clipped geometries are returned as GeoJSON when they no longer fit the original shape representation.

Important internal collaborations:

- `latlng-index` handles candidate generation, filtering, sorting, cursors, and output shaping
- `latlng-geo` owns geometry, bounding boxes, geohashes, GeoJSON conversion, and JSON path utilities
- `latlng-geofence` evaluates geofence state transitions and constructs `GeofenceEvent` values
- `latlng-storage` is used to persist commands and compact snapshots

## Event Pipeline

Geofence events are generated inside the core engine and then fanned out through one registry.

```text
mutation in latlng-core
        |
        v
primary log + latlng-geofence::GeofenceRegistry
   /              |                    \
  v               v                     v
channels      subscribers        durable webhook outbox
                |                  (SQLite materialized queue)
                v                           |
         WS / Cap'n Proto                   v
                                          HTTP POST
```

Details:

- `setchan()` and `sethook()` register geofence definitions in the same registry
- hook and channel definitions are also persisted in the primary log and replayed on startup
- `subscribe()` and `psubscribe()` create bounded mailbox receivers through `latlng-platform`
- the mailbox capacity is configurable from server/core config and defaults to `4096` events per subscriber
- on native, mailbox senders explicitly wake waiting receivers so `latlng-ws` and `latlng-capnp` consume those receivers without fixed-interval polling
- mutating commands persist a `Command(...)` record plus any `WebhookEnqueue(...)` records in one atomic storage batch before they become visible
- `latlng-server` rebuilds `latlng-webhook-queue` from the primary log on startup
- the outbox appends durable `WebhookAck`, `WebhookRetryScheduled`, and `WebhookDeadLetter` records back into the primary log
- the dispatcher reads `webhook_timeout_ms`, `webhook_retry_*`, `webhook_lease_ms`, and `webhook_concurrency_limit` from runtime config
- the native outbox loop is wake-driven: new work, queue rebuilds, and due retry deadlines wake the dispatcher instead of relying on a steady polling interval
- webhook delivery is `at-least-once`; stable event/job IDs are included for consumer-side deduplication
- roaming geofences, `ROAM`, and `NODWELL` are implemented in the portable geofence layer, not in the server transports
- `FLUSHDB` clears collections, geofence definitions, geofence state, and the materialized durable webhook queue in one coordinated reset
- live subscribers are preserved across `FLUSHDB`, but the geofence registry bumps an internal generation so buffered pre-flush events are dropped

## Storage Architecture

`latlng-storage` defines the backend contract:

- append one entry
- append a batch
- replay from a sequence
- snapshot
- compact
- return last sequence
- checksum ranges
- close

Current backends:

- `latlng-storage-memory`: in-process ephemeral backend
- `latlng-storage-aof`: append-only file backend used by the runnable server
- `latlng-storage-sqlite`: SQLite backend for embedded/native use

Persistence behavior:

- mutating core commands are persisted before they become visible in memory
- hook/channel definitions are persisted through the same primary log
- webhook enqueue intents and delivery state changes are also stored in the primary log
- durable backends treat `append_batch()` as atomic, so a persisted mutation and its webhook enqueue intents replay together or not at all
- startup replay rebuilds in-memory collections incrementally as storage yields entries; the core no longer buffers the full primary log into memory before applying it
- the AOF backend runs a dedicated writer thread and uses durable group commit, so concurrent append requests can share one flush/sync cycle without changing success semantics
- replay also rebuilds the durable webhook queue materialization used by the native server
- replication streams committed logical `StorageEntry` values and resumes by sequence/checksum rather than by raw AOF file offsets
- AOF compaction is exposed through `aofshrink`
- the AOF loader ignores a truncated final frame but rejects complete corrupt payloads
- logical compaction preserves current objects, active hooks/channels, and unresolved webhook jobs
- the server coordinates `FLUSHDB` with the outbox worker so queue replay, leasing, and finalization cannot race the reset
- runtime config rewrite is separate from object persistence and is handled by `latlng-config`

## Transport Architecture

### HTTP

Implemented in `latlng-http`.

Characteristics:

- built on `axum`
- request/response JSON
- request metrics collected in `RequestMetrics`
- request IDs are attached through `x-request-id` and emitted in structured access logs
- CORS is configuration-driven and disabled by default
- auth via shared bearer/JWT validation from `latlng-auth`
- runtime config rewrite through `/admin/config/rewrite`
- exposes admin endpoints such as `/config/{name}`, `/admin/readonly`, `/admin/timeout`, `/admin/flushdb`, `/admin/gc`, `/admin/aofshrink`, and `/admin/webhook-queue`; `/metrics` is Prometheus text exposition protected by `metrics:read`

### WebSocket

Implemented in `latlng-ws`.

Characteristics:

- built on `axum` websocket support
- accepts header-based auth or an in-band `auth` command
- logs connection lifecycle, auth attempts, and subscription changes with a connection ID
- supports `subscribe`, `psubscribe`, `ping`, and `quit` commands
- streams geofence events from mailbox receivers once a subscription is active
- on native, a blocking receiver bridge wakes on mailbox activity and forwards events into the async socket task

### TypeScript SDK

Implemented in `packages/sdk`.

Characteristics:

- HTTP + WebSocket oriented, not Cap'n Proto derived
- built with TypeScript `5.9.3`, Vite `8`, and Vitest
- exports one ESM-first package with declaration files
- supports browser and Node runtimes
- exposes optional configuration-driven follower read routing through `leaderUrl`, `readReplicas`, and `readPreference`
- uses the existing `/server` status surface to decide whether a replica is eligible for reads

### Cap'n Proto

Implemented in `latlng-capnp`.

Characteristics:

- schema lives in `crates/latlng-schema/schema/latlng.capnp`
- generated bindings are exported by `latlng-schema`
- transport uses real async `capnp-rpc`
- each accepted connection creates a `RpcSystem`
- logs accepted connections and disconnect/failure outcomes with a connection ID
- `GeofenceStream.next()` is async and uses the same wake-driven native receiver bridge as WebSocket delivery
- the server runs on a Tokio `LocalSet` because `capnp-rpc::RpcSystem` is `!Send`
- auth is session-based through the `auth(token)` RPC and validates against the same bearer/JWT config used by the server
- `timeout`, `readonly`, and `configRewrite` are wired into the shared runtime config and core admin surface
- native replication also uses Cap'n Proto: leaders expose replication info/checksum/stream capabilities, and followers consume those streams with a dedicated replication credential

## Wasm Boundary

The portability boundary is intentional.

Portable crates:

- `latlng-platform`
- `latlng-geo`
- `latlng-index`
- `latlng-storage`
- `latlng-core`
- `latlng-geofence`
- `latlng-storage-memory`

Native-only crates:

- `latlng-storage-aof`
- `latlng-storage-sqlite`
- `latlng-webhook-queue`
- `latlng-schema`
- `latlng-capnp`
- `latlng-http`
- `latlng-ws`
- `latlng-endpoints`
- `latlng-replication`
- `latlng-server`

Browser wasm behavior:

- the browser package uses the wasm bindings exposed by `latlng-core`
- it exposes an asynchronous TypeScript API over a browser Web Worker
- it does not ship native HTTP, WebSocket, Cap'n Proto, auth, durable storage, replication, or outbound webhook delivery into wasm

## Auth Model

The auth model is transport-specific but shares one server configuration model and one claims-based authorization model.

HTTP:

- static bearer token, HMAC JWT, PEM-configured asymmetric JWT, or JWKS-configured asymmetric JWT
- collection-scoped action enforcement for object CRUD, queries, hooks, channels, metrics, and admin routes
- filtered list semantics for collections, hooks, and channels

WebSocket:

- bearer/JWT validation during the HTTP upgrade path or a later `auth` command
- `subscriptions:read` is enforced separately from `queries:read`

Cap'n Proto:

- same configured bearer/JWT verification modes
- validated through `auth(token)` on the RPC connection
- once authenticated, per-RPC action checks mirror the native HTTP authz model

Replication:

- uses a dedicated native-only replication credential, separate from normal bearer/JWT API auth
- followers present that credential to the leader's internal Cap'n Proto replication RPCs
- leaders reject self-follow and reject attaching to a node that is already following another leader
- followers pin the leader's server ID across reconnects

Detailed claim/config documentation lives in [auth.md](auth.md).

Native config:

- `latlng-config` can load and rewrite JSON or TOML files
- env vars and CLI flags override file values at process startup

## Crate Responsibilities

| Crate | Responsibility | Portability |
|---|---|---|
| `latlng-auth` | shared bearer/JWT validation | native |
| `latlng-config` | runtime config model and file persistence | native |
| `latlng-platform` | portable locks, shared ownership, mailbox channels | portable |
| `latlng-geo` | geometry model, geohash helpers, GeoJSON conversion, JSON paths | portable |
| `latlng-index` | spatial index, filters, sort order, output shaping | portable |
| `latlng-storage` | backend trait and persistence contracts | portable |
| `latlng-core` | command engine, collections, config, geofence registration | portable |
| `latlng-geofence` | geofence matching, roaming state, event generation | portable |
| `latlng-storage-memory` | in-memory backend | portable |
| `latlng-storage-aof` | append-only file backend | native |
| `latlng-storage-sqlite` | SQLite backend | native |
| `latlng-webhook-queue` | durable webhook queue materialized from the primary log | native |
| `latlng-schema` | Cap'n Proto schema and generated bindings | native build tooling |
| `latlng-capnp` | async Cap'n Proto transport | native |
| `latlng-http` | HTTP/JSON transport | native |
| `latlng-ws` | WebSocket event transport | native |
| `latlng-endpoints` | outbound webhook delivery | native |
| `latlng-replication` | replication role/state types plus follower coordination helpers | native |
| `latlng-server` | runnable server binary | native |
| `latlng-cli` | operational CLI | native |
| `latlng-benchmark` | benchmark harness | native |
| `latlng-server-benchmark` | black-box benchmark harness for the real native server process | native |

## Current Gaps

The current architecture is coherent and working, but not every planned subsystem is complete.

- replication is intentionally scoped to single-leader follower mode; broader clustering/consensus is still out of scope
- the CLI intentionally focuses on common operational flows rather than every protocol command
