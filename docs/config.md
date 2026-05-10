# latlng Configuration

`latlng-server` reads JSON or TOML configuration from `--config` or `LATLNG_CONFIG`.
Environment variables and explicit CLI flags override file values.

Generate the machine-readable reference with:

```sh
latlng-server --print-config-reference
latlng-cli config-reference
```

Validate a deployment config with:

```sh
latlng-server --config ./latlng.toml --check-config
latlng-cli config-validate ./latlng.toml
```

## Production Guardrails

Set `production_mode = true` to convert security-sensitive warnings into startup failures.
In production mode, `latlng` rejects configs with disabled auth, static-bearer-only auth,
JWT/JWKS without issuer or audience checks, non-HTTPS JWKS URLs, wildcard CORS with auth,
or weak follower replication credentials.

For local development, keep `production_mode = false`. The same issues are reported as
warnings in `--check-config`, `config-validate`, and startup logs.

## HTTP Limits

The native HTTP server supports configurable request limits:

```toml
http_max_body_bytes = 10485760
http_request_timeout_ms = 30000
http_rate_limit_enabled = false
http_rate_limit_requests_per_second = 1000
http_rate_limit_burst = 1000
http_principal_rate_limit_enabled = false
http_principal_rate_limit_requests_per_second = 100
http_principal_rate_limit_burst = 200
```

The global limiter is a coarse process-wide overload guard. The optional per-principal
limiter buckets JWT callers by `sub`, JWTs without `sub` by a token fingerprint, static
bearer traffic as `bearer:service`, open-access traffic as `open:access`, and missing or
invalid auth as `anonymous`.

## TLS

`latlng-server` currently serves plain HTTP and WebSocket. Cap'n Proto is also plain TCP
when `capnp_enabled = true`. Production deployments should terminate TLS upstream at a
reverse proxy, load balancer, ingress, or service mesh. Native TLS support is intentionally
deferred until there is a concrete deployment requirement.

Bearer tokens and JWTs should only cross trusted networks or TLS-terminated paths.

## Auth

Static bearer tokens are full-admin credentials. They are useful for local development and
tests, but production deployments should prefer JWT or JWKS validation and set
`disable_bearer_token = true`.

JWT/JWKS deployments should configure both:

```toml
jwt_issuer = "https://issuer.example"
jwt_audience = "latlng"
```

JWKS deployments should use HTTPS:

```toml
jwks_url = "https://issuer.example/.well-known/jwks.json"
jwks_provider_id = "issuer-example"
```

## Complete Server Config Reference

The table below mirrors `latlng-server --print-config-reference`.
`default` is shown as JSON-compatible data even when the config file is TOML.
Host-dependent defaults are shown symbolically.

| Name | Kind | Default | Description |
| --- | --- | --- | --- |
| `production_mode` | `bool` | `false` | Enables strict production startup guardrails. |
| `listen_addr` | `string` | `"127.0.0.1:7421"` | HTTP listen address. |
| `capnp_enabled` | `bool` | `false` | Enables the Cap'n Proto RPC and replication listener. |
| `capnp_listen_addr` | `string` | `"127.0.0.1:7422"` | Cap'n Proto listen address. |
| `server_id` | `string` | `"<generated uuid>"` | Stable server identity used in replication status. |
| `storage` | `storage_mode` | `"memory"` | Storage backend. Use memory or aof with a path. |
| `read_only` | `bool` | `false` | Rejects mutating commands when true. |
| `command_timeouts` | `map<string,float>` | `{}` | Per-command timeout overrides in seconds. |
| `subscriber_queue_capacity` | `usize` | `4096` | Per-subscriber event queue capacity. |
| `webhook_queue_path` | `path|null` | `null` | SQLite webhook queue path. Defaults near the AOF or current directory. |
| `webhook_timeout_ms` | `u64` | `5000` | HTTP timeout for webhook deliveries. |
| `webhook_concurrency_limit` | `usize` | `128` | Maximum concurrent webhook delivery attempts. |
| `webhook_retry_count` | `u32` | `8` | Maximum webhook retry attempts before dead-lettering. |
| `webhook_retry_initial_backoff_ms` | `u64` | `200` | Initial webhook retry backoff. |
| `webhook_retry_max_backoff_ms` | `u64` | `30000` | Maximum webhook retry backoff. |
| `webhook_lease_ms` | `u64` | `30000` | Webhook job lease duration. |
| `native_executor_threads` | `usize` | `<available CPU parallelism>` | Native worker thread count for core operations. |
| `native_executor_queue_limit` | `usize` | `<native_executor_threads * 64>` | Native executor queue limit. |
| `aof_writer_queue_limit` | `usize` | `4096` | AOF writer queue limit. |
| `aof_group_commit_delay_ms` | `u64` | `1` | Maximum AOF group commit delay. |
| `aof_group_commit_max_requests` | `usize` | `128` | Maximum requests per AOF commit cycle. |
| `follow_host` | `string|null` | `null` | Leader host for follower replication. |
| `follow_port` | `u16|null` | `null` | Leader Cap'n Proto port for follower replication. |
| `replication_credential` | `string|null` | `null` | Dedicated credential for replication streams. |
| `replication_batch_size` | `usize` | `512` | Maximum entries per replication stream response. |
| `replication_reconnect_backoff_ms` | `u64` | `1000` | Follower reconnect backoff after failures. |
| `http_cors_enabled` | `bool` | `false` | Enables HTTP CORS middleware. |
| `http_cors_allowed_origins` | `list<string>` | `[]` | Allowed CORS origins. Avoid `*` with auth. |
| `http_cors_allowed_methods` | `list<string>` | `["GET","POST","PUT","DELETE","OPTIONS"]` | Allowed CORS methods. |
| `http_cors_allowed_headers` | `list<string>` | `["authorization","content-type","x-request-id"]` | Allowed CORS headers. |
| `http_cors_max_age_seconds` | `u64|null` | `null` | Optional CORS preflight cache max-age. |
| `http_max_body_bytes` | `usize` | `10485760` | Maximum accepted HTTP request body size. |
| `http_request_timeout_ms` | `u64` | `30000` | Maximum HTTP request duration. |
| `http_rate_limit_enabled` | `bool` | `false` | Enables a simple global HTTP token-bucket rate limit. |
| `http_rate_limit_requests_per_second` | `u64` | `1000` | Global HTTP rate-limit refill rate. |
| `http_rate_limit_burst` | `u64` | `1000` | Global HTTP rate-limit burst capacity. |
| `http_principal_rate_limit_enabled` | `bool` | `false` | Enables per-principal HTTP token-bucket rate limiting. |
| `http_principal_rate_limit_requests_per_second` | `u64` | `100` | Per-principal HTTP rate-limit refill rate. |
| `http_principal_rate_limit_burst` | `u64` | `200` | Per-principal HTTP rate-limit burst capacity. |
| `logging_enabled` | `bool` | `true` | Enables structured server logging. |
| `log_format` | `compact\|json` | `"compact"` | Log output format. |
| `log_level` | `string` | `"info"` | Tracing filter level. |
| `log_destination` | `stderr\|stdout\|file\|none` | `"stderr"` | Log destination. |
| `log_file_path` | `path|null` | `null` | Required when log destination is `file`. |
| `require_auth` | `bool` | `false` | Rejects unauthenticated requests when true. |
| `bearer_token` | `string|null` | `null` | Static full-admin bearer token. |
| `disable_bearer_token` | `bool` | `false` | Disables static bearer-token authentication even when configured. |
| `jwt_secret` | `string|null` | `null` | HMAC JWT verification secret. |
| `jwt_public_key_pem` | `string|null` | `null` | PEM public key for asymmetric JWT validation. |
| `jwt_issuer` | `string|null` | `null` | Expected JWT issuer. |
| `jwt_audience` | `string|null` | `null` | Expected JWT audience. |
| `jwt_algorithm` | `string|null` | `null` | JWT algorithm override. |
| `jwks_url` | `string|null` | `null` | JWKS endpoint URL. |
| `jwks_provider_id` | `string|null` | `null` | Provider ID for logs/docs. |
| `jwks_refresh_interval_seconds` | `u64` | `300` | JWKS background refresh interval. |
| `jwks_cache_ttl_seconds` | `u64` | `3600` | JWKS cache TTL. |
| `jwks_http_timeout_ms` | `u64` | `3000` | JWKS HTTP request timeout. |
| `jwt_leeway_seconds` | `u64` | `0` | JWT clock-skew leeway. |

### Storage Config Shape

In JSON, memory storage can be configured as:

```json
{ "storage": { "type": "memory" } }
```

AOF storage can be configured as:

```json
{ "storage": { "type": "aof", "path": "./data/appendonly.aof" } }
```

In TOML, the equivalent AOF shape is:

```toml
[storage]
type = "aof"
path = "./data/appendonly.aof"
```

The generated config reference remains canonical for the installed binary:

```sh
latlng-server --print-config-reference
latlng-cli config-reference
```
