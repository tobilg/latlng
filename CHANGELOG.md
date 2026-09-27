# Changelog

## 0.2.0 - 2026-09-27

### Breaking changes

- **SET merges fields.** A SET now keeps existing fields that are not in the request and adds
  or overwrites the ones that are; previously it replaced the whole field set, so
  position-only updates dropped all fields (which also made field-filtered geofences see
  objects without fields). The TTL is still replaced on every SET. Setting a numeric field to
  `0` deletes it, in SET and FSET, and a missing field now reads as `0` in `where`, `where_in`
  and expression filters. The log records the merged field set, so replaying existing AOF
  files is unaffected.
- **JSET and JDEL clear the TTL** and keep the fields. The TTL removal is logged alongside the
  edit, so replaying older logs keeps their TTLs.
- **HTTP status codes.** Client errors that returned `500` now return `404` (missing
  collection or object) or `400` (invalid geometry, regex, expression, or JSON-path edits on
  non-GeoJSON objects). `5xx` is reserved for storage and internal failures. Error messages
  for `4xx` responses no longer start with `internal error:`.
- **`GET /collections/{c}/objects/{id}` returns `404` for a missing object** instead of `200`
  with a `null` body. The TypeScript SDK's `get()` still resolves to `null`.
- **JGET on a non-GeoJSON object returns `400`** instead of `{"value": null}`, matching JSET
  and JDEL.
- **Unknown request fields are rejected.** HTTP request bodies, search options, nearby
  queries, field entries and geofence definitions reject unknown fields, so a misspelt key
  returns `400` instead of being ignored.
- **All HTTP error responses are JSON.** Malformed JSON, invalid path or query parameters,
  unknown routes (`404`) and wrong methods (`405`) return `{"error": "…"}`. A missing JSON
  content type returns `415`.
- **Coordinate and query validation.** New writes and queries reject latitudes outside
  [-90, 90], longitudes outside [-180, 180], non-finite numbers, non-positive radii, bearings
  outside [0, 360], and bounds with `min_lat > max_lat` or `min_lon > max_lon` (boxes crossing
  the antimeridian are not supported). Invalid regexes and expressions are rejected even when
  no object would be evaluated. Existing AOF data is loaded unchanged.
- **Hooks and channels with a `Reference` area are rejected** with `400` (see Fixed).
- **The server CLI rejects unknown flags and malformed values** with a non-zero exit.
  Previously they were silently ignored, so a typo such as `--reqiure-auth` started an
  unauthenticated server.
- **Expired objects fire `Del` geofence events** (see Active expiry under Added). Consumers that
  react to `Del` events will now also see them for expirations.

### Added

- **Active expiry.** Expired objects are deleted by a background sweep on the leader
  (`expiry_sweep_interval_ms`, default `100`, `0` disables it; env
  `LATLNG_EXPIRY_SWEEP_INTERVAL_MS`, flag `--expiry-sweep-interval-ms`). Each expiry goes
  through the normal delete path: it is logged, replicated to followers, and fires `Del`
  geofence events. Followers apply the leader's deletes instead of expiring objects
  themselves. The browser wasm package gains `expireDue()` (there is no background sweep in
  the browser), which returns the number of deleted objects and emits their `Del` geofence
  events.
- **Metrics:** `latlng_expired_objects_total` and `latlng_geofence_eval_errors_total`.
- **Cap'n Proto:** `OkResponse`, `SearchResponse` and the `get` results carry a new
  `code :ErrorCode` field (`none`, `notFound`, `badRequest`, `readOnly`, `internal`,
  `unauthorized`, `forbidden`, `unavailable`). The fields are appended, so older clients keep
  working.
- **WebSocket:** error frames include a machine-readable `code` next to `error`.
- **TypeScript SDK:** `BadRequestError` (400) and `NotFoundError` (404), both subclasses of
  `HttpError`.
- **Server CLI:** `--help`, `--flag=value` for every flag, and `--listen-addr` as an alias of
  `--listen`.

### Fixed

- **Hooks and channels can no longer block writes.** A geofence whose area is a `Reference`
  used to make every `SET` to the watched collection fail with `500`, even after a restart.
  Such definitions are now rejected at registration, and existing ones in an AOF are skipped
  on replay with a warning. A geofence that fails to evaluate is skipped instead of failing
  the write, and is counted in `latlng_geofence_eval_errors_total`.
- **Server CLI flags.** Flags are parsed with `clap`; the documented `--capnp-enabled=true`
  now works. Precedence is unchanged: defaults < config file < environment variables < CLI
  flags.
- **OpenAPI.** The scan and text-search request bodies are documented as the search options
  object itself instead of an `{"options": …}` wrapper. Areas, objects, fields, geofence
  definitions and search options now have real schemas, and `condition` is an enum.
- **Memory mode no longer writes to the working directory.** Without an explicit
  `webhook_queue_path`, memory storage keeps the webhook queue in memory.
- **JSET/JDEL validate before they are logged.** The edit is applied to a copy first; if the
  result has out-of-range coordinates or is no longer a valid geometry, or the path is
  invalid, the request fails with `400` and nothing is logged. Previously the command was
  logged before it could fail. Coordinate edits now also update the spatial index, so the
  object is found at its new position. Replayed JSET/JDEL commands are reindexed too; a legacy
  edit that produced an invalid geometry is left out of the spatial index instead of failing
  start-up.
- Replayed `FSET` commands now update field indexes.

### Build

- **wasm-bindgen 0.2.126.** The workspace pins `wasm-bindgen = "=0.2.126"` and
  `js-sys = "=0.3.103"`; building the wasm package requires `wasm-bindgen-cli` 0.2.126 (CI and
  release workflows updated).
