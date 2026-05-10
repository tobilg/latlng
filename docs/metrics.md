# Metrics

`GET /metrics` exposes Prometheus text exposition from the native HTTP server.

- Content type: `text/plain; version=0.0.4; charset=utf-8`
- Required scope when auth is enabled: `metrics:read`
- Labels: none currently
- Stability: metric names and types are stable within `0.x` unless release notes call out a change

| Metric | Type | Description |
| --- | --- | --- |
| `latlng_http_requests_total` | counter | Total HTTP requests handled by the native server. |
| `latlng_http_unauthorized_total` | counter | HTTP requests rejected as unauthenticated. |
| `latlng_http_forbidden_total` | counter | HTTP requests rejected by authorization. |
| `latlng_http_server_errors_total` | counter | HTTP requests that returned a 5xx status. |
| `latlng_http_global_rate_limited_total` | counter | HTTP requests rejected by the global rate limiter. |
| `latlng_http_principal_rate_limited_total` | counter | HTTP requests rejected by the per-principal rate limiter. |
| `latlng_http_request_duration_ms` | histogram | HTTP request duration in milliseconds. |
| `latlng_hook_attempts_total` | counter | Webhook delivery attempts. |
| `latlng_hook_success_total` | counter | Successful webhook deliveries. |
| `latlng_hook_failure_total` | counter | Failed webhook deliveries. |
| `latlng_hook_retry_total` | counter | Webhook deliveries scheduled for retry. |
| `latlng_hook_dead_letter_total` | counter | Webhook deliveries moved to the dead-letter state. |
| `latlng_hook_delivery_duration_ms` | histogram | Webhook delivery attempt duration in milliseconds. |
| `latlng_webhook_jobs_pending` | gauge | Current pending webhook jobs. |
| `latlng_webhook_jobs_leased` | gauge | Current leased webhook jobs. |
| `latlng_webhook_jobs_dead_letter` | gauge | Current dead-letter webhook jobs. |
| `latlng_webhook_oldest_pending_age_ms` | gauge | Oldest pending webhook job age in milliseconds. Omitted from the exposition when no pending job exists. |
| `latlng_replication_role` | gauge | Replication role: `0` means leader, `1` means follower. |
| `latlng_replication_caught_up` | gauge | `1` when the node is caught up with its leader, otherwise `0`. |
| `latlng_replication_local_last_sequence` | gauge | Last local committed sequence known to replication. |
| `latlng_replication_leader_last_sequence` | gauge | Last leader sequence observed by replication. |
| `latlng_replication_lag_sequences` | gauge | Difference between leader and local sequence. |
| `latlng_replication_reconnects_total` | counter | Replication reconnect attempts after follow failures. |
| `latlng_replication_checksum_mismatches_total` | counter | Replication checksum mismatches detected during resume. |
| `latlng_replication_resyncs_total` | counter | Full resyncs triggered by replication safety checks. |

Prometheus scrape example:

```yaml
scrape_configs:
  - job_name: latlng
    metrics_path: /metrics
    scheme: http
    bearer_token: "${LATLNG_METRICS_TOKEN}"
    static_configs:
      - targets: ["127.0.0.1:7421"]
```
