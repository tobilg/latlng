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
disable_bearer_token = true
jwt_secret = "replace-me"
jwt_algorithm = "HS256"
jwt_issuer = "https://id.example.com"
jwt_audience = "latlng"
jwt_leeway_seconds = 5
```

## Creating HMAC JWTs With `latlng-cli`

`latlng-cli` can generate HMAC JWT secrets and mint scoped JWTs that use the existing
`latlng_permissions` claim shape. This is intended for local and self-hosted deployments
where `latlng-server` verifies tokens with `jwt_secret`. It does not mint asymmetric
JWTs or publish JWKS keys.

Generate a strong HMAC secret:

```sh
latlng-cli token secret > .latlng-jwt-secret
```

Configure the server with that secret:

```toml
disable_bearer_token = true
jwt_secret = "<contents of .latlng-jwt-secret>"
jwt_algorithm = "HS256"
jwt_issuer = "https://id.example.com"
jwt_audience = "latlng"
```

Create a token from the config:

```sh
TOKEN="$(latlng-cli token create \
  --config ./latlng.toml \
  --subject dashboard-1 \
  --ttl 24h \
  --preset dashboard \
  --collection 'fleet-*')"
```

Use the token:

```sh
LATLNG_TOKEN="$TOKEN" latlng-cli collections
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7421/ping
```

Verify a token locally before handing it to a client:

```sh
latlng-cli token verify "$TOKEN" --config ./latlng.toml
```

Inspect a token without verifying its signature:

```sh
latlng-cli token inspect "$TOKEN"
```

Secret sources:

- `--config ./latlng.toml` reads `jwt_secret`, `jwt_algorithm`, `jwt_issuer`, and `jwt_audience`
- `--secret-env NAME` reads the HMAC secret from an environment variable
- `--secret-file PATH` reads the HMAC secret from a file and strips trailing newlines
- `--secret-stdin` reads the HMAC secret from standard input and strips trailing newlines
- `LATLNG_JWT_SECRET` is used as a fallback when no config secret or explicit secret source is provided

Token output formats:

```sh
latlng-cli token create ... --format token
latlng-cli token create ... --format json
latlng-cli token create ... --format env
latlng-cli token create ... --format curl
```

Permission presets:

| Preset | Collections | Actions |
| --- | --- | --- |
| `readonly` | requires `--collection` | `collections:list`, `collections:inspect`, `objects:read`, `queries:read` |
| `writer` | requires `--collection` | `collections:list`, `collections:inspect`, `objects:read`, `objects:write`, `objects:delete`, `queries:read` |
| `dashboard` | requires `--collection` | `collections:list`, `queries:read`, `subscriptions:read` |
| `hooks-admin` | requires `--collection` | `hooks:manage` |
| `channels-admin` | requires `--collection` | `channels:manage` |
| `metrics` | always `*` | `metrics:read` |

You can also pass explicit actions:

```sh
latlng-cli token create \
  --config ./latlng.toml \
  --subject writer-1 \
  --ttl 2h \
  --collection fleet-eu \
  --action collections:list \
  --action objects:read \
  --action objects:write
```

Create a full-admin JWT only when you intentionally need one:

```sh
latlng-cli token create --config ./latlng.toml --subject admin-1 --ttl 15m --admin
```

Troubleshooting:

- `unauthorized` during verification usually means the wrong secret, algorithm, issuer, audience, or an expired token
- `forbidden` from the server means the token is valid but lacks the required action or collection pattern
- `metrics:read` must be global; use the `metrics` preset instead of a collection-specific rule
- configs using `jwt_public_key_pem` or `jwks_url` can verify tokens but cannot be used by the CLI to mint HMAC JWTs

Example PEM config:

```toml
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

## Using An External IdP With JWKS

`latlng` can verify JWT access tokens issued by an external identity provider. In this
mode, `latlng-server` is a resource server:

- the IdP authenticates users or services
- the IdP issues signed JWT access tokens
- clients send those tokens to `latlng` as bearer tokens
- `latlng` fetches signing keys from the IdP's JWKS endpoint and enforces the
  `latlng_permissions` or `latlng_admin` claims in the token

`latlng` does not redirect users, run an OAuth login flow, call token introspection,
fetch userinfo, or map IdP groups/scopes to permissions. The access token itself must
contain the `latlng` authorization claims.

Basic setup:

1. Create an API/resource in your IdP for `latlng`.
2. Choose the expected audience, for example `latlng`.
3. Configure the IdP to sign access tokens with `RS256` or `ES256`.
4. Add a token mapper, rule, action, hook, or custom claim transform that emits
   `latlng_permissions` or `latlng_admin` into access tokens.
5. Configure `latlng-server` with the issuer, audience, algorithm, and JWKS URL.

Use the IdP's OpenID Connect discovery document if it provides one. The discovery
document is commonly available at:

```text
<issuer>/.well-known/openid-configuration
```

Use these values from that document:

- `issuer` -> `jwt_issuer`
- `jwks_uri` -> `jwks_url`

The configured `jwt_issuer` must match the token's `iss` claim exactly. The configured
`jwt_audience` must match the token's `aud` claim.

Example resource-server config:

```toml
require_auth = true
disable_bearer_token = true

jwt_algorithm = "RS256"
jwt_issuer = "https://idp.example.com/realms/latlng"
jwt_audience = "latlng"

jwks_url = "https://idp.example.com/realms/latlng/protocol/openid-connect/certs"
jwks_provider_id = "primary-idp"
jwks_refresh_interval_seconds = 300
jwks_cache_ttl_seconds = 3600
jwks_http_timeout_ms = 3000
```

Example access token payload:

```json
{
  "sub": "user-123",
  "iss": "https://idp.example.com/realms/latlng",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_permissions": [
    {
      "collections": ["fleet-*"],
      "actions": ["collections:list", "objects:read", "queries:read"]
    }
  ]
}
```

Admin access uses `latlng_admin`:

```json
{
  "sub": "ops-admin-1",
  "iss": "https://idp.example.com/realms/latlng",
  "aud": "latlng",
  "exp": 4102444800,
  "latlng_admin": true
}
```

Clients use the IdP-issued access token exactly like any other bearer token:

```sh
curl -H "Authorization: Bearer $ACCESS_TOKEN" http://127.0.0.1:7421/collections
LATLNG_TOKEN="$ACCESS_TOKEN" latlng-cli collections
```

For SDK clients:

```ts
import { LatLngClient } from "@latlng/sdk";

const client = new LatLngClient({
  leaderUrl: "https://latlng.example.com",
  token: accessTokenFromYourIdp,
});
```

Provider notes:

- Keycloak-style setups usually add `latlng_permissions` with a client scope or protocol mapper.
- Auth0/Okta-style setups usually add it with an action, rule, authorization server claim, or access-token custom claim.
- Some IdPs restrict arbitrary top-level custom claims. If your IdP cannot emit `latlng_permissions` or `latlng_admin` as top-level claims in an access token, use a small token broker or edge service to exchange the IdP token for a `latlng` JWT.
- Use access tokens, not ID tokens. ID tokens are meant for the client application and often have the wrong audience.

Troubleshooting IdP tokens:

- `401 unauthorized` usually means signature verification failed, the token is expired, the `kid` is missing or not present in JWKS, `iss` or `aud` does not match, or the configured algorithm does not match the token header.
- `403 forbidden` means the token is valid but lacks the required `latlng_permissions` action or collection pattern.
- If key rotation breaks auth, check that `jwks_refresh_interval_seconds`, `jwks_cache_ttl_seconds`, and IdP JWKS cache headers are compatible with your IdP's rotation policy.
- `latlng-cli token create` is only for HMAC JWTs. It does not mint IdP/JWKS tokens.

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
