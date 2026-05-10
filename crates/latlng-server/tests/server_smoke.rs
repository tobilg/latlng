#![forbid(unsafe_code)]

use std::net::TcpListener as StdTcpListener;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use capnp::message::ReaderOptions;
use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use futures_util::{SinkExt, StreamExt};
use latlng_capnp::schema::latlng_capnp::lat_lng;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use tempfile::tempdir;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::task::LocalSet;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[test]
fn server_version_exits_without_starting_server() {
    let output = StdCommand::new(env!("CARGO_BIN_EXE_latlng-server"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("latlng-server {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        output.stderr.is_empty(),
        "stderr should be empty, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn server_supports_http_ws_capnp_and_config_rewrite() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let config_path = dir.path().join("latlng.json");

            write_config(&config_path, http_port, capnp_port, &aof_path);

            let mut child = spawn_server(&config_path).await;
            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");
            wait_for_ping(&client, &base).await;

            let capnp = connect_capnp(([127, 0, 0, 1], capnp_port).into()).await;
            let mut auth = capnp.auth_request();
            auth.get().set_token("secret");
            let auth = auth.send().promise.await.unwrap();
            assert!(auth.get().unwrap().get_resp().unwrap().get_ok());

            let ping = capnp.ping_request().send().promise.await.unwrap();
            assert!(ping.get().unwrap().get_resp().unwrap().get_ok());

            let (mut ws, _) =
                tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{http_port}/ws"))
                    .await
                    .unwrap();
            ws.send(Message::Text(
                serde_json::json!({ "type": "auth", "token": "secret" })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            read_ws_message(&mut ws).await;

            set_channel(&client, &base).await;

            ws.send(Message::Text(
                serde_json::json!({ "type": "subscribe", "channels": ["fleet-events"] })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            read_ws_message(&mut ws).await;

            let mut subscribe = capnp.subscribe_request();
            subscribe.get().init_channels(1).set(0, "fleet-events");
            let stream = subscribe.send().promise.await.unwrap();
            let stream = stream.get().unwrap().get_stream().unwrap();

            set_point(&client, &base).await;

            let ws_event = wait_for_ws_event(&mut ws).await;
            let capnp_event = timeout(Duration::from_secs(3), stream.next_request().send().promise)
                .await
                .unwrap()
                .unwrap();
            let capnp_event = capnp_event.get().unwrap();
            assert!(!capnp_event.get_done());
            let capnp_event = capnp_event.get_event().unwrap();

            assert_eq!(ws_event["collection"], "fleet");
            assert_eq!(ws_event["id"], "truck-1");
            assert_eq!(ws_event["detect"], "Enter");
            assert_eq!(ws_event["command"], "Set");
            assert_eq!(capnp_event.get_collection().unwrap(), "fleet");
            assert_eq!(capnp_event.get_id().unwrap(), "truck-1");
            assert_eq!(capnp_event.get_command().unwrap(), "set");
            assert_eq!(
                format!("{:?}", capnp_event.get_detect().unwrap()).to_ascii_lowercase(),
                "enter"
            );

            let point = ws_event["object"]["Point"].clone();
            assert_eq!(point["lat"], 52.52);
            assert_eq!(point["lon"], 13.405);
            match capnp_event.get_object().unwrap().which().unwrap() {
                latlng_capnp::schema::latlng_capnp::geo_object::WhichReader::Point(reader) => {
                    let reader = reader.unwrap();
                    assert_eq!(reader.get_lat(), 52.52);
                    assert_eq!(reader.get_lon(), 13.405);
                }
                _ => panic!("unexpected capnp object variant"),
            }

            let timeout_response = client
                .post(format!("{base}/admin/timeout"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(CONTENT_TYPE, "application/json")
                .body(serde_json::json!({ "command": "set", "seconds": 1.5 }).to_string())
                .send()
                .await
                .unwrap();
            assert!(timeout_response.status().is_success());

            let readonly_response = client
                .post(format!("{base}/admin/readonly"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(CONTENT_TYPE, "application/json")
                .body(serde_json::json!({ "enabled": true }).to_string())
                .send()
                .await
                .unwrap();
            assert!(readonly_response.status().is_success());

            let rewrite_response = client
                .post(format!("{base}/admin/config/rewrite"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            assert!(rewrite_response.status().is_success());

            let rewritten = serde_json::from_str::<serde_json::Value>(
                &tokio::fs::read_to_string(&config_path).await.unwrap(),
            )
            .unwrap();
            assert_eq!(rewritten["read_only"], true);
            assert_eq!(rewritten["command_timeouts"]["set"], 1.5);

            child.kill().await.unwrap();
            let _ = child.wait().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn server_can_disable_capnp_listener() {
    let dir = tempdir().unwrap();
    let http_port = free_port();
    let capnp_port = free_port();
    let aof_path = dir.path().join("appendonly.aof");
    let config_path = dir.path().join("latlng.json");

    write_config(&config_path, http_port, capnp_port, &aof_path);
    let mut raw = serde_json::from_str::<serde_json::Value>(
        &tokio::fs::read_to_string(&config_path).await.unwrap(),
    )
    .unwrap();
    raw["capnp_enabled"] = serde_json::Value::Bool(false);
    tokio::fs::write(&config_path, serde_json::to_string_pretty(&raw).unwrap())
        .await
        .unwrap();

    let mut child = spawn_server(&config_path).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    wait_for_ping(&client, &base).await;

    let capnp_connect = TcpStream::connect(("127.0.0.1", capnp_port)).await;
    assert!(
        capnp_connect.is_err(),
        "Cap'n Proto listener should not bind when disabled"
    );

    child.kill().await.unwrap();
    let _ = child.wait().await;
}

#[tokio::test(flavor = "current_thread")]
async fn server_limits_webhook_delivery_concurrency() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let hook_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let config_path = dir.path().join("latlng.json");

            write_config_with_hook_settings(
                &config_path,
                http_port,
                capnp_port,
                &aof_path,
                5_000,
                2,
            );

            let hook_state = HookState::default();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", hook_port))
                .await
                .unwrap();
            let app = Router::new()
                .route("/hook", post(slow_hook))
                .with_state(hook_state.clone());
            let hook_server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            let mut child = spawn_server(&config_path).await;
            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");
            wait_for_ping(&client, &base).await;

            set_hook(
                &client,
                &base,
                &format!("http://127.0.0.1:{hook_port}/hook"),
            )
            .await;

            for index in 0..6 {
                set_point_for_id(&client, &base, &format!("truck-{index}"), 52.52, 13.405).await;
            }

            wait_for_hook_deliveries(&hook_state, 6).await;

            assert_eq!(hook_state.delivered.load(Ordering::Relaxed), 6);
            assert_eq!(hook_state.max_active.load(Ordering::Relaxed), 2);

            child.kill().await.unwrap();
            let _ = child.wait().await;
            hook_server.abort();
            let _ = hook_server.await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn server_delivers_geojson_webhook_without_log_tail_decode_errors() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let hook_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let config_path = dir.path().join("latlng.json");

            write_config(&config_path, http_port, capnp_port, &aof_path);

            let hook_state = HookState::default();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", hook_port))
                .await
                .unwrap();
            let app = Router::new()
                .route("/hook", post(fast_hook))
                .with_state(hook_state.clone());
            let hook_server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            let mut child = spawn_server(&config_path).await;
            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");
            wait_for_ping(&client, &base).await;

            set_geojson_hook(
                &client,
                &base,
                &format!("http://127.0.0.1:{hook_port}/hook"),
            )
            .await;
            set_point_for_id(&client, &base, "truck-1", 52.50, 13.37).await;
            set_point_for_id(&client, &base, "truck-1", 52.52, 13.405).await;

            wait_for_hook_deliveries(&hook_state, 1).await;

            child.kill().await.unwrap();
            let _ = child.wait().await;
            hook_server.abort();
            let _ = hook_server.await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn require_auth_refuses_open_server_startup() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("latlng.json");
    std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "listen_addr": format!("127.0.0.1:{}", free_port()),
            "capnp_listen_addr": format!("127.0.0.1:{}", free_port()),
            "require_auth": true
        }))
        .unwrap(),
    )
    .unwrap();

    let mut child = spawn_server(&config_path).await;
    let status = timeout(Duration::from_secs(3), child.wait())
        .await
        .expect("server should exit when require_auth is unsatisfied")
        .unwrap();
    assert!(!status.success());
}

#[tokio::test(flavor = "current_thread")]
async fn server_persists_hooks_and_channels_across_restart() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let queue_path = dir.path().join("webhooks.sqlite");
            let config_path = dir.path().join("latlng.json");

            write_config_with_webhook_settings(
                &config_path,
                http_port,
                capnp_port,
                &aof_path,
                Some(&queue_path),
                5_000,
                8,
                8,
                200,
                30_000,
            );

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            set_channel(&client, &base).await;
            set_hook(&client, &base, "http://127.0.0.1:1/hook").await;
            first.kill().await.unwrap();
            let _ = first.wait().await;

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;

            let channels = client
                .get(format!("{base}/channels"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let channels = response_json(channels).await;
            assert_eq!(channels["channels"][0], "fleet-events");

            let hooks = client
                .get(format!("{base}/hooks"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let hooks = response_json(hooks).await;
            assert_eq!(hooks[0]["name"], "fleet-hook");
            assert_eq!(hooks[0]["endpoint"], "http://127.0.0.1:1/hook");

            second.kill().await.unwrap();
            let _ = second.wait().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pending_webhook_jobs_survive_restart() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let hook_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let queue_path = dir.path().join("webhooks.sqlite");
            let config_path = dir.path().join("latlng.json");

            write_config_with_webhook_settings(
                &config_path,
                http_port,
                capnp_port,
                &aof_path,
                Some(&queue_path),
                100,
                8,
                8,
                50,
                200,
            );

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");
            let endpoint = format!("http://127.0.0.1:{hook_port}/hook");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            set_hook(&client, &base, &endpoint).await;
            set_point(&client, &base).await;
            wait_for_metric_at_least(&client, &base, "hook_failure_total", 1).await;
            first.kill().await.unwrap();
            let _ = first.wait().await;

            let hook_state = HookState::default();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", hook_port))
                .await
                .unwrap();
            let app = Router::new()
                .route("/hook", post(fast_hook))
                .with_state(hook_state.clone());
            let hook_server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            wait_for_hook_deliveries(&hook_state, 1).await;

            second.kill().await.unwrap();
            let _ = second.wait().await;
            hook_server.abort();
            let _ = hook_server.await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn pending_webhook_jobs_survive_sigterm_restart() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let hook_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let queue_path = dir.path().join("webhooks.sqlite");
            let config_path = dir.path().join("latlng.json");

            write_config_with_webhook_settings(
                &config_path,
                http_port,
                capnp_port,
                &aof_path,
                Some(&queue_path),
                100,
                8,
                8,
                50,
                200,
            );

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");
            let endpoint = format!("http://127.0.0.1:{hook_port}/hook");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            set_hook(&client, &base, &endpoint).await;
            set_point(&client, &base).await;
            wait_for_metric_at_least(&client, &base, "hook_failure_total", 1).await;
            send_sigterm(&mut first).await;

            let hook_state = HookState::default();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", hook_port))
                .await
                .unwrap();
            let app = Router::new()
                .route("/hook", post(fast_hook))
                .with_state(hook_state.clone());
            let hook_server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            wait_for_hook_deliveries(&hook_state, 1).await;

            send_sigterm(&mut second).await;
            hook_server.abort();
            let _ = hook_server.await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn flushdb_clears_geofences_and_pending_webhook_jobs() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let queue_path = dir.path().join("webhooks.sqlite");
            let config_path = dir.path().join("latlng.json");

            write_config_with_webhook_settings(
                &config_path,
                http_port,
                capnp_port,
                &aof_path,
                Some(&queue_path),
                100,
                8,
                8,
                200,
                30_000,
            );

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            set_channel(&client, &base).await;
            set_hook(&client, &base, "http://127.0.0.1:1/hook").await;
            set_point(&client, &base).await;
            wait_for_metric_at_least(&client, &base, "hook_failure_total", 1).await;

            let queue_before_flush = client
                .get(format!("{base}/admin/webhook-queue"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let queue_before_flush = response_json(queue_before_flush).await;
            let queued_before_flush = queue_before_flush["pending"].as_u64().unwrap_or_default()
                + queue_before_flush["leased"].as_u64().unwrap_or_default()
                + queue_before_flush["dead_letter"]
                    .as_u64()
                    .unwrap_or_default();
            assert!(queued_before_flush >= 1);

            let metrics_before_flush = client
                .get(format!("{base}/metrics"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let metrics_before_flush = response_text(metrics_before_flush).await;
            let hook_attempts_before_flush =
                metric_value(&metrics_before_flush, "latlng_hook_attempts_total")
                    .unwrap_or_default();

            let flush = client
                .post(format!("{base}/admin/flushdb"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            assert!(flush.status().is_success());

            let channels = client
                .get(format!("{base}/channels"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let channels = response_json(channels).await;
            assert_eq!(channels["channels"], serde_json::json!([]));

            let hooks = client
                .get(format!("{base}/hooks"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let hooks = response_json(hooks).await;
            assert_eq!(hooks, serde_json::json!([]));

            let queue_after_flush = client
                .get(format!("{base}/admin/webhook-queue"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let queue_after_flush = response_json(queue_after_flush).await;
            assert_eq!(queue_after_flush["pending"], 0);
            assert_eq!(queue_after_flush["leased"], 0);
            assert_eq!(queue_after_flush["dead_letter"], 0);

            sleep(Duration::from_millis(350)).await;

            let metrics_after_flush = client
                .get(format!("{base}/metrics"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let metrics_after_flush = response_text(metrics_after_flush).await;
            assert_eq!(
                metric_value(&metrics_after_flush, "latlng_hook_attempts_total")
                    .unwrap_or_default(),
                hook_attempts_before_flush
            );

            first.kill().await.unwrap();
            let _ = first.wait().await;

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;

            let channels = client
                .get(format!("{base}/channels"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let channels = response_json(channels).await;
            assert_eq!(channels["channels"], serde_json::json!([]));

            let hooks = client
                .get(format!("{base}/hooks"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let hooks = response_json(hooks).await;
            assert_eq!(hooks, serde_json::json!([]));

            let queue_after_restart = client
                .get(format!("{base}/admin/webhook-queue"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            let queue_after_restart = response_json(queue_after_restart).await;
            assert_eq!(queue_after_restart["pending"], 0);
            assert_eq!(queue_after_restart["leased"], 0);
            assert_eq!(queue_after_restart["dead_letter"], 0);

            second.kill().await.unwrap();
            let _ = second.wait().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn aofshrink_preserves_pending_webhook_jobs() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let hook_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let queue_path = dir.path().join("webhooks.sqlite");
            let config_path = dir.path().join("latlng.json");

            write_config_with_webhook_settings(
                &config_path,
                http_port,
                capnp_port,
                &aof_path,
                Some(&queue_path),
                100,
                8,
                8,
                50,
                200,
            );

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");
            let endpoint = format!("http://127.0.0.1:{hook_port}/hook");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            set_hook(&client, &base, &endpoint).await;
            set_point(&client, &base).await;
            wait_for_metric_at_least(&client, &base, "hook_failure_total", 1).await;

            let shrink = client
                .post(format!("{base}/admin/aofshrink"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            assert!(shrink.status().is_success());

            first.kill().await.unwrap();
            let _ = first.wait().await;

            let hook_state = HookState::default();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", hook_port))
                .await
                .unwrap();
            let app = Router::new()
                .route("/hook", post(fast_hook))
                .with_state(hook_state.clone());
            let hook_server = tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            wait_for_hook_deliveries(&hook_state, 1).await;

            second.kill().await.unwrap();
            let _ = second.wait().await;
            hook_server.abort();
            let _ = hook_server.await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn follower_catches_up_and_rejects_writes() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let leader_http = free_port();
            let leader_capnp = free_port();
            let follower_http = free_port();
            let follower_capnp = free_port();
            let leader_config = dir.path().join("leader.json");
            let follower_config = dir.path().join("follower.json");
            let leader_aof = dir.path().join("leader.aof");
            let follower_aof = dir.path().join("follower.aof");

            write_replication_config(
                &leader_config,
                leader_http,
                leader_capnp,
                &leader_aof,
                "leader-1",
                "repl-secret",
                None,
            );
            write_replication_config(
                &follower_config,
                follower_http,
                follower_capnp,
                &follower_aof,
                "follower-1",
                "repl-secret",
                Some(("127.0.0.1", leader_capnp)),
            );

            let client = reqwest::Client::new();
            let leader_base = format!("http://127.0.0.1:{leader_http}");
            let follower_base = format!("http://127.0.0.1:{follower_http}");

            let mut leader = spawn_server(&leader_config).await;
            let mut follower = spawn_server(&follower_config).await;
            wait_for_ping(&client, &leader_base).await;
            wait_for_ping(&client, &follower_base).await;

            set_point(&client, &leader_base).await;
            wait_for_follower_caught_up(&client, &follower_base).await;

            let object = wait_for_object_present(&client, &follower_base, "fleet", "truck-1").await;
            assert_eq!(object["id"], "truck-1");

            let server_info = response_json(
                client
                    .get(format!("{follower_base}/server"))
                    .header(AUTHORIZATION, "Bearer secret")
                    .send()
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(server_info["server_id"], "follower-1");
            assert_eq!(
                server_info["following"],
                format!("127.0.0.1:{leader_capnp}")
            );
            assert_eq!(server_info["leader"], false);
            assert_eq!(server_info["caught_up_once"], true);

            let write = client
                .post(format!("{follower_base}/collections/fleet/objects/truck-2"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(CONTENT_TYPE, "application/json")
                .body(
                    serde_json::json!({
                        "object": { "Point": { "lat": 48.13, "lon": 11.58, "z": null } }
                    })
                    .to_string(),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(write.status(), StatusCode::BAD_REQUEST);

            leader.kill().await.unwrap();
            let _ = leader.wait().await;
            follower.kill().await.unwrap();
            let _ = follower.wait().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn follower_resumes_replication_after_restart() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let leader_http = free_port();
            let leader_capnp = free_port();
            let follower_http = free_port();
            let follower_capnp = free_port();
            let leader_config = dir.path().join("leader.json");
            let follower_config = dir.path().join("follower.json");
            let leader_aof = dir.path().join("leader.aof");
            let follower_aof = dir.path().join("follower.aof");

            write_replication_config(
                &leader_config,
                leader_http,
                leader_capnp,
                &leader_aof,
                "leader-2",
                "repl-secret",
                None,
            );
            write_replication_config(
                &follower_config,
                follower_http,
                follower_capnp,
                &follower_aof,
                "follower-2",
                "repl-secret",
                Some(("127.0.0.1", leader_capnp)),
            );

            let client = reqwest::Client::new();
            let leader_base = format!("http://127.0.0.1:{leader_http}");
            let follower_base = format!("http://127.0.0.1:{follower_http}");

            let mut leader = spawn_server(&leader_config).await;
            let mut follower = spawn_server(&follower_config).await;
            wait_for_ping(&client, &leader_base).await;
            wait_for_ping(&client, &follower_base).await;

            set_point_for_id(&client, &leader_base, "truck-1", 52.52, 13.405).await;
            wait_for_object_present(&client, &follower_base, "fleet", "truck-1").await;

            follower.kill().await.unwrap();
            let _ = follower.wait().await;

            set_point_for_id(&client, &leader_base, "truck-2", 48.13, 11.58).await;

            let mut follower = spawn_server(&follower_config).await;
            wait_for_ping(&client, &follower_base).await;
            wait_for_follower_caught_up(&client, &follower_base).await;

            let object = wait_for_object_present(&client, &follower_base, "fleet", "truck-2").await;
            assert_eq!(object["id"], "truck-2");

            leader.kill().await.unwrap();
            let _ = leader.wait().await;
            follower.kill().await.unwrap();
            let _ = follower.wait().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn follower_checksum_mismatch_triggers_resync() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let leader_http = free_port();
            let leader_capnp = free_port();
            let follower_http = free_port();
            let follower_capnp = free_port();
            let leader_config = dir.path().join("leader.json");
            let follower_config = dir.path().join("follower.json");
            let leader_aof = dir.path().join("leader.aof");
            let follower_aof = dir.path().join("follower.aof");

            write_replication_config(
                &leader_config,
                leader_http,
                leader_capnp,
                &leader_aof,
                "leader-3",
                "repl-secret",
                None,
            );
            write_replication_config(
                &follower_config,
                follower_http,
                follower_capnp,
                &follower_aof,
                "follower-3",
                "repl-secret",
                None,
            );

            let client = reqwest::Client::new();
            let leader_base = format!("http://127.0.0.1:{leader_http}");
            let follower_base = format!("http://127.0.0.1:{follower_http}");

            let mut leader = spawn_server(&leader_config).await;
            let mut follower = spawn_server(&follower_config).await;
            wait_for_ping(&client, &leader_base).await;
            wait_for_ping(&client, &follower_base).await;

            set_point_for_id(&client, &leader_base, "leader-only", 52.52, 13.405).await;
            set_point_for_id(&client, &follower_base, "local-only", 48.13, 11.58).await;

            follower.kill().await.unwrap();
            let _ = follower.wait().await;
            write_replication_config(
                &follower_config,
                follower_http,
                follower_capnp,
                &follower_aof,
                "follower-3",
                "repl-secret",
                Some(("127.0.0.1", leader_capnp)),
            );
            let mut follower = spawn_server(&follower_config).await;
            wait_for_ping(&client, &follower_base).await;
            wait_for_follower_caught_up(&client, &follower_base).await;

            let leader_only =
                wait_for_object_present(&client, &follower_base, "fleet", "leader-only").await;
            assert_eq!(leader_only["id"], "leader-only");

            let local_only = client
                .get(format!(
                    "{follower_base}/collections/fleet/objects/local-only"
                ))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .await
                .unwrap();
            assert_eq!(local_only.status(), StatusCode::OK);
            let local_only = response_json(local_only).await;
            assert_eq!(local_only, serde_json::Value::Null);

            leader.kill().await.unwrap();
            let _ = leader.wait().await;
            follower.kill().await.unwrap();
            let _ = follower.wait().await;
        })
        .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn server_handles_sigterm_and_reopens_aof_cleanly() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let config_path = dir.path().join("latlng.json");

            write_config(&config_path, http_port, capnp_port, &aof_path);

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            set_point(&client, &base).await;
            send_sigterm(&mut first).await;

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            let object = wait_for_object_present(&client, &base, "fleet", "truck-1").await;
            assert_eq!(object["id"], "truck-1");
            second.kill().await.unwrap();
            let _ = second.wait().await;
        })
        .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn server_preserves_acknowledged_concurrent_writes_across_sigterm() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let dir = tempdir().unwrap();
            let http_port = free_port();
            let capnp_port = free_port();
            let aof_path = dir.path().join("appendonly.aof");
            let config_path = dir.path().join("latlng.json");

            write_config(&config_path, http_port, capnp_port, &aof_path);

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{http_port}");

            let mut first = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            let writes = (0..8_u32).map(|index| {
                let client = client.clone();
                let base = base.clone();
                async move {
                    set_point_for_id(
                        &client,
                        &base,
                        &format!("truck-{index}"),
                        52.52 + f64::from(index) * 0.0001,
                        13.405 + f64::from(index) * 0.0001,
                    )
                    .await;
                }
            });
            futures_util::future::join_all(writes).await;
            send_sigterm(&mut first).await;

            let mut second = spawn_server(&config_path).await;
            wait_for_ping(&client, &base).await;
            for index in 0..8_u32 {
                let object =
                    wait_for_object_present(&client, &base, "fleet", &format!("truck-{index}"))
                        .await;
                assert_eq!(object["id"], format!("truck-{index}"));
            }
            second.kill().await.unwrap();
            let _ = second.wait().await;
        })
        .await;
}

fn write_config(path: &Path, http_port: u16, capnp_port: u16, aof_path: &Path) {
    write_config_with_webhook_settings(
        path, http_port, capnp_port, aof_path, None, 5_000, 128, 8, 200, 30_000,
    );
}

fn write_replication_config(
    path: &Path,
    http_port: u16,
    capnp_port: u16,
    aof_path: &Path,
    server_id: &str,
    replication_credential: &str,
    follow: Option<(&str, u16)>,
) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(&serde_json::json!({
            "listen_addr": format!("127.0.0.1:{http_port}"),
            "capnp_enabled": true,
            "capnp_listen_addr": format!("127.0.0.1:{capnp_port}"),
            "server_id": server_id,
            "storage": {
                "type": "aof",
                "path": aof_path
            },
            "bearer_token": "secret",
            "replication_credential": replication_credential,
            "follow_host": follow.map(|value| value.0),
            "follow_port": follow.map(|value| value.1),
            "replication_batch_size": 32,
            "replication_reconnect_backoff_ms": 50
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_config_with_hook_settings(
    path: &Path,
    http_port: u16,
    capnp_port: u16,
    aof_path: &Path,
    webhook_timeout_ms: u64,
    webhook_concurrency_limit: usize,
) {
    write_config_with_webhook_settings(
        path,
        http_port,
        capnp_port,
        aof_path,
        None,
        webhook_timeout_ms,
        webhook_concurrency_limit,
        8,
        200,
        30_000,
    );
}

#[allow(clippy::too_many_arguments)]
fn write_config_with_webhook_settings(
    path: &Path,
    http_port: u16,
    capnp_port: u16,
    aof_path: &Path,
    webhook_queue_path: Option<&Path>,
    webhook_timeout_ms: u64,
    webhook_concurrency_limit: usize,
    webhook_retry_count: u32,
    webhook_retry_initial_backoff_ms: u64,
    webhook_retry_max_backoff_ms: u64,
) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(&serde_json::json!({
            "listen_addr": format!("127.0.0.1:{http_port}"),
            "capnp_enabled": true,
            "capnp_listen_addr": format!("127.0.0.1:{capnp_port}"),
            "storage": {
                "type": "aof",
                "path": aof_path
            },
            "bearer_token": "secret",
            "webhook_queue_path": webhook_queue_path.map(|value| value.display().to_string()),
            "webhook_timeout_ms": webhook_timeout_ms,
            "webhook_concurrency_limit": webhook_concurrency_limit,
            "webhook_retry_count": webhook_retry_count,
            "webhook_retry_initial_backoff_ms": webhook_retry_initial_backoff_ms,
            "webhook_retry_max_backoff_ms": webhook_retry_max_backoff_ms,
            "webhook_lease_ms": 1_000
        }))
        .unwrap(),
    )
    .unwrap();
}

async fn spawn_server(config_path: &Path) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_latlng-server"));
    command
        .kill_on_drop(true)
        .arg("--config")
        .arg(config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn().unwrap()
}

#[cfg(unix)]
async fn send_sigterm(child: &mut Child) {
    let pid = child.id().expect("child pid must exist");
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .await
        .unwrap();
    assert!(status.success());
    timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
}

async fn wait_for_ping(client: &reqwest::Client, base: &str) {
    for _ in 0..60 {
        let response = client
            .get(format!("{base}/ping"))
            .header(AUTHORIZATION, "Bearer secret")
            .send()
            .await;
        if matches!(response, Ok(resp) if resp.status().is_success()) {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become ready");
}

async fn set_channel(client: &reqwest::Client, base: &str) {
    let response = client
        .post(format!("{base}/channels"))
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(
            serde_json::json!({
                "name": "fleet-events",
                "def": {
                    "collection": "fleet",
                    "query": {
                        "Nearby": {
                            "lat": 52.52,
                            "lon": 13.405,
                            "meters": 500.0,
                            "options": {}
                        }
                    },
                    "detect": ["Enter"],
                    "commands": ["Set"]
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

async fn set_point(client: &reqwest::Client, base: &str) {
    set_point_for_id(client, base, "truck-1", 52.52, 13.405).await;
}

async fn set_point_for_id(client: &reqwest::Client, base: &str, id: &str, lat: f64, lon: f64) {
    let response = client
        .post(format!("{base}/collections/fleet/objects/{id}"))
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(
            serde_json::json!({
                "object": {
                    "Point": {
                        "lat": lat,
                        "lon": lon,
                        "z": null
                    }
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

async fn set_hook(client: &reqwest::Client, base: &str, endpoint: &str) {
    let response = client
        .post(format!("{base}/hooks"))
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(
            serde_json::json!({
                "name": "fleet-hook",
                "endpoint": endpoint,
                "def": {
                    "collection": "fleet",
                    "query": {
                        "Nearby": {
                            "lat": 52.52,
                            "lon": 13.405,
                            "meters": 500.0,
                            "options": {}
                        }
                    },
                    "detect": ["Enter"],
                    "commands": ["Set"]
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

async fn set_geojson_hook(client: &reqwest::Client, base: &str, endpoint: &str) {
    let response = client
        .post(format!("{base}/hooks"))
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(
            serde_json::json!({
                "name": "fleet-hook",
                "endpoint": endpoint,
                "def": {
                    "collection": "fleet",
                    "query": {
                        "Within": {
                            "area": {
                                "GeoJson": {
                                    "type": "Polygon",
                                    "coordinates": [[
                                        [13.39, 52.51],
                                        [13.42, 52.51],
                                        [13.42, 52.53],
                                        [13.39, 52.53],
                                        [13.39, 52.51]
                                    ]]
                                }
                            },
                            "options": {}
                        }
                    },
                    "detect": ["Enter"],
                    "commands": ["Set"]
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

#[derive(Clone, Default)]
struct HookState {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    delivered: Arc<AtomicUsize>,
}

async fn slow_hook(
    State(state): State<HookState>,
    Json(_payload): Json<serde_json::Value>,
) -> StatusCode {
    let current = state.active.fetch_add(1, Ordering::Relaxed) + 1;
    state.max_active.fetch_max(current, Ordering::Relaxed);
    sleep(Duration::from_millis(200)).await;
    state.active.fetch_sub(1, Ordering::Relaxed);
    state.delivered.fetch_add(1, Ordering::Relaxed);
    StatusCode::OK
}

async fn fast_hook(
    State(state): State<HookState>,
    Json(_payload): Json<serde_json::Value>,
) -> StatusCode {
    state.delivered.fetch_add(1, Ordering::Relaxed);
    StatusCode::OK
}

async fn wait_for_hook_deliveries(state: &HookState, expected: usize) {
    for _ in 0..240 {
        if state.delivered.load(Ordering::Relaxed) >= expected {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "expected {expected} webhook deliveries, saw {}",
        state.delivered.load(Ordering::Relaxed)
    );
}

async fn wait_for_metric_at_least(
    client: &reqwest::Client,
    base: &str,
    field: &str,
    expected: u64,
) {
    let metric_name = format!("latlng_{field}");
    for _ in 0..120 {
        let response = client
            .get(format!("{base}/metrics"))
            .header(AUTHORIZATION, "Bearer secret")
            .send()
            .await
            .unwrap();
        let metrics = response_text(response).await;
        if metric_value(&metrics, &metric_name).unwrap_or_default() >= expected {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("metric {field} did not reach {expected}");
}

async fn wait_for_ws_event(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) -> serde_json::Value {
    loop {
        let message = timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            let json = serde_json::from_str::<serde_json::Value>(&text).unwrap();
            if json.get("collection").is_some() {
                return json;
            }
        }
    }
}

async fn response_json(response: reqwest::Response) -> serde_json::Value {
    response.json().await.unwrap()
}

async fn response_text(response: reqwest::Response) -> String {
    response.text().await.unwrap()
}

fn metric_value(metrics: &str, name: &str) -> Option<u64> {
    metrics.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? == name {
            parts.next()?.parse::<u64>().ok()
        } else {
            None
        }
    })
}

async fn wait_for_follower_caught_up(client: &reqwest::Client, base: &str) -> serde_json::Value {
    for _ in 0..120 {
        let response = client
            .get(format!("{base}/server"))
            .header(AUTHORIZATION, "Bearer secret")
            .send()
            .await
            .unwrap();
        if response.status().is_success() {
            let json = response_json(response).await;
            if json["leader"] == false && json["caught_up_once"] == true {
                return json;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("follower did not catch up");
}

async fn wait_for_object_present(
    client: &reqwest::Client,
    base: &str,
    collection: &str,
    id: &str,
) -> serde_json::Value {
    for _ in 0..120 {
        let response = client
            .get(format!("{base}/collections/{collection}/objects/{id}"))
            .header(AUTHORIZATION, "Bearer secret")
            .send()
            .await
            .unwrap();
        if response.status().is_success() {
            let json = response_json(response).await;
            if !json.is_null() {
                return json;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("object {collection}/{id} did not appear");
}

async fn read_ws_message(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) -> serde_json::Value {
    let message = timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match message {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("unexpected websocket message: {other:?}"),
    }
}

async fn connect_capnp(addr: std::net::SocketAddr) -> lat_lng::Client {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (reader, writer) = tokio::io::split(stream);
    let network = twoparty::VatNetwork::new(
        reader.compat(),
        writer.compat_write(),
        rpc_twoparty_capnp::Side::Client,
        ReaderOptions::new(),
    );
    let mut rpc_system = RpcSystem::new(Box::new(network), None);
    let client: lat_lng::Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
    tokio::task::spawn_local(async move {
        let _ = rpc_system.await;
    });
    client
}

fn free_port() -> u16 {
    StdTcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
