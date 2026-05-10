# Authentication And Authorization

`latlng` supports shared authentication configuration across native HTTP, WebSocket, and Cap'n Proto.

## Authentication Modes

Supported modes:

- static bearer token
- HMAC JWT verification
- PEM-configured asymmetric JWT verification
- JWKS-configured asymmetric JWT verification

Authentication is optional by default for local development. If no bearer token and no JWT verification source are configured, the server is open. Production deployments should enable `require_auth` and configure JWT verification or a static bearer token with trusted upstream TLS.

`latlng-server` currently serves plain HTTP, WebSocket, and Cap'n Proto. Production deployments should terminate TLS upstream at a reverse proxy, load balancer, ingress, or service mesh. Bearer tokens and JWTs should only cross trusted networks or TLS-terminated paths.

Static bearer token behavior:

- the configured bearer token is a full-admin service/dev token
- it remains supported indefinitely
- production deployments can disable it entirely with `disable_bearer_token`

JWT verification rule:

- exactly one JWT verification source may be configured:
  - `jwt_secret`
  - `jwt_public_key_pem`
  - `jwks_url`

## Authorization Model

The recommended deployment model is one tenant per `latlng` instance.

Inside that instance, authorization is collection-scoped through JWT claims.

Supported action groups:

- `collections:list`
- `collections:create`
- `collections:delete`
- `collections:inspect`
- `objects:read`
- `objects:write`
- `objects:delete`
- `queries:read`
- `subscriptions:read`
- `hooks:manage`
- `channels:manage`
- `metrics:read`
- `admin:*`

Important semantics:

- `queries:read` and `subscriptions:read` are separate
- `metrics:read` is separate from `admin:*`
- `hooks:manage` and `channels:manage` are separate
- collection listing requires explicit `collections:list`
- `/collections` is filtered to visible collections only
- hooks/channels list responses are filtered to resources the principal may manage

## Claim Shape

Use the `latlng_permissions` array claim.

Example: read-only access to one collection

```json
{
  "sub": "user-123",
  "iss": "https://id.example.com",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_permissions": [
    {
      "collections": ["fleet-eu"],
      "actions": ["collections:list", "objects:read", "queries:read"]
    }
  ]
}
```

Example: live subscriptions

```json
{
  "sub": "dashboard-1",
  "iss": "https://id.example.com",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_permissions": [
    {
      "collections": ["fleet-*"],
      "actions": ["collections:list", "subscriptions:read"]
    }
  ]
}
```

Example: hook management

```json
{
  "sub": "ops-1",
  "iss": "https://id.example.com",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_permissions": [
    {
      "collections": ["fleet-eu"],
      "actions": ["hooks:manage"]
    }
  ]
}
```

Example: metrics without admin

```json
{
  "sub": "metrics-collector",
  "iss": "https://id.example.com",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_permissions": [
    {
      "collections": ["*"],
      "actions": ["metrics:read"]
    }
  ]
}
```

Example: admin

```json
{
  "sub": "admin-1",
  "iss": "https://id.example.com",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_admin": true
}
```

## Route Semantics

High-level mapping:

- object CRUD routes use `objects:*`
- collection create/delete/inspect routes use `collections:*`
- spatial/text query routes use `queries:read`
- WebSocket `subscribe`/`psubscribe` use `subscriptions:read`
- hook routes use `hooks:manage`
- channel routes use `channels:manage`
- `/metrics` uses `metrics:read`
- The emitted Prometheus metric contract is documented in [metrics.md](metrics.md).
- operational/admin routes use `admin:*`

## Native Configuration

Configuration precedence:

1. defaults
2. config file
3. environment variables
4. CLI flags

Config file fields:

- `require_auth`
- `bearer_token`
- `disable_bearer_token`
- `jwt_secret`
- `jwt_public_key_pem`
- `jwt_issuer`
- `jwt_audience`
- `jwt_algorithm`
- `jwt_leeway_seconds`
- `jwks_url`
- `jwks_provider_id`
- `jwks_refresh_interval_seconds`
- `jwks_cache_ttl_seconds`
- `jwks_http_timeout_ms`

Environment variables:

- `LATLNG_REQUIRE_AUTH`
- `LATLNG_BEARER_TOKEN`
- `LATLNG_DISABLE_BEARER_TOKEN`
- `LATLNG_JWT_SECRET`
- `LATLNG_JWT_PUBLIC_KEY_PEM`
- `LATLNG_JWT_ISSUER`
- `LATLNG_JWT_AUDIENCE`
- `LATLNG_JWT_ALGORITHM`
- `LATLNG_JWT_LEEWAY_SECONDS`
- `LATLNG_JWKS_URL`
- `LATLNG_JWKS_PROVIDER_ID`
- `LATLNG_JWKS_REFRESH_INTERVAL_SECONDS`
- `LATLNG_JWKS_CACHE_TTL_SECONDS`
- `LATLNG_JWKS_HTTP_TIMEOUT_MS`

CLI flags:

- `--require-auth`
- `--bearer-token`
- `--disable-bearer-token`
- `--jwt-secret`
- `--jwt-public-key-pem`
- `--jwt-issuer`
- `--jwt-audience`
- `--jwt-algorithm`
- `--jwt-leeway`
- `--jwks-url`
- `--jwks-provider-id`
- `--jwks-refresh-interval-seconds`
- `--jwks-cache-ttl-seconds`
- `--jwks-http-timeout-ms`

Example HMAC config:

```toml
[auth]
disable_bearer_token = true
jwt_secret = "replace-me"
jwt_algorithm = "HS256"
jwt_issuer = "https://id.example.com"
jwt_audience = "latlng"
jwt_leeway_seconds = 5
```

Example PEM config:

```toml
[auth]
disable_bearer_token = true
jwt_public_key_pem = """
-----BEGIN PUBLIC KEY-----
...
-----END PUBLIC KEY-----
"""
jwt_algorithm = "RS256"
jwt_issuer = "https://id.example.com"
jwt_audience = "latlng"
```

Example JWKS config:

```toml
[auth]
disable_bearer_token = true
jwt_algorithm = "RS256"
jwt_issuer = "https://id.example.com"
jwt_audience = "latlng"
jwks_url = "https://id.example.com/.well-known/jwks.json"
jwks_provider_id = "primary-idp"
jwks_refresh_interval_seconds = 300
jwks_cache_ttl_seconds = 3600
jwks_http_timeout_ms = 3000
```

## JWKS Behavior

`jwks_provider_id` is documentation/logging metadata only. It does not affect verification semantics.

JWKS behavior is fail-closed:

- if `jwks_url` is configured and key resolution fails, authentication fails
- if the JWKS HTTP request fails, authentication fails
- there is no fallback to bearer or another JWT source for that token

Recommended defaults:

- `jwks_refresh_interval_seconds = 300`
- `jwks_cache_ttl_seconds = 3600`
- `jwks_http_timeout_ms = 3000`

## SDK Expectations

`@latlng/sdk` transports bearer tokens. It does not mint or refresh JWTs for you.

Typical model:

- local/dev: static bearer token
- production: external token issuer or IdP mints JWTs
- SDK caller attaches the token to HTTP and WebSocket calls

## Operational Guidance

- use static bearer token for local development, tests, and tightly controlled service flows
- prefer JWTs for production user/service access
- prefer asymmetric JWT verification for production
- prefer JWKS when keys are managed by an external IdP
- pair `latlng` auth with TLS or trusted upstream TLS termination in production
- use edge per-client or per-token rate limiting for internet-facing deployments; the native limiter is process-global
