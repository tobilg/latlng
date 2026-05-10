#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use latlng_auth::{AuthAction, AuthError, AuthPrincipal, Authenticator, extract_bearer_token};
use latlng_core::LatLngNative;
use latlng_core::geofence::{GeofenceEvent, GeofenceEventReceiver};
use latlng_core::platform::NativePlatform;
use latlng_core::storage::StorageBackend;
use latlng_native_executor::NativeExecutor;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

pub use latlng_core as core;

pub type WsAuthConfig = Authenticator;

pub struct WsState<S: StorageBackend> {
    pub db: Arc<LatLngNative<S>>,
    pub executor: NativeExecutor<S>,
    pub auth: WsAuthConfig,
}

impl<S: StorageBackend> Clone for WsState<S> {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            executor: self.executor.clone(),
            auth: self.auth.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsCommand {
    Auth { token: String },
    Subscribe { channels: Vec<String> },
    Psubscribe { patterns: Vec<String> },
    Ping,
    Quit,
}

pub fn ws_route<S>(state: WsState<S>) -> axum::Router
where
    S: StorageBackend + Send + Sync + 'static,
{
    axum::Router::new()
        .route("/ws", axum::routing::get(ws_handler::<S>))
        .with_state(state)
}

async fn ws_handler<S>(
    State(state): State<WsState<S>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authorize(&state.auth, &headers).await?;
    let request_id = request_id(&headers);
    let connection_id = Uuid::new_v4().to_string();
    let response_request_id = request_id.clone();
    let mut response = ws
        .on_upgrade(move |socket| client_loop(state, socket, principal, request_id, connection_id));
    if let Ok(value) = HeaderValue::from_str(&response_request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    Ok(response)
}

async fn client_loop<S>(
    state: WsState<S>,
    mut socket: WebSocket,
    mut principal: Option<AuthPrincipal>,
    request_id: String,
    connection_id: String,
) where
    S: StorageBackend + Send + Sync + 'static,
{
    info!(
        request_id = %request_id,
        connection_id = %connection_id,
        authenticated = principal.is_some(),
        "websocket connection opened"
    );
    let mut receiver: Option<mpsc::UnboundedReceiver<GeofenceEvent>> = None;
    let mut receiver_bridge: Option<ReceiverBridge> = None;

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(message) = incoming else {
                    break;
                };
                let Ok(message) = message else {
                    break;
                };
                match message {
                    Message::Text(payload) => {
                        let Ok(command) = serde_json::from_str::<WsCommand>(&payload) else {
                            if send_json(&mut socket, serde_json::json!({ "error": "invalid websocket command" })).await.is_err() {
                                break;
                            }
                            continue;
                        };

                        match command {
                            WsCommand::Auth { token } => {
                                match state.auth.authenticate(Some(&token)).await {
                                    Ok(next_principal) => {
                                        principal = Some(next_principal);
                                        info!(request_id = %request_id, connection_id = %connection_id, "websocket authenticated");
                                        if send_json(&mut socket, serde_json::json!({ "ok": true, "authorized": true })).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => {
                                        principal = None;
                                        warn!(request_id = %request_id, connection_id = %connection_id, "websocket authentication failed");
                                        if send_json(&mut socket, serde_json::json!({ "error": "unauthorized" })).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            WsCommand::Ping => {
                                if send_json(&mut socket, serde_json::json!({ "ok": true, "pong": true })).await.is_err() {
                                    break;
                                }
                            }
                            WsCommand::Quit => break,
                            WsCommand::Subscribe { channels } => {
                                let Some(current_principal) = principal.as_ref() else {
                                    if send_json(&mut socket, serde_json::json!({ "error": "unauthorized" })).await.is_err() {
                                        break;
                                    }
                                    continue;
                                };
                                let subscribed = channels.clone();
                                let executor = state.executor.clone();
                                let channel_lookup = channels.clone();
                                let channel_defs = executor.execute(move |db: &LatLngNative<S>| {
                                    Ok::<_, latlng_core::CoreError>(
                                        channel_lookup
                                            .iter()
                                            .filter_map(|name| db.channel_def(name))
                                            .collect::<Vec<_>>(),
                                    )
                                }).await;
                                let Ok(channel_defs) = channel_defs else {
                                    break;
                                };
                                let Ok(channel_defs) = channel_defs else {
                                    break;
                                };
                                if channel_defs.iter().any(|channel| {
                                    !current_principal.allows(
                                        AuthAction::SubscriptionsRead,
                                        &channel.def.collection,
                                    )
                                }) {
                                    if send_json(&mut socket, serde_json::json!({ "error": "forbidden" })).await.is_err() {
                                        break;
                                    }
                                    continue;
                                }
                                let next_receiver = executor.execute(move |db: &LatLngNative<S>| {
                                    let refs = channels.iter().map(String::as_str).collect::<Vec<_>>();
                                    db.subscribe(&refs)
                                })
                                .await;
                                let Ok(next_receiver) = next_receiver else {
                                    break;
                                };
                                if let Some(bridge) = receiver_bridge.take() {
                                    bridge.stop();
                                }
                                let (next_events, next_bridge) = spawn_receiver_bridge(next_receiver);
                                receiver = Some(next_events);
                                receiver_bridge = Some(next_bridge);
                                info!(
                                    request_id = %request_id,
                                    connection_id = %connection_id,
                                    subscription_count = subscribed.len(),
                                    "websocket subscribed"
                                );
                                if send_json(&mut socket, serde_json::json!({ "ok": true, "subscribed": subscribed })).await.is_err() {
                                    break;
                                }
                            }
                            WsCommand::Psubscribe { patterns } => {
                                let Some(current_principal) = principal.as_ref() else {
                                    if send_json(&mut socket, serde_json::json!({ "error": "unauthorized" })).await.is_err() {
                                        break;
                                    }
                                    continue;
                                };
                                if !current_principal.any_collection_permission(AuthAction::SubscriptionsRead)
                                    && !current_principal.is_admin()
                                {
                                    if send_json(&mut socket, serde_json::json!({ "error": "forbidden" })).await.is_err() {
                                        break;
                                    }
                                    continue;
                                }
                                let psubscribed = patterns.clone();
                                let executor = state.executor.clone();
                                let next_receiver = executor.execute(move |db: &LatLngNative<S>| {
                                    let refs = patterns.iter().map(String::as_str).collect::<Vec<_>>();
                                    db.psubscribe(&refs)
                                })
                                .await;
                                let Ok(next_receiver) = next_receiver else {
                                    break;
                                };
                                if let Some(bridge) = receiver_bridge.take() {
                                    bridge.stop();
                                }
                                let (next_events, next_bridge) = spawn_receiver_bridge(next_receiver);
                                receiver = Some(next_events);
                                receiver_bridge = Some(next_bridge);
                                info!(
                                    request_id = %request_id,
                                    connection_id = %connection_id,
                                    pattern_count = psubscribed.len(),
                                    "websocket pattern subscribed"
                                );
                                if send_json(&mut socket, serde_json::json!({ "ok": true, "psubscribed": psubscribed })).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            event = recv_bridge_event(&mut receiver), if receiver.is_some() => {
                match event {
                    Some(event) => {
                        let Some(current_principal) = principal.as_ref() else {
                            continue;
                        };
                        if !current_principal.allows(AuthAction::SubscriptionsRead, &event.collection) {
                            continue;
                        }
                        if socket
                            .send(Message::Text(
                                serde_json::to_string(&event)
                                    .unwrap_or_else(|_| "{}".to_owned())
                                    .into(),
                            ))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    None => {
                        receiver = None;
                        receiver_bridge = None;
                    }
                }
            }
        }
    }

    if let Some(bridge) = receiver_bridge {
        bridge.stop();
    }
    info!(
        request_id = %request_id,
        connection_id = %connection_id,
        "websocket connection closed"
    );
}

fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

async fn authorize(
    config: &WsAuthConfig,
    headers: &HeaderMap,
) -> Result<Option<AuthPrincipal>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(extract_bearer_token);
    if token.is_none() {
        return Ok((!config.config().auth_enabled()).then(AuthPrincipal::open_access));
    }
    config
        .authenticate(token)
        .await
        .map(Some)
        .map_err(|error| match error {
            AuthError::Unauthorized => StatusCode::UNAUTHORIZED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })
}

async fn send_json(socket: &mut WebSocket, value: serde_json::Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(value.to_string().into())).await
}

async fn recv_bridge_event(
    receiver: &mut Option<mpsc::UnboundedReceiver<GeofenceEvent>>,
) -> Option<GeofenceEvent> {
    match receiver.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => None,
    }
}

struct ReceiverBridge {
    cancel: Arc<AtomicBool>,
    wake: latlng_core::platform::NativeWakeHandle<GeofenceEvent>,
}

impl ReceiverBridge {
    fn stop(self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.wake.wake();
    }
}

fn spawn_receiver_bridge(
    mut receiver: GeofenceEventReceiver<NativePlatform>,
) -> (mpsc::UnboundedReceiver<GeofenceEvent>, ReceiverBridge) {
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let wake = receiver.wake_handle();
    let worker_cancel = Arc::clone(&cancel);
    tokio::task::spawn_blocking(move || {
        while let Some(event) = receiver.recv_blocking_with_cancel(worker_cancel.as_ref()) {
            if events_tx.send(event).is_err() {
                break;
            }
        }
    });
    (events_rx, ReceiverBridge { cancel, wake })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use latlng_auth::AuthConfig;
    use latlng_core::{
        LatLng, LatLngNative,
        geofence::{DetectType, GeofenceDef, GeofenceQuery, MutationCommand},
        index::SearchOptions,
    };
    use latlng_native_executor::NativeExecutor;
    use latlng_storage_memory::MemoryBackend;
    use tokio::net::TcpListener;
    use tokio::time::timeout;
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

    use super::{WsState, ws_route};

    #[tokio::test]
    async fn websocket_subscribe_requires_subscriptions_read() {
        let db: LatLngNative<MemoryBackend> = LatLng::builder()
            .storage(MemoryBackend::new())
            .build()
            .unwrap();
        let db = Arc::new(db);
        db.setchan(
            "fleet-events",
            GeofenceDef {
                collection: "fleet".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 250.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter],
                commands: vec![MutationCommand::Set],
            },
        )
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = ws_route(WsState {
            db: Arc::clone(&db),
            executor: NativeExecutor::with_defaults(Arc::clone(&db)).unwrap(),
            auth: AuthConfig {
                jwt_secret: Some("jwt-secret".to_owned()),
                ..AuthConfig::default()
            }
            .authenticator()
            .unwrap(),
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let token = encode(
            &Header::new(Algorithm::HS256),
            &serde_json::json!({
                "sub": "reader",
                "exp": 4_102_444_800usize,
                "latlng_permissions": [
                    {
                        "collections": ["fleet"],
                        "actions": ["queries:read"]
                    }
                ]
            }),
            &EncodingKey::from_secret(b"jwt-secret"),
        )
        .unwrap();

        let (mut socket, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        socket
            .send(WsMessage::Text(
                serde_json::json!({
                    "type": "auth",
                    "token": token,
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let auth = timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let auth_text = auth.into_text().unwrap();
        assert!(auth_text.contains("\"authorized\":true"));

        socket
            .send(WsMessage::Text(
                serde_json::json!({
                    "type": "subscribe",
                    "channels": ["fleet-events"],
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let denied = timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let denied_text = denied.into_text().unwrap();
        assert!(denied_text.contains("\"error\":\"forbidden\""));

        server.abort();
    }
}
