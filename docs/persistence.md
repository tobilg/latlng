# Persistence

`latlng` currently ships three storage modes:

- memory: ephemeral, process-local
- AOF: append-only file with replay and compaction
- SQLite: embedded/native backend

## AOF Format

`latlng-storage-aof` stores each physical frame as:

1. an unsigned 64-bit little-endian payload length
2. a `bincode`-encoded frame payload

Frame payloads are one of:

- a single `StorageEntry`
- an atomic batch of `StorageEntry` values
- a grouped commit frame containing multiple logical single-entry or batch requests that were durably synced together

`append_batch()` is a storage-level atomicity boundary. When the core persists a mutation plus one or more `WebhookEnqueue(...)` records, durable backends either replay the whole batch or none of it.

Compaction rewrites the full logical entry set into a temporary file and then renames it over the live file.

On native AOF-backed servers, append requests are funneled through a dedicated writer thread. That writer can coalesce multiple concurrent append requests into one durable flush/sync cycle, but each caller still waits for durability before it sees success.

## Recovery Behavior

- replay stops cleanly at a truncated final frame and keeps the valid prefix
- a complete but corrupt payload fails startup with a codec error
- a truncated final batch or grouped-commit frame is ignored as one unit, so the loader never replays only part of a logical command-plus-webhook batch
- startup replay is streaming/incremental: the core applies each entry as it is read instead of buffering the whole primary log first
- `snapshot()` and `compact()` rewrite the file contents rather than appending tombstones forever
- expirations are persisted as absolute deadlines in the primary log, so replay and `AOFSHRINK` preserve the original expiry instant instead of extending TTLs on restart
- SQLite uses a transaction for `append_batch()`, so the same atomic batch guarantee applies there as well

## Operational Guidance

- prefer AOF for the runnable server when durability matters
- keep the default safe fsync behavior unless you have measured reasons to relax it
- if you need to tune AOF throughput, adjust `aof_writer_queue_limit`, `aof_group_commit_delay_ms`, and `aof_group_commit_max_requests` before weakening durability behavior
- those AOF settings can be supplied through:
  - config file fields: `aof_writer_queue_limit`, `aof_group_commit_delay_ms`, `aof_group_commit_max_requests`
  - env vars: `LATLNG_AOF_WRITER_QUEUE_LIMIT`, `LATLNG_AOF_GROUP_COMMIT_DELAY_MS`, `LATLNG_AOF_GROUP_COMMIT_MAX_REQUESTS`
  - CLI flags: `--aof-writer-queue-limit`, `--aof-group-commit-delay-ms`, `--aof-group-commit-max-requests`
- if startup fails with a codec error, treat the tail as corrupt rather than silently discarding arbitrary complete entries
- use `AOFSHRINK` after heavy churn to rewrite the active state into a compact file
- use `latlng-cli aof-verify <path>` before startup or maintenance windows to inspect the durable prefix, last sequence, entry count, checksum, and whether a torn final frame was ignored
- use `latlng-cli aof-backup <source.aof> <backup.json>` to create an offline JSON backup with version, timestamp, sequence, and checksum metadata
- use `latlng-cli aof-restore <backup.json> <target.aof>` to restore into a new AOF path; add `--force` only when intentionally replacing an existing target

## Backup Guidance

`latlng-cli aof-backup` is an offline tool. Use it against a stopped server or a copied AOF that is no longer being written.

Do not copy a live AOF file as the primary backup procedure. The AOF writer may be between frame writes, so a raw live copy can include a torn final frame. Startup recovery can ignore a truncated final frame, but an operator backup should still avoid racing the writer because it makes restore validation and incident analysis harder.

Recommended procedures:

- safest: send `SIGTERM`, wait for graceful shutdown, run `latlng-cli aof-verify`, then run `latlng-cli aof-backup`
- acceptable for filesystem snapshots: use storage-level snapshots that preserve point-in-time file consistency, then run `latlng-cli aof-verify` on the snapshot copy before treating it as a backup
- after restore: restore to a new AOF path, run `latlng-cli aof-verify`, then start the server against that restored path

Online, API-level snapshot shipping is not implemented yet. Until it exists, live backup procedures should be based on graceful shutdown or crash-consistent filesystem snapshots plus verification.

## What Is Not There Yet

- snapshot shipping
- configurable fsync policies beyond the current always-safe default
