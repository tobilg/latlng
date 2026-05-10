#![forbid(unsafe_code)]

mod logging;
mod replication;
mod storage;

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use latlng_capnp::{CapnpAuthConfig, CapnpService};
use latlng_config::{
    FlushDbCoordinator, FlushDbFuture, LogDestination, LogFormat, RuntimeConfig,
    SharedRuntimeConfig, StorageMode as RuntimeStorageMode, config_reference_json, load_from_path,
};
use latlng_core::{
    Config as CoreConfig, LatLng, LatLngNative, LogRecord, WebhookAckRecord,
    WebhookDeadLetterRecord, WebhookRetryScheduledRecord,
};
use latlng_endpoints::deliver_event;
use latlng_http::{HttpState, RequestMetrics};
use latlng_native_executor::NativeExecutor;
use latlng_replication::SharedReplicationStatus;
use latlng_webhook_queue::{QueueJob, WebhookQueue};
use latlng_ws::WsState;
use logging::{init_logging, log_auth_mode, log_production_guardrails};
use replication::start_replication_manager;
use storage::ServerStorage;
use tokio::sync::{Notify, RwLock};
use tokio::task::{JoinSet, LocalSet};
use tracing::{error, info, warn};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("latlng-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if env::args().any(|arg| arg == "--print-config-reference") {
        println!(
            "{}",
            serde_json::to_string_pretty(&config_reference_json())?
        );
        return Ok(());
    }
    if env::args().any(|arg| arg == "--print-openapi") {
        println!(
            "{}",
            serde_json::to_string_pretty(&latlng_http::openapi_spec())?
        );
        return Ok(());
    }
    let config = read_config()?;
    config.validate_for_startup()?;
    if env::args().any(|arg| arg == "--check-config") {
        print_config_check(&config)?;
        return Ok(());
    }
    let _log_guard = init_logging(&config)?;
    log_auth_mode(&config);
    log_production_guardrails(&config);
    let runtime_config = Arc::new(std::sync::RwLock::new(config.clone()));
    let storage = ServerStorage::open(&config)?;

    let mut core_config = CoreConfig {
        read_only: config.read_only,
        subscriber_queue_capacity: config.subscriber_queue_capacity,
        webhook_retry_count: config.webhook_retry_count,
        ..CoreConfig::default()
    };
    core_config.config_file = config
        .config_path
        .as_ref()
        .map(|path| path.display().to_string());
    for (command, seconds) in &config.command_timeouts {
        core_config.set_timeout(command, *seconds);
    }

    let db: LatLngNative<ServerStorage> = LatLng::builder()
        .storage(storage)
        .config(core_config)
        .build()?;
    let shared = Arc::new(db);
    let executor = NativeExecutor::new(
        Arc::clone(&shared),
        config.native_executor_threads,
        config.native_executor_queue_limit,
    )?;
    let webhook_queue_path = resolve_webhook_queue_path(&config);
    if matches!(config.storage, RuntimeStorageMode::Memory) {
        warn!(
            queue = %webhook_queue_path.display(),
            "memory storage mode is active; durable webhook recovery across restarts is not guaranteed"
        );
    }
    let webhook_queue = Arc::new(WebhookQueue::open(&webhook_queue_path)?);
    let starts_following = config
        .follow_host
        .as_ref()
        .is_some_and(|host| !host.trim().is_empty())
        && config.follow_port.is_some_and(|port| port > 0);
    let last_webhook_seq = if starts_following {
        webhook_queue.reset()?;
        shared.last_sequence()
    } else {
        rebuild_webhook_queue_from_log(&shared, &webhook_queue).await?
    };

    let auth = config.auth.authenticator()?;
    let metrics = Arc::new(RequestMetrics::default());
    let outbox_control = Arc::new(OutboxControl::default());

    let shared_for_close = Arc::clone(&shared);
    {
        let local = LocalSet::new();
        local
            .run_until(async move {
            let replication_notify = Arc::new(Notify::new());
            let (replication_status, replication_coordinator) = start_replication_manager(
                Arc::clone(&shared),
                executor.clone(),
                Arc::clone(&runtime_config),
                outbox_control.notifier(),
            )
            .await
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
            let flushdb_coordinator = Arc::new(FlushDbCoordinatorImpl {
                db: Arc::clone(&shared),
                queue: Arc::clone(&webhook_queue),
                metrics: Arc::clone(&metrics),
                outbox_control: Arc::clone(&outbox_control),
                replication_notify: Arc::clone(&replication_notify),
            });

            let app = latlng_http::router(HttpState {
                db: Arc::clone(&shared),
                executor: executor.clone(),
                auth: auth.clone(),
                metrics: Arc::clone(&metrics),
                runtime_config: Some(Arc::clone(&runtime_config)),
                webhook_queue: Some(Arc::clone(&webhook_queue)),
                flushdb_coordinator: Some(flushdb_coordinator.clone()),
                outbox_notify: Some(outbox_control.notifier()),
                replication_status: Some(Arc::clone(&replication_status)),
                replication_coordinator: Some(Arc::clone(&replication_coordinator)),
                replication_notify: Some(Arc::clone(&replication_notify)),
            })
            .merge(latlng_ws::ws_route(WsState {
                db: Arc::clone(&shared),
                executor: executor.clone(),
                auth: auth.clone(),
            }));
            let app = latlng_http::apply_runtime_layers_with_context(
                app,
                &config,
                auth.clone(),
                Arc::clone(&metrics),
            )
                .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

            let hook_task = tokio::spawn(run_webhook_outbox(
                Arc::clone(&shared),
                Arc::clone(&webhook_queue),
                Arc::clone(&metrics),
                Arc::clone(&runtime_config),
                last_webhook_seq,
                Arc::clone(&outbox_control),
                Arc::clone(&replication_status),
            ));
            let capnp_task = if config.capnp_enabled {
                Some(tokio::task::spawn_local(run_capnp(
                    CapnpService::with_runtime_config_flushdb_coordinator_and_executor(
                        Arc::clone(&shared),
                        executor.clone(),
                        latlng_capnp::CapnpRuntimeBindings {
                            runtime_config: Some(Arc::clone(&runtime_config)),
                            flushdb_coordinator: Some(flushdb_coordinator),
                            outbox_notify: Some(outbox_control.notifier()),
                            replication_notify: Some(replication_notify),
                            replication_status: Some(replication_status),
                        },
                    ),
                    config.capnp_listen_addr.clone(),
                    auth.clone(),
                )))
            } else {
                None
            };

            let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
            info!(
                listen = %config.listen_addr,
                capnp_enabled = config.capnp_enabled,
                capnp_listen = if config.capnp_enabled { config.capnp_listen_addr.as_str() } else { "disabled" },
                "server listening"
            );
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
            if let Some(capnp_task) = capnp_task {
                capnp_task.abort();
                let _ = capnp_task.await;
            }
            hook_task.abort();
            let _ = hook_task.await;
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await?;
    }
    shared_for_close.close_storage()?;
    Ok(())
}

async fn run_webhook_outbox(
    db: Arc<LatLngNative<ServerStorage>>,
    queue: Arc<WebhookQueue>,
    metrics: Arc<RequestMetrics>,
    runtime_config: SharedRuntimeConfig,
    mut last_applied_sequence: u64,
    outbox_control: Arc<OutboxControl>,
    replication_status: SharedReplicationStatus,
) {
    let client = reqwest::Client::new();
    let worker_id = Uuid::new_v4().to_string();
    let mut in_flight = JoinSet::<WebhookAttemptResult>::new();
    let mut was_following = false;

    loop {
        let following = replication_status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_follower();
        if following {
            if !was_following {
                queue.reset().ok();
                refresh_queue_metrics(&queue, &metrics);
                last_applied_sequence = match tokio::task::spawn_blocking({
                    let db = Arc::clone(&db);
                    move || Ok::<u64, latlng_core::CoreError>(db.last_sequence())
                })
                .await
                {
                    Ok(Ok(sequence)) => sequence,
                    Ok(Err(error)) => {
                        error!(error = %error, "failed to inspect follower log position");
                        last_applied_sequence
                    }
                    Err(error) => {
                        error!(error = %error, "failed to inspect follower log position");
                        last_applied_sequence
                    }
                };
            }
            was_following = true;
            outbox_control.notify.notified().await;
            continue;
        }
        was_following = false;
        {
            let _guard = outbox_control.gate.read().await;
            if let Err(error) = apply_log_tail(&db, &queue, &mut last_applied_sequence).await {
                error!(error = %error, "failed to apply webhook log tail");
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            if let Err(error) = queue.release_expired_leases(now_ms()) {
                error!(error = %error, "failed to release expired webhook leases");
            }
        }
        refresh_queue_metrics(&queue, &metrics);
        drain_completed_hook_tasks(
            &mut in_flight,
            &db,
            &queue,
            &metrics,
            &runtime_config,
            &mut last_applied_sequence,
            &outbox_control,
        )
        .await;
        let concurrency_limit = webhook_concurrency_limit(&runtime_config);

        let mut spawned = false;
        while in_flight.len() < concurrency_limit {
            let available = concurrency_limit.saturating_sub(in_flight.len());
            let jobs = {
                let _guard = outbox_control.gate.read().await;
                match queue.lease_due(
                    available,
                    webhook_lease_ms(&runtime_config),
                    &worker_id,
                    now_ms(),
                ) {
                    Ok(jobs) => jobs,
                    Err(error) => {
                        error!(error = %error, "failed to lease webhook jobs");
                        break;
                    }
                }
            };
            if jobs.is_empty() {
                break;
            }
            let epoch = outbox_control.current_epoch();
            for job in jobs {
                let client = client.clone();
                let runtime_config = Arc::clone(&runtime_config);
                let outbox_control = Arc::clone(&outbox_control);
                let metrics_for_task = Arc::clone(&metrics);
                metrics.record_hook_attempt();
                in_flight.spawn(async move {
                    let _guard = outbox_control.gate.read().await;
                    if epoch != outbox_control.current_epoch() {
                        return WebhookAttemptResult {
                            job,
                            epoch,
                            delivery: WebhookDeliveryOutcome::Skipped,
                        };
                    }
                    let timeout = webhook_timeout(&runtime_config);
                    let started = std::time::Instant::now();
                    let delivery = deliver_event(&client, &job.endpoint, &job.event, timeout).await;
                    metrics_for_task.record_hook_delivery_duration(started.elapsed());
                    WebhookAttemptResult {
                        job,
                        epoch,
                        delivery: WebhookDeliveryOutcome::Attempted(delivery),
                    }
                });
                spawned = true;
            }
        }

        if spawned {
            continue;
        }

        if in_flight.len() >= concurrency_limit {
            if let Some(result) = in_flight.join_next().await {
                handle_join_result(
                    result,
                    &db,
                    &queue,
                    &metrics,
                    &runtime_config,
                    &mut last_applied_sequence,
                    &outbox_control,
                )
                .await;
            }
            continue;
        }

        let notified = outbox_control.notify.notified();
        tokio::pin!(notified);
        if in_flight.is_empty() {
            match queue.next_due_at_ms() {
                Ok(Some(next_due_at_ms)) => {
                    tokio::select! {
                        _ = &mut notified => {}
                        _ = tokio::time::sleep(Duration::from_millis(next_due_at_ms.saturating_sub(now_ms()))) => {}
                    }
                }
                Ok(None) => {
                    notified.await;
                }
                Err(error) => {
                    error!(error = %error, "failed to inspect next due webhook job");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            continue;
        }

        let next_due_at_ms = match queue.next_due_at_ms() {
            Ok(next_due_at_ms) => next_due_at_ms,
            Err(error) => {
                error!(error = %error, "failed to inspect next due webhook job");
                None
            }
        };
        tokio::select! {
            result = in_flight.join_next() => {
                if let Some(result) = result {
                    handle_join_result(
                        result,
                        &db,
                        &queue,
                        &metrics,
                        &runtime_config,
                        &mut last_applied_sequence,
                        &outbox_control,
                    )
                    .await;
                }
            }
            _ = &mut notified => {}
            _ = tokio::time::sleep(Duration::from_millis(next_due_at_ms.map(|due| due.saturating_sub(now_ms())).unwrap_or(u64::MAX))), if next_due_at_ms.is_some() => {}
        }
    }
}

async fn run_capnp(
    service: CapnpService<ServerStorage>,
    listen_addr: String,
    auth: CapnpAuthConfig,
) {
    if let Err(error) = service.serve(&listen_addr, auth).await {
        error!(listen = %listen_addr, error = %error, "capnp service stopped");
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn read_config() -> Result<RuntimeConfig, Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let config_path = cli_value(&args, "--config")
        .map(std::path::PathBuf::from)
        .or_else(|| env::var("LATLNG_CONFIG").ok().map(std::path::PathBuf::from));
    let mut config = if let Some(path) = &config_path {
        load_from_path(path)?
    } else {
        RuntimeConfig::default()
    };
    config.assign_path(config_path);

    if let Ok(value) = env::var("LATLNG_LISTEN") {
        config.listen_addr = value;
    }
    if let Ok(value) = env::var("LATLNG_CAPNP_ENABLED") {
        config.capnp_enabled = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_CAPNP_LISTEN") {
        config.capnp_listen_addr = value;
    }
    if let Ok(value) = env::var("LATLNG_SERVER_ID") {
        config.server_id = value;
    }
    if let Ok(value) = env::var("LATLNG_AOF_PATH") {
        config.storage = RuntimeStorageMode::Aof { path: value.into() };
    }
    if let Ok(value) = env::var("LATLNG_BEARER_TOKEN") {
        config.auth.bearer_token = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_DISABLE_BEARER_TOKEN") {
        config.auth.disable_bearer_token = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_JWT_SECRET") {
        config.auth.jwt_secret = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWT_PUBLIC_KEY_PEM") {
        config.auth.jwt_public_key_pem = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWT_ISSUER") {
        config.auth.jwt_issuer = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWT_AUDIENCE") {
        config.auth.jwt_audience = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWT_ALGORITHM") {
        config.auth.jwt_algorithm = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWT_LEEWAY_SECONDS") {
        config.auth.jwt_leeway_seconds = value.parse().unwrap_or(0);
    }
    if let Ok(value) = env::var("LATLNG_JWKS_URL") {
        config.auth.jwks_url = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWKS_PROVIDER_ID") {
        config.auth.jwks_provider_id = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_JWKS_REFRESH_INTERVAL_SECONDS") {
        config.auth.jwks_refresh_interval_seconds =
            parse_u64_or(value.as_str(), config.auth.jwks_refresh_interval_seconds);
    }
    if let Ok(value) = env::var("LATLNG_JWKS_CACHE_TTL_SECONDS") {
        config.auth.jwks_cache_ttl_seconds =
            parse_u64_or(value.as_str(), config.auth.jwks_cache_ttl_seconds);
    }
    if let Ok(value) = env::var("LATLNG_JWKS_HTTP_TIMEOUT_MS") {
        config.auth.jwks_http_timeout_ms =
            parse_u64_or(value.as_str(), config.auth.jwks_http_timeout_ms);
    }
    if let Ok(value) = env::var("LATLNG_READ_ONLY") {
        config.read_only = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_SUBSCRIBER_QUEUE_CAPACITY") {
        config.subscriber_queue_capacity =
            parse_usize_or(value.as_str(), config.subscriber_queue_capacity);
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_QUEUE_PATH") {
        config.webhook_queue_path = Some(value.into());
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_TIMEOUT_MS") {
        config.webhook_timeout_ms = parse_u64_or(value.as_str(), config.webhook_timeout_ms);
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_CONCURRENCY_LIMIT") {
        config.webhook_concurrency_limit =
            parse_usize_or(value.as_str(), config.webhook_concurrency_limit);
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_RETRY_COUNT") {
        config.webhook_retry_count = parse_u32_or(value.as_str(), config.webhook_retry_count);
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_RETRY_INITIAL_BACKOFF_MS") {
        config.webhook_retry_initial_backoff_ms =
            parse_u64_or(value.as_str(), config.webhook_retry_initial_backoff_ms);
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_RETRY_MAX_BACKOFF_MS") {
        config.webhook_retry_max_backoff_ms =
            parse_u64_or(value.as_str(), config.webhook_retry_max_backoff_ms);
    }
    if let Ok(value) = env::var("LATLNG_WEBHOOK_LEASE_MS") {
        config.webhook_lease_ms = parse_u64_or(value.as_str(), config.webhook_lease_ms);
    }
    if let Ok(value) = env::var("LATLNG_NATIVE_EXECUTOR_THREADS") {
        config.native_executor_threads =
            parse_usize_or(value.as_str(), config.native_executor_threads);
    }
    if let Ok(value) = env::var("LATLNG_NATIVE_EXECUTOR_QUEUE_LIMIT") {
        config.native_executor_queue_limit =
            parse_usize_or(value.as_str(), config.native_executor_queue_limit);
    }
    if let Ok(value) = env::var("LATLNG_AOF_WRITER_QUEUE_LIMIT") {
        config.aof_writer_queue_limit =
            parse_usize_or(value.as_str(), config.aof_writer_queue_limit);
    }
    if let Ok(value) = env::var("LATLNG_AOF_GROUP_COMMIT_DELAY_MS") {
        config.aof_group_commit_delay_ms =
            parse_u64_or(value.as_str(), config.aof_group_commit_delay_ms);
    }
    if let Ok(value) = env::var("LATLNG_AOF_GROUP_COMMIT_MAX_REQUESTS") {
        config.aof_group_commit_max_requests =
            parse_usize_or(value.as_str(), config.aof_group_commit_max_requests);
    }
    if let Ok(value) = env::var("LATLNG_FOLLOW_HOST") {
        config.follow_host = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_FOLLOW_PORT") {
        config.follow_port = value.parse::<u16>().ok().filter(|port| *port > 0);
    }
    if let Ok(value) = env::var("LATLNG_REPLICATION_CREDENTIAL") {
        config.replication_credential = Some(value);
    }
    if let Ok(value) = env::var("LATLNG_REPLICATION_BATCH_SIZE") {
        config.replication_batch_size =
            parse_usize_or(value.as_str(), config.replication_batch_size);
    }
    if let Ok(value) = env::var("LATLNG_REPLICATION_RECONNECT_BACKOFF_MS") {
        config.replication_reconnect_backoff_ms =
            parse_u64_or(value.as_str(), config.replication_reconnect_backoff_ms);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_CORS_ENABLED") {
        config.http_cors_enabled = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_CORS_ALLOWED_ORIGINS") {
        config.http_cors_allowed_origins = parse_csv(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_CORS_ALLOWED_METHODS") {
        config.http_cors_allowed_methods = parse_csv(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_CORS_ALLOWED_HEADERS") {
        config.http_cors_allowed_headers = parse_csv(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_CORS_MAX_AGE_SECONDS") {
        config.http_cors_max_age_seconds = value.trim().parse::<u64>().ok();
    }
    if let Ok(value) = env::var("LATLNG_LOGGING_ENABLED") {
        config.logging_enabled = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_LOG_FORMAT") {
        config.log_format =
            parse_log_format(&value).ok_or_else(|| format!("unsupported log format: {value}"))?;
    }
    if let Ok(value) = env::var("LATLNG_LOG_LEVEL") {
        config.log_level = value;
    }
    if let Ok(value) = env::var("LATLNG_LOG_DESTINATION") {
        config.log_destination = parse_log_destination(&value)
            .ok_or_else(|| format!("unsupported log destination: {value}"))?;
    }
    if let Ok(value) = env::var("LATLNG_LOG_FILE_PATH") {
        config.log_file_path = Some(value.into());
    }
    if let Ok(value) = env::var("LATLNG_REQUIRE_AUTH") {
        config.require_auth = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_PRODUCTION_MODE") {
        config.production_mode = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_MAX_BODY_BYTES") {
        config.http_max_body_bytes = parse_usize_or(value.as_str(), config.http_max_body_bytes);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_REQUEST_TIMEOUT_MS") {
        config.http_request_timeout_ms =
            parse_u64_or(value.as_str(), config.http_request_timeout_ms);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_RATE_LIMIT_ENABLED") {
        config.http_rate_limit_enabled = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_RATE_LIMIT_REQUESTS_PER_SECOND") {
        config.http_rate_limit_requests_per_second =
            parse_u64_or(value.as_str(), config.http_rate_limit_requests_per_second);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_RATE_LIMIT_BURST") {
        config.http_rate_limit_burst = parse_u64_or(value.as_str(), config.http_rate_limit_burst);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_PRINCIPAL_RATE_LIMIT_ENABLED") {
        config.http_principal_rate_limit_enabled = parse_bool(&value);
    }
    if let Ok(value) = env::var("LATLNG_HTTP_PRINCIPAL_RATE_LIMIT_REQUESTS_PER_SECOND") {
        config.http_principal_rate_limit_requests_per_second = parse_u64_or(
            value.as_str(),
            config.http_principal_rate_limit_requests_per_second,
        );
    }
    if let Ok(value) = env::var("LATLNG_HTTP_PRINCIPAL_RATE_LIMIT_BURST") {
        config.http_principal_rate_limit_burst =
            parse_u64_or(value.as_str(), config.http_principal_rate_limit_burst);
    }

    if let Some(value) = cli_value(&args, "--listen") {
        config.listen_addr = value.to_owned();
    }
    if let Some(value) = cli_bool_or_value(&args, "--capnp-enabled") {
        config.capnp_enabled = value;
    }
    if args.iter().any(|flag| flag == "--no-capnp") {
        config.capnp_enabled = false;
    }
    if let Some(value) = cli_value(&args, "--capnp-listen") {
        config.capnp_listen_addr = value.to_owned();
    }
    if let Some(value) = cli_value(&args, "--server-id") {
        config.server_id = value.to_owned();
    }
    if let Some(value) = cli_value(&args, "--aof") {
        config.storage = RuntimeStorageMode::Aof { path: value.into() };
    }
    if args.iter().any(|flag| flag == "--memory") {
        config.storage = RuntimeStorageMode::Memory;
    }
    if let Some(value) = cli_value(&args, "--bearer-token") {
        config.auth.bearer_token = Some(value.to_owned());
    }
    if args.iter().any(|flag| flag == "--disable-bearer-token") {
        config.auth.disable_bearer_token = true;
    }
    if let Some(value) = cli_value(&args, "--jwt-secret") {
        config.auth.jwt_secret = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwt-public-key-pem") {
        config.auth.jwt_public_key_pem = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwt-issuer") {
        config.auth.jwt_issuer = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwt-audience") {
        config.auth.jwt_audience = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwt-algorithm") {
        config.auth.jwt_algorithm = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwt-leeway") {
        config.auth.jwt_leeway_seconds = value.parse().unwrap_or(0);
    }
    if let Some(value) = cli_value(&args, "--jwks-url") {
        config.auth.jwks_url = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwks-provider-id") {
        config.auth.jwks_provider_id = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--jwks-refresh-interval-seconds") {
        config.auth.jwks_refresh_interval_seconds =
            parse_u64_or(value, config.auth.jwks_refresh_interval_seconds);
    }
    if let Some(value) = cli_value(&args, "--jwks-cache-ttl-seconds") {
        config.auth.jwks_cache_ttl_seconds =
            parse_u64_or(value, config.auth.jwks_cache_ttl_seconds);
    }
    if let Some(value) = cli_value(&args, "--jwks-http-timeout-ms") {
        config.auth.jwks_http_timeout_ms = parse_u64_or(value, config.auth.jwks_http_timeout_ms);
    }
    if let Some(value) = cli_value(&args, "--read-only") {
        config.read_only = parse_bool(value);
    }
    if let Some(value) = cli_value(&args, "--subscriber-queue-capacity") {
        config.subscriber_queue_capacity = parse_usize_or(value, config.subscriber_queue_capacity);
    }
    if let Some(value) = cli_value(&args, "--webhook-queue-path") {
        config.webhook_queue_path = Some(value.into());
    }
    if let Some(value) = cli_value(&args, "--webhook-timeout-ms") {
        config.webhook_timeout_ms = parse_u64_or(value, config.webhook_timeout_ms);
    }
    if let Some(value) = cli_value(&args, "--webhook-concurrency-limit") {
        config.webhook_concurrency_limit = parse_usize_or(value, config.webhook_concurrency_limit);
    }
    if let Some(value) = cli_value(&args, "--webhook-retry-count") {
        config.webhook_retry_count = parse_u32_or(value, config.webhook_retry_count);
    }
    if let Some(value) = cli_value(&args, "--webhook-retry-initial-backoff-ms") {
        config.webhook_retry_initial_backoff_ms =
            parse_u64_or(value, config.webhook_retry_initial_backoff_ms);
    }
    if let Some(value) = cli_value(&args, "--webhook-retry-max-backoff-ms") {
        config.webhook_retry_max_backoff_ms =
            parse_u64_or(value, config.webhook_retry_max_backoff_ms);
    }
    if let Some(value) = cli_value(&args, "--webhook-lease-ms") {
        config.webhook_lease_ms = parse_u64_or(value, config.webhook_lease_ms);
    }
    if let Some(value) = cli_value(&args, "--native-executor-threads") {
        config.native_executor_threads = parse_usize_or(value, config.native_executor_threads);
    }
    if let Some(value) = cli_value(&args, "--native-executor-queue-limit") {
        config.native_executor_queue_limit =
            parse_usize_or(value, config.native_executor_queue_limit);
    }
    if let Some(value) = cli_value(&args, "--aof-writer-queue-limit") {
        config.aof_writer_queue_limit = parse_usize_or(value, config.aof_writer_queue_limit);
    }
    if let Some(value) = cli_value(&args, "--aof-group-commit-delay-ms") {
        config.aof_group_commit_delay_ms = parse_u64_or(value, config.aof_group_commit_delay_ms);
    }
    if let Some(value) = cli_value(&args, "--aof-group-commit-max-requests") {
        config.aof_group_commit_max_requests =
            parse_usize_or(value, config.aof_group_commit_max_requests);
    }
    if let Some(value) = cli_value(&args, "--follow-host") {
        config.follow_host = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--follow-port") {
        config.follow_port = value.parse::<u16>().ok().filter(|port| *port > 0);
    }
    if let Some(value) = cli_value(&args, "--replication-credential") {
        config.replication_credential = Some(value.to_owned());
    }
    if let Some(value) = cli_value(&args, "--replication-batch-size") {
        config.replication_batch_size = parse_usize_or(value, config.replication_batch_size);
    }
    if let Some(value) = cli_value(&args, "--replication-reconnect-backoff-ms") {
        config.replication_reconnect_backoff_ms =
            parse_u64_or(value, config.replication_reconnect_backoff_ms);
    }
    if let Some(value) = cli_bool_or_value(&args, "--http-cors-enabled") {
        config.http_cors_enabled = value;
    }
    if let Some(value) = cli_value(&args, "--http-cors-allowed-origins") {
        config.http_cors_allowed_origins = parse_csv(value);
    }
    if let Some(value) = cli_value(&args, "--http-cors-allowed-methods") {
        config.http_cors_allowed_methods = parse_csv(value);
    }
    if let Some(value) = cli_value(&args, "--http-cors-allowed-headers") {
        config.http_cors_allowed_headers = parse_csv(value);
    }
    if let Some(value) = cli_value(&args, "--http-cors-max-age-seconds") {
        config.http_cors_max_age_seconds = value.trim().parse::<u64>().ok();
    }
    if let Some(value) = cli_bool_or_value(&args, "--logging-enabled") {
        config.logging_enabled = value;
    }
    if args.iter().any(|flag| flag == "--no-logging") {
        config.logging_enabled = false;
    }
    if let Some(value) = cli_value(&args, "--log-format") {
        config.log_format =
            parse_log_format(value).ok_or_else(|| format!("unsupported log format: {value}"))?;
    }
    if let Some(value) = cli_value(&args, "--log-level") {
        config.log_level = value.to_owned();
    }
    if let Some(value) = cli_value(&args, "--log-destination") {
        config.log_destination = parse_log_destination(value)
            .ok_or_else(|| format!("unsupported log destination: {value}"))?;
    }
    if let Some(value) = cli_value(&args, "--log-file") {
        config.log_file_path = Some(value.into());
    }
    if args.iter().any(|flag| flag == "--require-auth") {
        config.require_auth = true;
    }
    if let Some(value) = cli_bool_or_value(&args, "--production-mode") {
        config.production_mode = value;
    }
    if let Some(value) = cli_value(&args, "--http-max-body-bytes") {
        config.http_max_body_bytes = parse_usize_or(value, config.http_max_body_bytes);
    }
    if let Some(value) = cli_value(&args, "--http-request-timeout-ms") {
        config.http_request_timeout_ms = parse_u64_or(value, config.http_request_timeout_ms);
    }
    if let Some(value) = cli_bool_or_value(&args, "--http-rate-limit-enabled") {
        config.http_rate_limit_enabled = value;
    }
    if let Some(value) = cli_value(&args, "--http-rate-limit-requests-per-second") {
        config.http_rate_limit_requests_per_second =
            parse_u64_or(value, config.http_rate_limit_requests_per_second);
    }
    if let Some(value) = cli_value(&args, "--http-rate-limit-burst") {
        config.http_rate_limit_burst = parse_u64_or(value, config.http_rate_limit_burst);
    }
    if let Some(value) = cli_bool_or_value(&args, "--http-principal-rate-limit-enabled") {
        config.http_principal_rate_limit_enabled = value;
    }
    if let Some(value) = cli_value(&args, "--http-principal-rate-limit-requests-per-second") {
        config.http_principal_rate_limit_requests_per_second =
            parse_u64_or(value, config.http_principal_rate_limit_requests_per_second);
    }
    if let Some(value) = cli_value(&args, "--http-principal-rate-limit-burst") {
        config.http_principal_rate_limit_burst =
            parse_u64_or(value, config.http_principal_rate_limit_burst);
    }

    Ok(config)
}

fn print_config_check(config: &RuntimeConfig) -> Result<(), Box<dyn std::error::Error>> {
    let auth = if config.auth.jwt_secret.is_some() {
        "jwt_hmac"
    } else if config.auth.jwt_public_key_pem.is_some() {
        "jwt_public_key"
    } else if config.auth.jwks_url.is_some() {
        "jwks"
    } else if config.auth.bearer_enabled() {
        "static_bearer"
    } else {
        "disabled"
    };
    let storage = match &config.storage {
        RuntimeStorageMode::Memory => "memory".to_owned(),
        RuntimeStorageMode::Aof { path } => format!("aof:{}", path.display()),
    };
    let report = serde_json::json!({
        "ok": true,
        "listen_addr": config.listen_addr,
        "capnp_enabled": config.capnp_enabled,
        "capnp_listen_addr": config.capnp_listen_addr,
        "storage": storage,
        "production_mode": config.production_mode,
        "require_auth": config.require_auth,
        "auth_mode": auth,
        "production_guardrail_warnings": config.production_guardrail_warnings(),
        "cors_enabled": config.http_cors_enabled,
        "http_max_body_bytes": config.http_max_body_bytes,
        "http_request_timeout_ms": config.http_request_timeout_ms,
        "http_rate_limit_enabled": config.http_rate_limit_enabled,
        "http_rate_limit_requests_per_second": config.http_rate_limit_requests_per_second,
        "http_rate_limit_burst": config.http_rate_limit_burst,
        "http_principal_rate_limit_enabled": config.http_principal_rate_limit_enabled,
        "http_principal_rate_limit_requests_per_second": config.http_principal_rate_limit_requests_per_second,
        "http_principal_rate_limit_burst": config.http_principal_rate_limit_burst,
        "logging_enabled": config.logging_enabled,
        "log_destination": format!("{:?}", config.log_destination).to_ascii_lowercase(),
        "following": config.follow_host.as_ref().zip(config.follow_port).map(|(host, port)| format!("{host}:{port}")),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn cli_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].as_str())
}

fn cli_bool_or_value(args: &[String], flag: &str) -> Option<bool> {
    let index = args.iter().position(|value| value == flag)?;
    match args.get(index + 1) {
        Some(value) if !value.starts_with("--") => Some(parse_bool(value)),
        _ => Some(true),
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_log_format(value: &str) -> Option<LogFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "compact" | "text" => Some(LogFormat::Compact),
        "json" => Some(LogFormat::Json),
        _ => None,
    }
}

fn parse_log_destination(value: &str) -> Option<LogDestination> {
    match value.trim().to_ascii_lowercase().as_str() {
        "stderr" => Some(LogDestination::Stderr),
        "stdout" => Some(LogDestination::Stdout),
        "file" => Some(LogDestination::File),
        "none" | "off" | "disabled" => Some(LogDestination::None),
        _ => None,
    }
}

fn parse_usize_or(value: &str, fallback: usize) -> usize {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .unwrap_or(fallback)
}

fn parse_u64_or(value: &str, fallback: u64) -> u64 {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .unwrap_or(fallback)
}

fn parse_u32_or(value: &str, fallback: u32) -> u32 {
    value.trim().parse::<u32>().ok().unwrap_or(fallback)
}

fn webhook_timeout(runtime_config: &SharedRuntimeConfig) -> Duration {
    let millis = match runtime_config.read() {
        Ok(guard) => guard.webhook_timeout_ms,
        Err(poisoned) => poisoned.into_inner().webhook_timeout_ms,
    };
    Duration::from_millis(millis.max(1))
}

fn webhook_concurrency_limit(runtime_config: &SharedRuntimeConfig) -> usize {
    match runtime_config.read() {
        Ok(guard) => guard.webhook_concurrency_limit.max(1),
        Err(poisoned) => poisoned.into_inner().webhook_concurrency_limit.max(1),
    }
}

fn webhook_retry_initial_backoff_ms(runtime_config: &SharedRuntimeConfig) -> u64 {
    match runtime_config.read() {
        Ok(guard) => guard.webhook_retry_initial_backoff_ms.max(1),
        Err(poisoned) => poisoned
            .into_inner()
            .webhook_retry_initial_backoff_ms
            .max(1),
    }
}

fn webhook_retry_max_backoff_ms(runtime_config: &SharedRuntimeConfig) -> u64 {
    match runtime_config.read() {
        Ok(guard) => guard
            .webhook_retry_max_backoff_ms
            .max(guard.webhook_retry_initial_backoff_ms.max(1)),
        Err(poisoned) => {
            let guard = poisoned.into_inner();
            guard
                .webhook_retry_max_backoff_ms
                .max(guard.webhook_retry_initial_backoff_ms.max(1))
        }
    }
}

fn webhook_lease_ms(runtime_config: &SharedRuntimeConfig) -> u64 {
    match runtime_config.read() {
        Ok(guard) => guard.webhook_lease_ms.max(1),
        Err(poisoned) => poisoned.into_inner().webhook_lease_ms.max(1),
    }
}

fn resolve_webhook_queue_path(config: &RuntimeConfig) -> PathBuf {
    if let Some(path) = &config.webhook_queue_path {
        return path.clone();
    }
    match &config.storage {
        RuntimeStorageMode::Aof { path } => path.with_extension("webhooks.sqlite"),
        RuntimeStorageMode::Memory => PathBuf::from("./data/webhook-queue.sqlite"),
    }
}

async fn rebuild_webhook_queue_from_log(
    db: &Arc<LatLngNative<ServerStorage>>,
    queue: &Arc<WebhookQueue>,
) -> Result<u64, Box<dyn std::error::Error>> {
    queue.reset()?;
    let queue = Arc::clone(queue);
    let db = Arc::clone(db);
    let last_sequence = tokio::task::spawn_blocking(move || {
        let mut last_sequence = 0_u64;
        db.replay_log(0, &mut |sequence, record| {
            queue
                .apply_log_record(sequence, &record)
                .map_err(|error| latlng_core::CoreError::Message(error.to_string()))?;
            last_sequence = sequence;
            Ok(())
        })?;
        Ok::<u64, latlng_core::CoreError>(last_sequence)
    })
    .await
    .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?
    .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?;
    Ok(last_sequence)
}

async fn apply_log_tail(
    db: &Arc<LatLngNative<ServerStorage>>,
    queue: &Arc<WebhookQueue>,
    last_applied_sequence: &mut u64,
) -> Result<(), latlng_core::CoreError> {
    let after_seq = *last_applied_sequence;
    let queue = Arc::clone(queue);
    let db = Arc::clone(db);
    let last_seen = tokio::task::spawn_blocking(move || {
        let mut last_seen = after_seq;
        db.replay_log(after_seq, &mut |sequence, record| {
            queue
                .apply_log_record(sequence, &record)
                .map_err(|error| latlng_core::CoreError::Message(error.to_string()))?;
            last_seen = sequence;
            Ok(())
        })?;
        Ok::<u64, latlng_core::CoreError>(last_seen)
    })
    .await
    .map_err(|error| latlng_core::CoreError::Message(error.to_string()))??;
    *last_applied_sequence = last_seen;
    Ok(())
}

async fn drain_completed_hook_tasks(
    in_flight: &mut JoinSet<WebhookAttemptResult>,
    db: &Arc<LatLngNative<ServerStorage>>,
    queue: &Arc<WebhookQueue>,
    metrics: &RequestMetrics,
    runtime_config: &SharedRuntimeConfig,
    last_applied_sequence: &mut u64,
    outbox_control: &OutboxControl,
) {
    while let Some(result) = in_flight.try_join_next() {
        handle_join_result(
            result,
            db,
            queue,
            metrics,
            runtime_config,
            last_applied_sequence,
            outbox_control,
        )
        .await;
    }
}

async fn handle_join_result(
    result: Result<WebhookAttemptResult, tokio::task::JoinError>,
    db: &Arc<LatLngNative<ServerStorage>>,
    queue: &Arc<WebhookQueue>,
    metrics: &RequestMetrics,
    runtime_config: &SharedRuntimeConfig,
    last_applied_sequence: &mut u64,
    outbox_control: &OutboxControl,
) {
    match result {
        Ok(result) => {
            if let Err(error) = finalize_webhook_attempt(
                db,
                queue,
                metrics,
                runtime_config,
                last_applied_sequence,
                result,
                outbox_control,
            )
            .await
            {
                error!(error = %error, "failed to finalize webhook delivery attempt");
            }
            refresh_queue_metrics(queue, metrics);
        }
        Err(error) => {
            error!(error = %error, "hook delivery task failed");
        }
    }
}

async fn finalize_webhook_attempt(
    db: &Arc<LatLngNative<ServerStorage>>,
    queue: &Arc<WebhookQueue>,
    metrics: &RequestMetrics,
    runtime_config: &SharedRuntimeConfig,
    _last_applied_sequence: &mut u64,
    result: WebhookAttemptResult,
    outbox_control: &OutboxControl,
) -> Result<(), Box<dyn std::error::Error>> {
    let _guard = outbox_control.gate.read().await;
    if result.epoch != outbox_control.current_epoch() {
        return Ok(());
    }
    let record = match result.delivery {
        WebhookDeliveryOutcome::Skipped => return Ok(()),
        WebhookDeliveryOutcome::Attempted(Ok(())) => {
            metrics.record_hook_success();
            LogRecord::WebhookAck(WebhookAckRecord {
                job_id: result.job.job_id.clone(),
            })
        }
        WebhookDeliveryOutcome::Attempted(Err(error)) => {
            metrics.record_hook_failure();
            let attempts_used = result.job.attempts_used.saturating_add(1);
            if attempts_used >= result.job.max_attempts {
                metrics.record_hook_dead_letter();
                LogRecord::WebhookDeadLetter(WebhookDeadLetterRecord {
                    job_id: result.job.job_id.clone(),
                    attempts_used,
                    last_error: error.to_string(),
                })
            } else {
                metrics.record_hook_retry();
                let backoff_ms = exponential_backoff_ms(
                    webhook_retry_initial_backoff_ms(runtime_config),
                    webhook_retry_max_backoff_ms(runtime_config),
                    attempts_used,
                );
                LogRecord::WebhookRetryScheduled(WebhookRetryScheduledRecord {
                    job_id: result.job.job_id.clone(),
                    attempts_used,
                    next_attempt_at_ms: now_ms().saturating_add(backoff_ms),
                    last_error: error.to_string(),
                })
            }
        }
    };

    let db = Arc::clone(db);
    let persisted_record = record.clone();
    let sequence = tokio::task::spawn_blocking(move || db.append_log_record(persisted_record))
        .await
        .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?
        .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?;
    queue.apply_log_record(sequence, &record)?;
    Ok(())
}

fn refresh_queue_metrics(queue: &Arc<WebhookQueue>, metrics: &RequestMetrics) {
    if let Ok(stats) = queue.stats(now_ms()) {
        metrics.update_webhook_queue_stats(
            stats.pending,
            stats.leased,
            stats.dead_letter,
            stats.oldest_pending_age_ms,
        );
    }
}

fn exponential_backoff_ms(initial_ms: u64, max_ms: u64, attempts_used: u32) -> u64 {
    let mut backoff = initial_ms.max(1);
    let exponent = attempts_used.saturating_sub(1);
    for _ in 0..exponent {
        backoff = backoff.saturating_mul(2);
        if backoff >= max_ms {
            return max_ms.max(initial_ms);
        }
    }
    backoff.min(max_ms.max(initial_ms))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Default)]
struct OutboxControl {
    epoch: AtomicU64,
    gate: RwLock<()>,
    notify: Arc<Notify>,
}

impl OutboxControl {
    fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    fn notify_work(&self) {
        self.notify.notify_waiters();
    }
}

struct FlushDbCoordinatorImpl {
    db: Arc<LatLngNative<ServerStorage>>,
    queue: Arc<WebhookQueue>,
    metrics: Arc<RequestMetrics>,
    outbox_control: Arc<OutboxControl>,
    replication_notify: Arc<Notify>,
}

impl FlushDbCoordinator for FlushDbCoordinatorImpl {
    fn flushdb(&self) -> FlushDbFuture<'_> {
        Box::pin(async move {
            let _guard = self.outbox_control.gate.write().await;
            let db = Arc::clone(&self.db);
            tokio::task::spawn_blocking(move || db.flushdb())
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            self.queue.reset().map_err(|error| error.to_string())?;
            self.outbox_control.epoch.fetch_add(1, Ordering::SeqCst);
            self.metrics.update_webhook_queue_stats(0, 0, 0, None);
            self.outbox_control.notify_work();
            self.replication_notify.notify_waiters();
            Ok(())
        })
    }
}

struct WebhookAttemptResult {
    job: QueueJob,
    epoch: u64,
    delivery: WebhookDeliveryOutcome,
}

enum WebhookDeliveryOutcome {
    Attempted(Result<(), latlng_endpoints::EndpointError>),
    Skipped,
}
