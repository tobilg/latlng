use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use latlng_replication::ReplicationStatus;

#[derive(Default)]
pub struct RequestMetrics {
    pub(crate) requests_total: AtomicU64,
    pub(crate) unauthorized_total: AtomicU64,
    pub(crate) forbidden_total: AtomicU64,
    pub(crate) server_errors_total: AtomicU64,
    http_global_rate_limited_total: AtomicU64,
    http_principal_rate_limited_total: AtomicU64,
    hook_attempts_total: AtomicU64,
    hook_success_total: AtomicU64,
    hook_failure_total: AtomicU64,
    hook_retry_total: AtomicU64,
    hook_dead_letter_total: AtomicU64,
    webhook_jobs_pending: AtomicU64,
    webhook_jobs_leased: AtomicU64,
    webhook_jobs_dead_letter: AtomicU64,
    webhook_oldest_pending_age_ms: AtomicU64,
    http_request_duration_ms: LatencyHistogram,
    hook_delivery_duration_ms: LatencyHistogram,
}

const LATENCY_BUCKETS_MS: [u64; 12] =
    [1, 5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];

#[derive(Debug)]
struct LatencyHistogram {
    buckets: [AtomicU64; LATENCY_BUCKETS_MS.len()],
    count: AtomicU64,
    sum_ms: AtomicU64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; LATENCY_BUCKETS_MS.len()],
            count: AtomicU64::new(0),
            sum_ms: AtomicU64::new(0),
        }
    }
}

impl LatencyHistogram {
    fn observe_ms(&self, value: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_ms.fetch_add(value, Ordering::Relaxed);
        for (index, bucket) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if value <= *bucket {
                self.buckets[index].fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl RequestMetrics {
    pub fn snapshot(&self) -> serde_json::Value {
        let oldest_pending_age_ms = self.webhook_oldest_pending_age_ms.load(Ordering::Relaxed);
        serde_json::json!({
            "requests_total": self.requests_total.load(Ordering::Relaxed),
            "unauthorized_total": self.unauthorized_total.load(Ordering::Relaxed),
            "forbidden_total": self.forbidden_total.load(Ordering::Relaxed),
            "server_errors_total": self.server_errors_total.load(Ordering::Relaxed),
            "global_rate_limited_total": self.http_global_rate_limited_total.load(Ordering::Relaxed),
            "principal_rate_limited_total": self.http_principal_rate_limited_total.load(Ordering::Relaxed),
            "hook_attempts_total": self.hook_attempts_total.load(Ordering::Relaxed),
            "hook_success_total": self.hook_success_total.load(Ordering::Relaxed),
            "hook_failure_total": self.hook_failure_total.load(Ordering::Relaxed),
            "hook_retry_total": self.hook_retry_total.load(Ordering::Relaxed),
            "hook_dead_letter_total": self.hook_dead_letter_total.load(Ordering::Relaxed),
            "webhook_jobs_pending": self.webhook_jobs_pending.load(Ordering::Relaxed),
            "webhook_jobs_leased": self.webhook_jobs_leased.load(Ordering::Relaxed),
            "webhook_jobs_dead_letter": self.webhook_jobs_dead_letter.load(Ordering::Relaxed),
            "webhook_oldest_pending_age_ms": if oldest_pending_age_ms == u64::MAX {
                serde_json::Value::Null
            } else {
                serde_json::json!(oldest_pending_age_ms)
            },
            "http_request_duration_count": self.http_request_duration_ms.count.load(Ordering::Relaxed),
            "hook_delivery_duration_count": self.hook_delivery_duration_ms.count.load(Ordering::Relaxed),
        })
    }

    pub fn prometheus_text(&self) -> String {
        self.prometheus_text_with_replication(None, None)
    }

    pub fn prometheus_text_with_replication(
        &self,
        replication: Option<&ReplicationStatus>,
        local_last_sequence: Option<u64>,
    ) -> String {
        let counters = [
            (
                "latlng_http_requests_total",
                "Total HTTP requests handled by the native server.",
                self.requests_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_http_unauthorized_total",
                "HTTP requests rejected as unauthenticated.",
                self.unauthorized_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_http_forbidden_total",
                "HTTP requests rejected by authorization.",
                self.forbidden_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_http_server_errors_total",
                "HTTP requests that returned a 5xx status.",
                self.server_errors_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_http_global_rate_limited_total",
                "HTTP requests rejected by the global rate limiter.",
                self.http_global_rate_limited_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_http_principal_rate_limited_total",
                "HTTP requests rejected by the per-principal rate limiter.",
                self.http_principal_rate_limited_total
                    .load(Ordering::Relaxed),
            ),
            (
                "latlng_hook_attempts_total",
                "Webhook delivery attempts.",
                self.hook_attempts_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_hook_success_total",
                "Successful webhook deliveries.",
                self.hook_success_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_hook_failure_total",
                "Failed webhook deliveries.",
                self.hook_failure_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_hook_retry_total",
                "Webhook deliveries scheduled for retry.",
                self.hook_retry_total.load(Ordering::Relaxed),
            ),
            (
                "latlng_hook_dead_letter_total",
                "Webhook deliveries moved to the dead-letter state.",
                self.hook_dead_letter_total.load(Ordering::Relaxed),
            ),
        ];
        let gauges = [
            (
                "latlng_webhook_jobs_pending",
                "Current pending webhook jobs.",
                Some(self.webhook_jobs_pending.load(Ordering::Relaxed)),
            ),
            (
                "latlng_webhook_jobs_leased",
                "Current leased webhook jobs.",
                Some(self.webhook_jobs_leased.load(Ordering::Relaxed)),
            ),
            (
                "latlng_webhook_jobs_dead_letter",
                "Current dead-letter webhook jobs.",
                Some(self.webhook_jobs_dead_letter.load(Ordering::Relaxed)),
            ),
            (
                "latlng_webhook_oldest_pending_age_ms",
                "Oldest pending webhook job age in milliseconds.",
                match self.webhook_oldest_pending_age_ms.load(Ordering::Relaxed) {
                    u64::MAX => None,
                    value => Some(value),
                },
            ),
        ];

        let mut output = String::new();
        for (name, help, value) in counters {
            push_metric(&mut output, name, help, "counter", Some(value));
        }
        for (name, help, value) in gauges {
            push_metric(&mut output, name, help, "gauge", value);
        }
        push_histogram(
            &mut output,
            "latlng_http_request_duration_ms",
            "HTTP request duration in milliseconds.",
            &self.http_request_duration_ms,
        );
        push_histogram(
            &mut output,
            "latlng_hook_delivery_duration_ms",
            "Webhook delivery attempt duration in milliseconds.",
            &self.hook_delivery_duration_ms,
        );
        if let Some(replication) = replication {
            let role = if replication.is_follower() { 1 } else { 0 };
            let local = local_last_sequence.unwrap_or(replication.local_last_sequence);
            let leader = replication.leader_last_sequence.max(local);
            push_metric(
                &mut output,
                "latlng_replication_role",
                "Replication role as a gauge: 0 means leader, 1 means follower.",
                "gauge",
                Some(role),
            );
            push_metric(
                &mut output,
                "latlng_replication_caught_up",
                "Whether the node is caught up with its leader.",
                "gauge",
                Some(u64::from(replication.caught_up)),
            );
            push_metric(
                &mut output,
                "latlng_replication_local_last_sequence",
                "Last local committed sequence known to replication.",
                "gauge",
                Some(local),
            );
            push_metric(
                &mut output,
                "latlng_replication_leader_last_sequence",
                "Last leader sequence observed by replication.",
                "gauge",
                Some(leader),
            );
            push_metric(
                &mut output,
                "latlng_replication_lag_sequences",
                "Leader sequence minus local sequence.",
                "gauge",
                Some(leader.saturating_sub(local)),
            );
            push_metric(
                &mut output,
                "latlng_replication_reconnects_total",
                "Replication reconnect attempts after follow failures.",
                "counter",
                Some(replication.reconnects_total),
            );
            push_metric(
                &mut output,
                "latlng_replication_checksum_mismatches_total",
                "Replication checksum mismatches detected during resume.",
                "counter",
                Some(replication.checksum_mismatches_total),
            );
            push_metric(
                &mut output,
                "latlng_replication_resyncs_total",
                "Replication full resyncs triggered by safety checks.",
                "counter",
                Some(replication.resyncs_total),
            );
        }
        output
    }

    pub fn record_hook_attempt(&self) {
        self.hook_attempts_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hook_success(&self) {
        self.hook_success_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hook_failure(&self) {
        self.hook_failure_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hook_retry(&self) {
        self.hook_retry_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hook_dead_letter(&self) {
        self.hook_dead_letter_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_http_request_duration(&self, duration: Duration) {
        self.http_request_duration_ms
            .observe_ms(duration.as_millis() as u64);
    }

    pub(crate) fn record_http_global_rate_limited(&self) {
        self.http_global_rate_limited_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_http_principal_rate_limited(&self) {
        self.http_principal_rate_limited_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hook_delivery_duration(&self, duration: Duration) {
        self.hook_delivery_duration_ms
            .observe_ms(duration.as_millis() as u64);
    }

    pub fn update_webhook_queue_stats(
        &self,
        pending: u64,
        leased: u64,
        dead_letter: u64,
        oldest_pending_age_ms: Option<u64>,
    ) {
        self.webhook_jobs_pending.store(pending, Ordering::Relaxed);
        self.webhook_jobs_leased.store(leased, Ordering::Relaxed);
        self.webhook_jobs_dead_letter
            .store(dead_letter, Ordering::Relaxed);
        self.webhook_oldest_pending_age_ms
            .store(oldest_pending_age_ms.unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

fn push_metric(output: &mut String, name: &str, help: &str, metric_type: &str, value: Option<u64>) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push(' ');
    output.push_str(metric_type);
    output.push('\n');
    if let Some(value) = value {
        output.push_str(name);
        output.push(' ');
        output.push_str(&value.to_string());
        output.push('\n');
    }
}

fn push_histogram(output: &mut String, name: &str, help: &str, histogram: &LatencyHistogram) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push_str(" histogram\n");
    for (index, bucket) in LATENCY_BUCKETS_MS.iter().enumerate() {
        output.push_str(name);
        output.push_str("_bucket{le=\"");
        output.push_str(&bucket.to_string());
        output.push_str("\"} ");
        output.push_str(&histogram.buckets[index].load(Ordering::Relaxed).to_string());
        output.push('\n');
    }
    output.push_str(name);
    output.push_str("_bucket{le=\"+Inf\"} ");
    output.push_str(&histogram.count.load(Ordering::Relaxed).to_string());
    output.push('\n');
    output.push_str(name);
    output.push_str("_sum ");
    output.push_str(&histogram.sum_ms.load(Ordering::Relaxed).to_string());
    output.push('\n');
    output.push_str(name);
    output.push_str("_count ");
    output.push_str(&histogram.count.load(Ordering::Relaxed).to_string());
    output.push('\n');
}
