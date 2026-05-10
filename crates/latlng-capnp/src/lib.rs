#![forbid(unsafe_code)]

mod codec;
mod error;
mod handlers;
mod rpc_state;
mod runtime;
mod service;
mod streams;

pub use error::CapnpError;
pub use latlng_core as core;
pub use latlng_schema as schema;
pub use service::{CapnpAuthConfig, CapnpRuntimeBindings, CapnpService};

pub(crate) use schema::latlng_capnp::{self as rpc, geofence_stream, lat_lng, replication_stream};

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use capnp::message::ReaderOptions;
    use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use latlng_auth::AuthConfig;
    use latlng_core::{
        LatLng, LatLngNative, SetCondition, SetRequest,
        geo::GeoType,
        geofence::{DetectType, GeofenceDef, GeofenceQuery, MutationCommand},
        index::SearchOptions,
    };
    use latlng_storage_memory::MemoryBackend;
    use tokio::net::TcpStream;
    use tokio::task::LocalSet;
    use tokio::time::timeout;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    use super::{CapnpService, lat_lng};

    #[tokio::test(flavor = "current_thread")]
    async fn capnp_service_handles_ping_server_and_async_events() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let db: LatLngNative<MemoryBackend> = LatLng::builder()
                    .storage(MemoryBackend::new())
                    .build()
                    .unwrap();
                let shared = Arc::new(db);
                let service = CapnpService::new(Arc::clone(&shared));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();

                let server = tokio::task::spawn_local({
                    let service = service.clone();
                    async move {
                        let _ = service
                            .serve_listener(
                                listener,
                                AuthConfig::default().authenticator().unwrap(),
                            )
                            .await;
                    }
                });

                let client = connect(addr).await;

                let ping = client.ping_request().send().promise.await.unwrap();
                assert!(ping.get().unwrap().get_resp().unwrap().get_ok());

                let info = client.server_request().send().promise.await.unwrap();
                assert_eq!(info.get().unwrap().get_info().unwrap().get_num_objects(), 0);

                let mut subscribe = client.psubscribe_request();
                subscribe.get().init_patterns(1).set(0, "*");
                let stream = subscribe.send().promise.await.unwrap();
                let stream = stream.get().unwrap().get_stream().unwrap();

                shared
                    .setchan(
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
                shared
                    .set(SetRequest {
                        collection: "fleet".to_owned(),
                        id: "truck-1".to_owned(),
                        object: GeoType::point(52.52, 13.405),
                        fields: Vec::new(),
                        expire_seconds: None,
                        condition: SetCondition::Always,
                    })
                    .unwrap();

                let event = timeout(Duration::from_secs(2), stream.next_request().send().promise)
                    .await
                    .unwrap()
                    .unwrap();
                let event = event.get().unwrap();
                assert!(!event.get_done());
                let event = event.get_event().unwrap();
                assert_eq!(event.get_collection().unwrap(), "fleet");
                assert_eq!(event.get_id().unwrap(), "truck-1");

                server.abort();
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn capnp_service_enforces_auth_via_auth_command() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let db: LatLngNative<MemoryBackend> = LatLng::builder()
                    .storage(MemoryBackend::new())
                    .build()
                    .unwrap();
                let service = CapnpService::new(Arc::new(db));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();

                let server = tokio::task::spawn_local({
                    let service = service.clone();
                    async move {
                        let _ = service
                            .serve_listener(
                                listener,
                                AuthConfig {
                                    bearer_token: Some("secret".to_owned()),
                                    ..AuthConfig::default()
                                }
                                .authenticator()
                                .unwrap(),
                            )
                            .await;
                    }
                });

                let client = connect(addr).await;
                assert!(client.ping_request().send().promise.await.is_err());

                let mut auth = client.auth_request();
                auth.get().set_token("secret");
                let auth = auth.send().promise.await.unwrap();
                assert!(auth.get().unwrap().get_resp().unwrap().get_ok());

                let ping = client.ping_request().send().promise.await.unwrap();
                assert!(ping.get().unwrap().get_resp().unwrap().get_ok());

                let mut timeout_request = client.timeout_request();
                timeout_request.get().set_seconds(1.5);
                timeout_request.get().set_command("set");
                let timeout = timeout_request.send().promise.await.unwrap();
                assert!(timeout.get().unwrap().get_resp().unwrap().get_ok());

                server.abort();
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn capnp_service_enforces_collection_permissions_after_auth() {
        let local = LocalSet::new();
        local
            .run_until(async {
                let db: LatLngNative<MemoryBackend> = LatLng::builder()
                    .storage(MemoryBackend::new())
                    .build()
                    .unwrap();
                db.create_collection("fleet").unwrap();
                db.set(SetRequest {
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: Vec::new(),
                    expire_seconds: None,
                    condition: SetCondition::Always,
                })
                .unwrap();
                let service = CapnpService::new(Arc::new(db));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();

                let server = tokio::task::spawn_local({
                    let service = service.clone();
                    async move {
                        let _ = service
                            .serve_listener(
                                listener,
                                AuthConfig {
                                    jwt_secret: Some("secret".to_owned()),
                                    ..AuthConfig::default()
                                }
                                .authenticator()
                                .unwrap(),
                            )
                            .await;
                    }
                });

                let token = encode(
                    &Header::new(Algorithm::HS256),
                    &serde_json::json!({
                        "sub": "reader",
                        "exp": 4_102_444_800usize,
                        "latlng_permissions": [
                            {
                                "collections": ["fleet"],
                                "actions": ["collections:list", "objects:read"]
                            }
                        ]
                    }),
                    &EncodingKey::from_secret(b"secret"),
                )
                .unwrap();

                let client = connect(addr).await;
                let mut auth = client.auth_request();
                auth.get().set_token(&token);
                let auth = auth.send().promise.await.unwrap();
                assert!(auth.get().unwrap().get_resp().unwrap().get_ok());

                let get = {
                    let mut req = client.get_request();
                    let mut payload = req.get().init_req();
                    payload.set_collection("fleet");
                    payload.set_id("truck-1");
                    req.send().promise.await.unwrap()
                };
                assert!(get.get().unwrap().get_ok());

                let mut del = client.del_request();
                del.get().set_collection("fleet");
                del.get().set_id("truck-1");
                match del.send().promise.await {
                    Ok(_) => panic!("delete should be forbidden"),
                    Err(error) => assert!(error.to_string().contains("forbidden")),
                }

                server.abort();
            })
            .await;
    }

    async fn connect(addr: std::net::SocketAddr) -> lat_lng::Client {
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
}
