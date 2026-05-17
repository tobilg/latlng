# latlng

`latlng` is an open source geospatial object engine written in Rust. The repository contains a portable core, native server transports, pluggable storage backends, geofence and webhook eventing, a TypeScript SDK package, and a public browser wasm package built on the same engine.

## Features

Implemented:

- portable Rust core for object storage, spatial indexing, search, metadata fields, and JSON subdocument updates
- `INTERSECTS` supports real geometry clipping for `OBJECTS` output; non-object outputs ignore `clip`, and clipped results may normalize to GeoJSON
- native HTTP/JSON server with shared auth, runtime config rewrite, Prometheus metrics, generated OpenAPI v3 docs, and admin endpoints
- native WebSocket command/event transport with header auth, in-band `auth`, and async subscription streaming
- native async Cap'n Proto RPC using generated schema bindings and `capnp-rpc`
- native single-leader follower replication over Cap'n Proto streaming with checksum-based resume/resync
- in-memory, append-only-file, and SQLite storage backends
- geofencing with static and roaming geofences, `NODWELL`, channel subscriptions, and durable HTTP POST webhook delivery
- wasm bindings for `latlng-core` and a browser Web Worker package for in-browser demos and local geospatial workloads

Intentional scope boundaries:

- the CLI covers common query and admin flows, but it is still not a full command-for-command shell
- replication is native-only and intentionally scoped to single-leader follower mode rather than broader clustering/consensus

## High-Level Architecture

The same engine is used in three shapes:

1. Embedded Rust library: call `latlng-core` directly with any storage backend.
2. Native server: expose the engine through HTTP, WebSocket, and Cap'n Proto.
3. Browser wasm package: compile the portable crates to `wasm32-unknown-unknown` and run the in-memory engine behind a browser Web Worker API.

The native server keeps `Arc<LatLngNative<S>>` directly, routes request-style synchronous core work through a dedicated bounded native executor, and relies on a portable global control gate plus per-collection cells inside `latlng-core`. In practice that gives native parallel reads, steady-state collection-local concurrency, and explicit backpressure for core request execution without changing the single-threaded wasm behavior model.

At a high level, requests flow like this:

```text
HTTP / WebSocket / Cap'n Proto / browser wasm API
                |
                v
         transport adapter
                |
                v
            latlng-core
         /       |       \
        v        v        v
  latlng-index latlng-geofence latlng-storage
        |                    /    |     \
        v                   v     v      v
    latlng-geo          memory   AOF   SQLite
```

Mutation events flow separately:

```text
latlng-core mutation
        |
        v
  primary log + geofence registry
      /         |              \
     v          v               v
channel subs  WS/Capnp     durable webhook outbox
  live only    streams      -> SQLite queue -> HTTP POST
```

The detailed architecture is documented in [docs/architecture.md](docs/architecture.md).
Configuration, persistence, and release notes live in [docs/config.md](docs/config.md), [docs/persistence.md](docs/persistence.md), and [docs/release-checklist.md](docs/release-checklist.md).

## Workspace Overview

- `crates/latlng-auth`: shared bearer/JWT validation used by HTTP, WebSocket, and Cap'n Proto
- `crates/latlng-config`: runtime config model plus JSON/TOML load/save helpers
- `crates/latlng-platform`: portable lock and mailbox abstractions for native and wasm builds
- `crates/latlng-geo`: geometry types, bounding boxes, geohash helpers, and JSON path utilities
- `crates/latlng-index`: spatial index plus filtering, sorting, and output shaping
- `crates/latlng-storage`: backend trait plus shared persistence contracts
- `crates/latlng-core`: command engine, collection lifecycle, geofence registration, and server info/config
- `crates/latlng-geofence`: geofence matching, roaming state, subscriptions, and event generation
- `crates/latlng-storage-memory`: in-memory backend
- `crates/latlng-storage-aof`: append-only file backend with compaction, integrity, backup, and restore support
- `crates/latlng-storage-sqlite`: SQLite backend for embedded/native use
- `crates/latlng-webhook-queue`: SQLite-backed durable webhook queue materialized from the primary log
- `crates/latlng-schema`: Cap'n Proto schema plus generated Rust bindings
- `crates/latlng-capnp`: async Cap'n Proto RPC transport
- `crates/latlng-http`: HTTP/JSON transport built on `axum`
- `crates/latlng-ws`: WebSocket event transport
- `crates/latlng-endpoints`: webhook delivery helpers
- `crates/latlng-replication`: follower state, replication client/coordinator, and checksum/chunk helpers
- `crates/latlng-server`: runnable native server binary
- `tools/latlng-cli`: operational CLI for common query and admin flows
- `tools/latlng-benchmark`: benchmark harness for writes, queries, geofences, and webhook delivery
- `tools/latlng-server-benchmark`: black-box localhost benchmark harness for the real `latlng-server` process
- `packages/sdk`: TypeScript SDK for the HTTP and WebSocket server surfaces
- `packages/wasm`: public browser-only Web Worker package around the wasm core for demos and local in-browser geospatial workloads
- `packages/example-wasm`: static Vite site showcasing `@latlng/wasm` for Cloudflare Pages

## Quickstart

The commands below assume the release binaries `latlng-server` and `latlng-cli`
are installed and available on `PATH`. Install them with Homebrew, Docker, or a
GitHub release archive as described below.

Start the native server:

```sh
latlng-server
```

By default it listens on:

- HTTP: `127.0.0.1:7421`

Cap'n Proto is disabled by default. Enable it when native Cap'n Proto clients or
leader/follower replication are needed:

```sh
latlng-server --capnp-enabled=true
```

The native server also supports JSON or TOML config files:

```sh
latlng-server --config ./latlng.json
```

The canonical generated OpenAPI v3 document for the native HTTP server is available at:

- `GET /api-docs`

It describes the stable native HTTP surface with typed request and response schemas. Diagnostic and replication-management routes are intentionally not part of the stable public API document. Release builds also attach the same generated document as `openapi.json`; locally it can be generated with `latlng-server --print-openapi` or `make openapi`.

The subscriber mailbox used for channel, WebSocket, Cap'n Proto, and webhook event delivery defaults to `4096` events per subscriber. You can override it with:

```sh
LATLNG_SUBSCRIBER_QUEUE_CAPACITY=8192 latlng-server
latlng-server --subscriber-queue-capacity 8192
```

The dedicated native core executor defaults to one worker per available CPU and a bounded queue sized at `threads * 64`. You can override it with:

```sh
LATLNG_NATIVE_EXECUTOR_THREADS=8 LATLNG_NATIVE_EXECUTOR_QUEUE_LIMIT=512 latlng-server
latlng-server --native-executor-threads 8 --native-executor-queue-limit 512
```

Webhook HTTP delivery uses a per-request timeout that defaults to `5000ms`. You can override it with:

```sh
LATLNG_WEBHOOK_TIMEOUT_MS=10000 latlng-server
latlng-server --webhook-timeout-ms 10000
```

Webhook delivery concurrency is bounded and defaults to `128` in-flight HTTP deliveries. You can override it with:

```sh
LATLNG_WEBHOOK_CONCURRENCY_LIMIT=256 latlng-server
latlng-server --webhook-concurrency-limit 256
```

Durable webhook delivery also has queue and retry settings:

```sh
LATLNG_WEBHOOK_QUEUE_PATH=./data/webhooks.sqlite latlng-server
LATLNG_WEBHOOK_RETRY_COUNT=8 latlng-server
LATLNG_WEBHOOK_RETRY_INITIAL_BACKOFF_MS=200 latlng-server
LATLNG_WEBHOOK_RETRY_MAX_BACKOFF_MS=30000 latlng-server
LATLNG_WEBHOOK_LEASE_MS=30000 latlng-server
```

Store and query a point through the CLI:

```sh
latlng-cli --base-url http://127.0.0.1:7421 set-point fleet truck-1 52.52 13.405
latlng-cli --base-url http://127.0.0.1:7421 get fleet truck-1
latlng-cli --base-url http://127.0.0.1:7421 nearby fleet 52.52 13.405 500
latlng-cli collection-create fleet
latlng-cli fset fleet truck-1 speed 42
latlng-cli fget fleet truck-1 speed
latlng-cli expire fleet truck-1 300
latlng-cli ttl fleet truck-1
latlng-cli jset fleet truck-1 properties.status active
latlng-cli jget fleet truck-1 properties.status
latlng-cli del fleet truck-1
latlng-cli timeout set 1.5
latlng-cli readonly yes
latlng-cli config-rewrite
```

Hook and channel geofences can be created from GeoJSON files. The file may include `properties.collection`, `properties.detect`, `properties.commands`, and `properties.mode`; otherwise pass `--collection`, `--detect`, `--commands`, or `--mode` on the CLI.

```sh
latlng-cli hook-set fleet-hook https://example.com/hook --geojson ./geofence.geojson --collection fleet
latlng-cli hooks
latlng-cli hook-get fleet-hook
latlng-cli channel-set fleet-channel --geojson ./geofence.geojson --collection fleet
latlng-cli channels
latlng-cli channel-del fleet-channel
```

Inspect and maintain an offline AOF file:

```sh
latlng-cli aof-verify ./data/appendonly.aof
latlng-cli aof-backup ./data/appendonly.aof ./backup/appendonly.backup.json
latlng-cli aof-restore ./backup/appendonly.backup.json ./restore/appendonly.aof
```

Or use plain HTTP:

```sh
curl -sS -X POST http://127.0.0.1:7421/collections/fleet/objects/truck-1 \
  -H 'content-type: application/json' \
  -d '{"object":{"Point":{"lat":52.52,"lon":13.405,"z":null}}}'

curl -sS -X POST http://127.0.0.1:7421/collections/fleet/search/nearby \
  -H 'content-type: application/json' \
  -d '{"lat":52.52,"lon":13.405,"meters":500,"options":{}}'
```

Or use the TypeScript SDK:

```sh
cd packages/sdk
npm install
npm run build
```

```ts
import { LatLngClient, point } from "@latlng/sdk";

const client = new LatLngClient({
  leaderUrl: "http://127.0.0.1:7421",
  token: "dev-token",
});

await client.setPoint("fleet", "truck-1", { lat: 52.52, lon: 13.405 });
const object = await client.get("fleet", "truck-1");
const nearby = await client.nearby("fleet", {
  lat: 52.52,
  lon: 13.405,
  meters: 500,
});
```

## Docker

The repository includes:

- a multi-stage production `Dockerfile`
- a single-node [docker-compose.yml](docker-compose.yml)
- a leader/follower [docker-compose.replication.yml](docker-compose.replication.yml)
- sample mounted configs under [examples/docker](examples/docker)

The published image is config-file driven and starts with:
`latlng-server --config /etc/latlng/latlng.toml`.

Container contract:

| Purpose | Container value | Notes |
| --- | --- | --- |
| HTTP, WebSocket, metrics, and API traffic | port `7421` | publish with `-p 7421:7421` |
| Cap'n Proto RPC and replication traffic | port `7422` | publish only when `capnp_enabled = true` or replication clients need host access |
| Default config path | `/etc/latlng/latlng.toml` | mount TOML or JSON config here, or override the command / `LATLNG_CONFIG` |
| Persistent data path | `/var/lib/latlng` | mount this when using AOF persistence or the durable webhook queue |
| Runtime user | `latlng` | the image runs as a non-root user |

Container configs should bind to `0.0.0.0`, not `127.0.0.1`, when the port must be
reachable through Docker port publishing. The sample configs already do this.

Build the image:

```sh
docker build -t latlng-server .
```

Published release images are available from Docker Hub as `tobilg/latlng:latest`
and versioned tags such as `tobilg/latlng:v0.1.3`.

Run a single node from the published image with a mounted config file and persistent
data volume:

```sh
docker run --rm \
  --name latlng \
  -p 7421:7421 \
  -p 7422:7422 \
  -v "$(pwd)/examples/docker/single-node.toml:/etc/latlng/latlng.toml:ro" \
  -v latlng-data:/var/lib/latlng \
  tobilg/latlng:latest
```

The bundled single-node example config uses:

- AOF: `/var/lib/latlng/appendonly.aof`
- webhook queue: `/var/lib/latlng/webhook-queue.sqlite`
- bearer token: `dev-token`

Check the HTTP endpoint with the sample bearer token:

```sh
curl -sS -H "Authorization: Bearer dev-token" http://127.0.0.1:7421/ping
```

For an HTTP-only container, omit `-p 7422:7422` and set `capnp_enabled = false`
in the mounted config.

Single-node compose:

```sh
docker compose up --build
```

Leader/follower compose:

```sh
docker compose -f docker-compose.replication.yml up --build
```

That brings up:

- leader HTTP on `127.0.0.1:7421`
- leader Cap'n Proto on `127.0.0.1:7422`
- follower HTTP on `127.0.0.1:17421`
- follower Cap'n Proto on `127.0.0.1:17422`

The follower example config follows the leader through Docker DNS using:

- `follow_host = "latlng-leader"`
- `follow_port = 7422`
- `replication_credential = "replication-secret"`

If you want to mount a different config path, either:

- override the command:
  `docker run ... latlng-server --config /some/other/path.toml`
- or set `LATLNG_CONFIG=/some/other/path.toml`

Config precedence is unchanged in containers:

- defaults
- config file
- environment variables
- CLI flags

## Homebrew

macOS arm64 release binaries are published to the `tobilg/latlng` Homebrew tap:

```sh
brew tap tobilg/latlng
brew install latlng
```

The formula installs `latlng-server` and `latlng-cli`. Its service uses AOF
persistence by default and listens on `127.0.0.1:7421`.

```sh
brew services start latlng
brew services stop latlng
```

Default Homebrew paths:

| Purpose | Path |
| --- | --- |
| Config file | `$(brew --prefix)/etc/latlng/latlng.toml` |
| AOF and webhook queue data | `$(brew --prefix)/var/latlng` |
| Service log | `$(brew --prefix)/var/log/latlng/latlng-server.log` |

Run the server manually with the same defaults:

```sh
latlng-server --config "$(brew --prefix)/etc/latlng/latlng.toml"
```

## Server Configuration

`latlng-server` reads JSON or TOML config files via `--config` or `LATLNG_CONFIG`.
The complete server config option set is:

| Name | Default | Description |
| --- | --- | --- |
| `production_mode` | `false` | Enables strict production startup guardrails. |
| `listen_addr` | `"127.0.0.1:7421"` | HTTP listen address. |
| `capnp_enabled` | `false` | Enables the Cap'n Proto RPC and replication listener. |
| `capnp_listen_addr` | `"127.0.0.1:7422"` | Cap'n Proto listen address. |
| `server_id` | `"<generated uuid>"` | Stable server identity used in replication status. |
| `storage` | `"memory"` | Storage backend. Use memory or aof with a path. |
| `read_only` | `false` | Rejects mutating commands when true. |
| `command_timeouts` | `{}` | Per-command timeout overrides in seconds. |
| `subscriber_queue_capacity` | `4096` | Per-subscriber event queue capacity. |
| `webhook_queue_path` | `null` | SQLite webhook queue path. Defaults near the AOF or current directory. |
| `webhook_timeout_ms` | `5000` | HTTP timeout for webhook deliveries. |
| `webhook_concurrency_limit` | `128` | Maximum concurrent webhook delivery attempts. |
| `webhook_retry_count` | `8` | Maximum webhook retry attempts before dead-lettering. |
| `webhook_retry_initial_backoff_ms` | `200` | Initial webhook retry backoff. |
| `webhook_retry_max_backoff_ms` | `30000` | Maximum webhook retry backoff. |
| `webhook_lease_ms` | `30000` | Webhook job lease duration. |
| `native_executor_threads` | `<available CPU parallelism>` | Native worker thread count for core operations. |
| `native_executor_queue_limit` | `<native_executor_threads * 64>` | Native executor queue limit. |
| `aof_writer_queue_limit` | `4096` | AOF writer queue limit. |
| `aof_group_commit_delay_ms` | `1` | Maximum AOF group commit delay. |
| `aof_group_commit_max_requests` | `128` | Maximum requests per AOF commit cycle. |
| `follow_host` | `null` | Leader host for follower replication. |
| `follow_port` | `null` | Leader Cap'n Proto port for follower replication. |
| `replication_credential` | `null` | Dedicated credential for replication streams. |
| `replication_batch_size` | `512` | Maximum entries per replication stream response. |
| `replication_reconnect_backoff_ms` | `1000` | Follower reconnect backoff after failures. |
| `http_cors_enabled` | `false` | Enables HTTP CORS middleware. |
| `http_cors_allowed_origins` | `[]` | Allowed CORS origins. Avoid `*` with auth. |
| `http_cors_allowed_methods` | `["GET","POST","PUT","DELETE","OPTIONS"]` | Allowed CORS methods. |
| `http_cors_allowed_headers` | `["authorization","content-type","x-request-id"]` | Allowed CORS headers. |
| `http_cors_max_age_seconds` | `null` | Optional CORS preflight cache max-age. |
| `http_max_body_bytes` | `10485760` | Maximum accepted HTTP request body size. |
| `http_request_timeout_ms` | `30000` | Maximum HTTP request duration. |
| `http_rate_limit_enabled` | `false` | Enables a simple global HTTP token-bucket rate limit. |
| `http_rate_limit_requests_per_second` | `1000` | Global HTTP rate-limit refill rate. |
| `http_rate_limit_burst` | `1000` | Global HTTP rate-limit burst capacity. |
| `http_principal_rate_limit_enabled` | `false` | Enables per-principal HTTP token-bucket rate limiting. |
| `http_principal_rate_limit_requests_per_second` | `100` | Per-principal HTTP rate-limit refill rate. |
| `http_principal_rate_limit_burst` | `200` | Per-principal HTTP rate-limit burst capacity. |
| `logging_enabled` | `true` | Enables structured server logging. |
| `log_format` | `"compact"` | Log output format. Values: `compact`, `json`. |
| `log_level` | `"info"` | Tracing filter level. |
| `log_destination` | `"stderr"` | Log destination. Values: `stderr`, `stdout`, `file`, `none`. |
| `log_file_path` | `null` | Required when log destination is `file`. |
| `require_auth` | `false` | Rejects unauthenticated requests when true. |
| `bearer_token` | `null` | Static full-admin bearer token. |
| `disable_bearer_token` | `false` | Disables static bearer-token authentication even when configured. |
| `jwt_secret` | `null` | HMAC JWT verification secret. |
| `jwt_public_key_pem` | `null` | PEM public key for asymmetric JWT validation. |
| `jwt_issuer` | `null` | Expected JWT issuer. |
| `jwt_audience` | `null` | Expected JWT audience. |
| `jwt_algorithm` | `null` | JWT algorithm override. |
| `jwks_url` | `null` | JWKS endpoint URL. |
| `jwks_provider_id` | `null` | Provider ID for logs/docs. |
| `jwks_refresh_interval_seconds` | `300` | JWKS background refresh interval. |
| `jwks_cache_ttl_seconds` | `3600` | JWKS cache TTL. |
| `jwks_http_timeout_ms` | `3000` | JWKS HTTP request timeout. |
| `jwt_leeway_seconds` | `0` | JWT clock-skew leeway. |

Use `latlng-server --print-config-reference` or `latlng-cli config-reference` to inspect the machine-readable reference for the installed binary. Operational guidance and storage config shapes are documented in [docs/config.md](docs/config.md).

## Transport And Auth

HTTP:

- implemented in `latlng-http`
- supports static bearer token auth, HMAC JWTs, PEM-configured asymmetric JWTs, and JWKS-backed asymmetric JWTs
- static bearer token remains a full-admin service/dev token unless `disable_bearer_token` is enabled
- production guardrails can require an auth source with `require_auth`, `LATLNG_REQUIRE_AUTH=1`, or `--require-auth`
- claims-based authz is collection-scoped and uses the `latlng_permissions` claim
- `queries:read` and `subscriptions:read` are separate scopes
- `/metrics` returns Prometheus text exposition and `metrics:read` is separate from `admin:*`
- See [metrics.md](docs/metrics.md) for the Prometheus metric contract.
- `latlng-server` currently serves plain HTTP, WebSocket, and Cap'n Proto; production deployments should terminate TLS at an upstream reverse proxy, load balancer, ingress, or service mesh
- bearer/JWT credentials should only cross trusted networks or TLS-terminated paths

WebSocket:

- implemented in `latlng-ws`
- supports `auth`, `subscribe`, `psubscribe`, `ping`, and `quit` command envelopes
- accepts bearer/JWT auth during the upgrade path or through the first `auth` frame
- enforces `subscriptions:read` separately from request/response query access
- streams geofence events from the shared registry used by the other transports

Cap'n Proto:

- implemented in `latlng-capnp`
- uses generated schema bindings from `crates/latlng-schema/schema/latlng.capnp`
- runs on real async `capnp-rpc`, not the previous blocking framed transport
- uses session auth via the `auth(token)` RPC when bearer/JWT auth is enabled, then enforces the same action-level authz model as the native HTTP routes
- `timeout`, `configRewrite`, `readonly`, and the other shipped admin RPCs route into the same runtime config model as HTTP
- also exposes the internal native-only replication stream used by followers
- disabled by default; enable with `capnp_enabled = true`, `LATLNG_CAPNP_ENABLED=true`, or `--capnp-enabled=true` when Cap'n Proto clients or replication are needed

CLI:

- uses typed `clap` subcommands with `--help` output for command documentation
- automatically attaches `Authorization: Bearer ...` when `LATLNG_TOKEN` is set
- can generate HMAC JWT secrets and scoped JWTs for local or self-hosted deployments
- covers `ping`, `healthz`, `server`, `info`, `collections`, `metrics`, `bounds`, `stats`, `get`, `set-point`, `nearby`, `config-get`, `config-set`, `config-validate`, `config-reference`, `config-rewrite`, `readonly`, `timeout`, `aofshrink`, `aof-verify`, `aof-backup`, and `aof-restore`

Create a scoped HMAC JWT:

```sh
latlng-cli token secret > .latlng-jwt-secret
```

```toml
require_auth = true
disable_bearer_token = true
jwt_secret = "<contents of .latlng-jwt-secret>"
jwt_algorithm = "HS256"
jwt_issuer = "https://id.example.com"
jwt_audience = "latlng"
```

```sh
TOKEN="$(latlng-cli token create \
  --config ./latlng.toml \
  --subject dashboard-1 \
  --ttl 24h \
  --preset dashboard \
  --collection 'fleet-*')"

LATLNG_TOKEN="$TOKEN" latlng-cli collections
latlng-cli token verify "$TOKEN" --config ./latlng.toml
```

External IdPs can be integrated through JWKS. In that mode the IdP issues access tokens,
and `latlng-server` verifies them with `jwt_issuer`, `jwt_audience`, `jwt_algorithm`,
and `jwks_url`. The access token must include `latlng_permissions` or `latlng_admin`.
See [docs/auth.md](docs/auth.md#using-an-external-idp-with-jwks).

Full auth/authz documentation, claim examples, and config reference:

- [docs/auth.md](docs/auth.md)
- [docs/config.md](docs/config.md)

Native query execution:

- `latlng-server` now enables the internal `parallel` query feature by default
- large `NEARBY`, `WITHIN`, `INTERSECTS`, `SCAN`, and `SEARCH` queries snapshot only their prefiltered candidate set, then run native-only parallel candidate evaluation while preserving deterministic ordering and cursor behavior
- wasm builds stay on the serial path and do not depend on rayon

## Storage And Eventing

Storage modes:

- default: in-memory
- AOF server mode: set `LATLNG_AOF_PATH=/path/to/latlng.aof`
- SQLite: use `latlng-storage-sqlite` directly from embedded/native applications
- JSON/TOML config files can also select storage mode and auth/runtime settings
- `require_auth` fails startup when no bearer token or JWT verifier is configured
- `subscriber_queue_capacity` controls the bounded per-subscriber event mailbox size and defaults to `4096`
- `native_executor_threads` controls the number of dedicated native core worker threads and defaults to available CPU parallelism
- `native_executor_queue_limit` controls the bounded native core submission queue and defaults to `native_executor_threads * 64`
- `webhook_timeout_ms` controls the HTTP request timeout for outbound webhook delivery and defaults to `5000`
- `webhook_concurrency_limit` controls the maximum number of concurrent outbound webhook deliveries and defaults to `128`
- `webhook_queue_path` controls the SQLite materialized queue path used by the durable webhook outbox
- `webhook_retry_count` defaults to `8` retries after the initial attempt
- `webhook_retry_initial_backoff_ms` defaults to `200`
- `webhook_retry_max_backoff_ms` defaults to `30000`
- `webhook_lease_ms` defaults to `30000`
- `aof_writer_queue_limit` controls the bounded submission queue for the AOF writer thread and defaults to `4096`
- `aof_group_commit_delay_ms` controls how long the AOF writer waits to coalesce concurrent append requests and defaults to `1`
- `aof_group_commit_max_requests` caps how many logical append requests can share one durable sync cycle and defaults to `128`
- `server_id` uniquely identifies the node for replication self-checks and reconnect validation
- `follow_host` / `follow_port` configure follower mode at startup
- `replication_credential` configures the dedicated follower-to-leader authentication secret
- `replication_batch_size` controls how many storage entries are fetched per replication chunk and defaults to `512`
- `replication_reconnect_backoff_ms` controls follower reconnect delay and defaults to `1000`
- `http_cors_enabled` enables HTTP CORS; keep it disabled unless browsers need direct access
- `http_cors_allowed_origins`, `http_cors_allowed_methods`, `http_cors_allowed_headers`, and `http_cors_max_age_seconds` define the CORS policy
- `http_rate_limit_enabled`, `http_rate_limit_requests_per_second`, and `http_rate_limit_burst` configure a process-global limiter for accidental overload protection
- `http_principal_rate_limit_enabled`, `http_principal_rate_limit_requests_per_second`, and `http_principal_rate_limit_burst` configure per-principal HTTP buckets for JWT subjects, static bearer service traffic, open access, and anonymous/invalid requests
- `logging_enabled`, `log_format`, `log_level`, `log_destination`, and `log_file_path` configure structured HTTP, WebSocket, and Cap'n Proto access logs
- durable webhook recovery across restart requires a durable primary log, so use AOF-backed server storage for that guarantee

You can set the AOF tuning values in all three configuration layers:

- config file fields:
  - `aof_writer_queue_limit`
  - `aof_group_commit_delay_ms`
  - `aof_group_commit_max_requests`
- env vars:
  - `LATLNG_AOF_WRITER_QUEUE_LIMIT`
  - `LATLNG_AOF_GROUP_COMMIT_DELAY_MS`
  - `LATLNG_AOF_GROUP_COMMIT_MAX_REQUESTS`
- CLI flags:
  - `--aof-writer-queue-limit`
  - `--aof-group-commit-delay-ms`
  - `--aof-group-commit-max-requests`

Example config:

```toml
listen_addr = "127.0.0.1:7421"
capnp_enabled = false
capnp_listen_addr = "127.0.0.1:7422"

[storage]
type = "aof"
path = "/var/lib/latlng/appendonly.aof"

aof_writer_queue_limit = 4096
aof_group_commit_delay_ms = 1
aof_group_commit_max_requests = 128
```

Equivalent env/CLI overrides:

```sh
LATLNG_AOF_WRITER_QUEUE_LIMIT=4096 \
LATLNG_AOF_GROUP_COMMIT_DELAY_MS=1 \
LATLNG_AOF_GROUP_COMMIT_MAX_REQUESTS=128 \
latlng-server \
  --aof /var/lib/latlng/appendonly.aof \
  --aof-writer-queue-limit 4096 \
  --aof-group-commit-delay-ms 1 \
  --aof-group-commit-max-requests 128
```

Validate a config before deployment:

```sh
latlng-server --config /etc/latlng/server.toml --check-config
latlng-cli config-validate /etc/latlng/server.toml
latlng-cli config-reference
```

Enable browser CORS and JSON access logs:

```toml
http_cors_enabled = true
http_cors_allowed_origins = ["https://app.example.com"]
http_cors_allowed_methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
http_cors_allowed_headers = ["authorization", "content-type", "x-request-id"]
http_cors_max_age_seconds = 600

logging_enabled = true
log_format = "json"
log_level = "info"
log_destination = "file"
log_file_path = "/var/log/latlng/server.log"
```

Geofence and hook behavior:

- channel geofences are registered inside the core engine and exposed through WebSocket and Cap'n Proto streams
- hook and channel definitions are persisted in the primary log and replayed on restart
- webhook enqueue intents, retries, acknowledgements, and dead-letter transitions are recorded in the primary log
- mutating commands and their webhook enqueue intents are persisted in one atomic storage batch on durable backends
- startup recovery applies primary-log entries incrementally as they are replayed instead of buffering the whole log first
- `latlng-server` rebuilds a SQLite webhook queue from that log on startup and dispatches due jobs from the queue
- replication is driven from committed storage entries, not the ephemeral channel/pubsub path
- followers authenticate with a dedicated replication credential, verify leader identity, and resume from the local last sequence when checksum verification matches
- checksum mismatch triggers a full local reset/resync from sequence `0`
- followers are forced into read-only mode while following and reject normal reads until they have caught up once
- followers do not deliver durable webhooks from replicated log records; webhook dispatch stays leader-local
- WebSocket and Cap'n Proto subscription streams are wake-driven on native instead of using fixed poll intervals
- the native webhook outbox is wake-driven too: new work, queue rebuilds, and due retry deadlines wake the dispatcher instead of a steady idle poll
- request-style native HTTP, WebSocket, and Cap'n Proto core calls use the dedicated native executor; long-lived subscription bridges and background outbox work stay outside that pool
- outbound webhook requests use the configured `webhook_timeout_ms` timeout and are processed concurrently up to `webhook_concurrency_limit`
- failed deliveries use exponential backoff and become dead-lettered after `webhook_retry_count` retries
- webhook delivery is `at-least-once`; payloads and headers include stable event/job IDs for receiver-side deduplication
- roaming geofences, `ROAM`, and `NODWELL` logic are implemented in the portable geofence layer
- WebSocket and Cap'n Proto event streams are exercised against the same live server in integration tests to keep payload parity honest
- subscriber mailboxes are bounded, in-memory queues; when full they drop the oldest events
- `FLUSHDB` is a full reset: it clears collections, channel geofences, webhook geofences, geofence state, and the durable webhook queue
- live subscribers stay connected across `FLUSHDB`, but any buffered pre-flush events are discarded so post-flush streams only contain post-flush state

AOF behavior:

- complete but corrupt entries fail startup with a codec error
- a truncated final AOF frame is ignored during replay so crash-tail recovery can still rebuild the valid prefix
- when the truncated frame was a batched write, the whole batch is discarded rather than replaying a partial command-plus-webhook tail
- concurrent AOF appends are funneled through a dedicated writer thread and can share one flush/sync cycle without changing the “success means durable” contract
- logical compaction preserves current objects, active hooks/channels, and unresolved webhook jobs
- offline integrity verification reports entry count, sequence range, durable prefix bytes, truncated-tail status, and checksum
- offline backups are inspectable JSON files with version, source path, timestamp, sequence, and checksum metadata
- restores refuse to overwrite an existing target unless `--force` is supplied

## Performance Benchmarks

Two benchmark layers now exist:

- `tools/latlng-benchmark`: in-process engine-level benchmarking for `latlng-core`
- `tools/latlng-server-benchmark`: black-box localhost benchmarking for the real `latlng-server` binary over HTTP

The server benchmark tool is manual/local only for now. It is intentionally not wired into standard CI or nightly automation. Benchmark JSON outputs are written into the local `benchmark-results/` directory, which is intentionally gitignored.
Benchmark binaries are local engineering tools and are intentionally not included in GitHub release binary archives. Release archives contain only `latlng-server` and `latlng-cli`; `openapi.json` is attached separately to GitHub Releases.

Build and run it with the Makefile entry points:

```sh
make bench-server
make bench-server-capnp
make bench-server-aof
make bench-server-tile38
make bench-server-compare OLD=benchmark-results/bench-server-memory.json NEW=benchmark-results/bench-server-aof.json
make bench-server-compare-capnp OLD=benchmark-results/bench-server-memory.json NEW=benchmark-results/bench-server-capnp.json
make bench-server-compare-tile38 OLD=benchmark-results/bench-server-memory.json NEW=benchmark-results/bench-server-tile38.json
```

Useful overrides:

```sh
make bench-server BENCH_FLAGS="--warmup-secs 1 --measure-secs 2 --seed-objects 1000 --startup-records 1000"
make bench-server-capnp BENCH_FLAGS="--warmup-secs 1 --measure-secs 2 --seed-objects 1000"
make bench-server-aof BENCH_FLAGS="--concurrency-list 8,32 --measure-secs 10"
make bench-server-tile38 BENCH_FLAGS="--tile38-server-bin /usr/local/bin/tile38-server --scenario get_object_read"
```

The benchmark tool reports:

- throughput in `ops/sec`
- mean, `p50`, `p95`, and `p99` latency
- error counts
- AOF startup replay duration as a separate scenario

Latlng runs default to HTTP; pass `--latlng-transport capnp` or use `make bench-server-capnp` to isolate protocol overhead from core engine work.
Tile38 runs use `tile38-server` by default and write a separate JSON file with `engine: "tile38"`.
The default Tile38 mode is in-memory-style `--appendonly no`; pass `--tile38-appendonly yes` when comparing Tile38 AOF behavior. The startup replay scenario is latlng-only and is skipped for Tile38 runs. The standard scenario set includes the geofence-heavy `fenced_set_point_write` case.

Interpret the results as local engineering signals for before/after comparison, not as product SLA numbers.

## Verification

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo test -p latlng-server --test server_smoke
cargo check --target wasm32-unknown-unknown -p latlng-core --features wasm-bindings
cd packages/sdk && npm ci && npm run typecheck && npm run build && npm run docs:api && npm run test
cd packages/wasm && npm ci && npm run typecheck && npm run build && npm run test
cd packages/example-wasm && npm ci && npm run typecheck && npm run build
```
