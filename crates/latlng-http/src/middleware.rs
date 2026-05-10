use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use latlng_auth::{AuthError, AuthPrincipal, Authenticator, extract_bearer_token};
use latlng_core::storage::StorageBackend;
use tracing::info;
use uuid::Uuid;

use crate::{HttpError, HttpState, RequestMetrics, json_error_response};

#[derive(Debug)]
pub(crate) struct GlobalRateLimiter {
    state: Mutex<RateLimitState>,
    capacity: f64,
    refill_per_second: f64,
}

#[derive(Debug)]
struct RateLimitState {
    tokens: f64,
    last_refill: Instant,
}

impl GlobalRateLimiter {
    pub(crate) fn new(requests_per_second: u64, burst: u64) -> Self {
        let capacity = burst.max(1) as f64;
        Self {
            state: Mutex::new(RateLimitState {
                tokens: capacity,
                last_refill: Instant::now(),
            }),
            capacity,
            refill_per_second: requests_per_second.max(1) as f64,
        }
    }

    fn allow(&self) -> bool {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.refill_per_second).min(self.capacity);
        state.last_refill = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub(crate) struct PrincipalRateLimiter {
    state: Mutex<PrincipalRateLimitState>,
    capacity: f64,
    refill_per_second: f64,
}

#[derive(Debug)]
struct PrincipalRateLimitState {
    buckets: HashMap<String, RateLimitState>,
    last_cleanup: Instant,
}

const PRINCIPAL_BUCKET_IDLE_TTL: Duration = Duration::from_secs(10 * 60);
const PRINCIPAL_BUCKET_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const PRINCIPAL_BUCKET_MAX_BUCKETS: usize = 16_384;

impl PrincipalRateLimiter {
    pub(crate) fn new(requests_per_second: u64, burst: u64) -> Self {
        let now = Instant::now();
        Self {
            state: Mutex::new(PrincipalRateLimitState {
                buckets: HashMap::new(),
                last_cleanup: now,
            }),
            capacity: burst.max(1) as f64,
            refill_per_second: requests_per_second.max(1) as f64,
        }
    }

    fn allow(&self, key: &str) -> bool {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        if now.duration_since(state.last_cleanup) >= PRINCIPAL_BUCKET_CLEANUP_INTERVAL
            || state.buckets.len() > PRINCIPAL_BUCKET_MAX_BUCKETS
        {
            cleanup_principal_buckets(&mut state, now);
        }
        let bucket = state
            .buckets
            .entry(key.to_owned())
            .or_insert_with(|| RateLimitState {
                tokens: self.capacity,
                last_refill: now,
            });
        allow_bucket(bucket, now, self.capacity, self.refill_per_second)
    }
}

fn cleanup_principal_buckets(state: &mut PrincipalRateLimitState, now: Instant) {
    state
        .buckets
        .retain(|_, bucket| now.duration_since(bucket.last_refill) < PRINCIPAL_BUCKET_IDLE_TTL);
    if state.buckets.len() > PRINCIPAL_BUCKET_MAX_BUCKETS {
        let mut oldest = state
            .buckets
            .iter()
            .map(|(key, bucket)| (key.clone(), bucket.last_refill))
            .collect::<Vec<_>>();
        oldest.sort_unstable_by_key(|(_, last_refill)| *last_refill);
        let remove_count = state.buckets.len() - PRINCIPAL_BUCKET_MAX_BUCKETS;
        for (key, _) in oldest.into_iter().take(remove_count) {
            state.buckets.remove(&key);
        }
    }
    state.last_cleanup = now;
}

fn allow_bucket(
    bucket: &mut RateLimitState,
    now: Instant,
    capacity: f64,
    refill_per_second: f64,
) -> bool {
    let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * refill_per_second).min(capacity);
    bucket.last_refill = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

pub(crate) struct RateLimitMiddlewareState {
    pub(crate) limiter: Arc<GlobalRateLimiter>,
    pub(crate) metrics: Option<Arc<RequestMetrics>>,
}

#[derive(Clone)]
pub(crate) struct PrincipalRateLimitMiddlewareState {
    pub(crate) limiter: Arc<PrincipalRateLimiter>,
    pub(crate) auth: Authenticator,
    pub(crate) metrics: Option<Arc<RequestMetrics>>,
}

pub(crate) async fn max_body_size_middleware(
    State(max_body_bytes): State<usize>,
    request: Request,
    next: Next,
) -> Response {
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > max_body_bytes)
    {
        return json_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("request body exceeds {max_body_bytes} bytes"),
        );
    }
    next.run(request).await
}

pub(crate) async fn request_timeout_middleware(
    State(timeout_ms): State<u64>,
    request: Request,
    next: Next,
) -> Response {
    match tokio::time::timeout(Duration::from_millis(timeout_ms), next.run(request)).await {
        Ok(response) => response,
        Err(_) => json_error_response(StatusCode::REQUEST_TIMEOUT, "request timed out"),
    }
}

pub(crate) async fn rate_limit_middleware(
    State(state): State<Arc<RateLimitMiddlewareState>>,
    request: Request,
    next: Next,
) -> Response {
    if !state.limiter.allow() {
        if let Some(metrics) = &state.metrics {
            metrics.record_http_global_rate_limited();
        }
        return rate_limit_response("global rate limit exceeded");
    }
    next.run(request).await
}

pub(crate) async fn principal_rate_limit_middleware(
    State(state): State<Arc<PrincipalRateLimitMiddlewareState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token)
        .map(str::to_owned);
    match state.auth.authenticate(token.as_deref()).await {
        Ok(principal) => {
            if !state.limiter.allow(&principal.rate_limit_key) {
                if let Some(metrics) = &state.metrics {
                    metrics.record_http_principal_rate_limited();
                }
                return rate_limit_response("principal rate limit exceeded");
            }
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(error) => {
            if !state
                .limiter
                .allow(&AuthPrincipal::anonymous().rate_limit_key)
            {
                if let Some(metrics) = &state.metrics {
                    metrics.record_http_principal_rate_limited();
                }
                return rate_limit_response("principal rate limit exceeded");
            }
            if let Some(metrics) = &state.metrics
                && matches!(error, AuthError::Unauthorized)
            {
                metrics.unauthorized_total.fetch_add(1, Ordering::Relaxed);
            }
            match error {
                AuthError::Unauthorized => HttpError::Unauthorized.into_response(),
                AuthError::InvalidConfiguration(_) | AuthError::UnsupportedAlgorithm(_) => {
                    HttpError::Internal(error.to_string()).into_response()
                }
            }
        }
    }
}

fn rate_limit_response(error: &'static str) -> Response {
    let mut response = json_error_response(StatusCode::TOO_MANY_REQUESTS, error);
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

pub(crate) async fn request_context_middleware(mut request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    request.extensions_mut().insert(request_id.clone());
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    info!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        duration_ms = started.elapsed().as_millis() as u64,
        "http request"
    );
    response
}

pub(crate) async fn metrics_middleware<S>(
    State(state): State<HttpState<S>>,
    request: Request,
    next: Next,
) -> Response
where
    S: StorageBackend + Send + Sync + 'static,
{
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    let response = next.run(request).await;
    state
        .metrics
        .record_http_request_duration(started.elapsed());
    if response.status() == StatusCode::UNAUTHORIZED {
        state
            .metrics
            .unauthorized_total
            .fetch_add(1, Ordering::Relaxed);
    }
    if response.status() == StatusCode::FORBIDDEN {
        state
            .metrics
            .forbidden_total
            .fetch_add(1, Ordering::Relaxed);
    }
    if response.status().is_server_error() {
        state
            .metrics
            .server_errors_total
            .fetch_add(1, Ordering::Relaxed);
    }
    response
}
