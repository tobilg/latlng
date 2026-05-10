use std::sync::Arc;

use capnp::message::ReaderOptions;
use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use latlng_auth::Authenticator;
use latlng_config::{SharedFlushDbCoordinator, SharedRuntimeConfig};
use latlng_core::storage::StorageBackend;
use latlng_core::{LatLngNative, ServerInfo};
use latlng_native_executor::NativeExecutor;
use latlng_replication::SharedReplicationStatus;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::CapnpError;
use crate::rpc_state::LatLngRpc;
use crate::runtime::{apply_replication_to_server_info, snapshot_replication_status};
use crate::{lat_lng, schema};

pub type CapnpAuthConfig = Authenticator;

pub struct CapnpService<S: StorageBackend> {
    executor: NativeExecutor<S>,
    runtime_config: Option<SharedRuntimeConfig>,
    flushdb_coordinator: Option<SharedFlushDbCoordinator>,
    outbox_notify: Option<Arc<Notify>>,
    replication_notify: Option<Arc<Notify>>,
    replication_status: Option<SharedReplicationStatus>,
}

#[derive(Clone)]
pub struct CapnpRuntimeBindings {
    pub runtime_config: Option<SharedRuntimeConfig>,
    pub flushdb_coordinator: Option<SharedFlushDbCoordinator>,
    pub outbox_notify: Option<Arc<Notify>>,
    pub replication_notify: Option<Arc<Notify>>,
    pub replication_status: Option<SharedReplicationStatus>,
}

impl<S: StorageBackend> Clone for CapnpService<S> {
    fn clone(&self) -> Self {
        Self {
            executor: self.executor.clone(),
            runtime_config: self.runtime_config.as_ref().map(Arc::clone),
            flushdb_coordinator: self.flushdb_coordinator.as_ref().map(Arc::clone),
            outbox_notify: self.outbox_notify.as_ref().map(Arc::clone),
            replication_notify: self.replication_notify.as_ref().map(Arc::clone),
            replication_status: self.replication_status.as_ref().map(Arc::clone),
        }
    }
}

impl<S> CapnpService<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    pub fn new(db: Arc<LatLngNative<S>>) -> Self {
        Self {
            executor: NativeExecutor::with_defaults(Arc::clone(&db))
                .expect("failed to start native executor"),
            runtime_config: None,
            flushdb_coordinator: None,
            outbox_notify: None,
            replication_notify: None,
            replication_status: None,
        }
    }

    pub fn with_runtime_config(
        db: Arc<LatLngNative<S>>,
        runtime_config: SharedRuntimeConfig,
    ) -> Self {
        Self {
            executor: NativeExecutor::with_defaults(Arc::clone(&db))
                .expect("failed to start native executor"),
            runtime_config: Some(runtime_config),
            flushdb_coordinator: None,
            outbox_notify: None,
            replication_notify: None,
            replication_status: None,
        }
    }

    pub fn with_runtime_config_flushdb_coordinator_and_executor(
        _db: Arc<LatLngNative<S>>,
        executor: NativeExecutor<S>,
        bindings: CapnpRuntimeBindings,
    ) -> Self {
        Self {
            executor,
            runtime_config: bindings.runtime_config,
            flushdb_coordinator: bindings.flushdb_coordinator,
            outbox_notify: bindings.outbox_notify,
            replication_notify: bindings.replication_notify,
            replication_status: bindings.replication_status,
        }
    }

    pub fn available() -> bool {
        schema::CAPNP_CODEGEN_AVAILABLE
    }

    pub fn schema_codegen_available() -> bool {
        schema::CAPNP_CODEGEN_AVAILABLE
    }

    pub fn schema_text() -> &'static str {
        schema::SCHEMA_TEXT
    }

    pub async fn ping(&self) -> bool {
        true
    }

    pub async fn server_info(&self) -> ServerInfo {
        let mut info = self
            .run_value(|db| db.server_info())
            .await
            .unwrap_or_default();
        apply_replication_to_server_info(
            &mut info,
            self.replication_status
                .as_ref()
                .map(snapshot_replication_status)
                .as_ref(),
        );
        info
    }

    pub async fn serve(&self, addr: &str, auth: CapnpAuthConfig) -> Result<(), CapnpError> {
        let listener = TcpListener::bind(addr).await?;
        self.serve_listener(listener, auth).await
    }

    pub async fn serve_listener(
        &self,
        listener: TcpListener,
        auth: CapnpAuthConfig,
    ) -> Result<(), CapnpError> {
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            let connection_id = Uuid::new_v4().to_string();
            info!(
                connection_id = %connection_id,
                peer = %peer_addr,
                "capnp connection opened"
            );
            let (reader, writer) = tokio::io::split(stream);
            let network = twoparty::VatNetwork::new(
                reader.compat(),
                writer.compat_write(),
                rpc_twoparty_capnp::Side::Server,
                ReaderOptions::new(),
            );
            let bootstrap: lat_lng::Client = capnp_rpc::new_client(LatLngRpc::new(
                self.executor.clone(),
                CapnpRuntimeBindings {
                    runtime_config: self.runtime_config.as_ref().map(Arc::clone),
                    flushdb_coordinator: self.flushdb_coordinator.as_ref().map(Arc::clone),
                    outbox_notify: self.outbox_notify.as_ref().map(Arc::clone),
                    replication_notify: self.replication_notify.as_ref().map(Arc::clone),
                    replication_status: self.replication_status.as_ref().map(Arc::clone),
                },
                auth.clone(),
            ));
            let rpc_system = RpcSystem::new(Box::new(network), Some(bootstrap.client));
            tokio::task::spawn_local(async move {
                match rpc_system.await {
                    Ok(()) => {
                        info!(
                            connection_id = %connection_id,
                            peer = %peer_addr,
                            "capnp connection closed"
                        );
                    }
                    Err(error) => {
                        warn!(
                            connection_id = %connection_id,
                            peer = %peer_addr,
                            error = %error,
                            "capnp connection failed"
                        );
                    }
                }
            });
        }
    }
}

impl<S> CapnpService<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    async fn run_value<T, F>(&self, op: F) -> Result<T, capnp::Error>
    where
        T: Send + 'static,
        F: FnOnce(&LatLngNative<S>) -> T + Send + 'static,
    {
        self.executor
            .execute(op)
            .await
            .map_err(|error| capnp::Error::failed(error.to_string()))
    }
}
