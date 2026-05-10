use std::sync::Arc;

use latlng_auth::Authenticator;
use latlng_config::{SharedFlushDbCoordinator, SharedRuntimeConfig};
use latlng_core::LatLngNative;
use latlng_core::storage::StorageBackend;
use latlng_native_executor::NativeExecutor;
use latlng_replication::{SharedReplicationCoordinator, SharedReplicationStatus};
use latlng_webhook_queue::WebhookQueue;
use tokio::sync::Notify;

use crate::RequestMetrics;

pub struct HttpState<S: StorageBackend> {
    pub db: Arc<LatLngNative<S>>,
    pub executor: NativeExecutor<S>,
    pub auth: Authenticator,
    pub metrics: Arc<RequestMetrics>,
    pub runtime_config: Option<SharedRuntimeConfig>,
    pub webhook_queue: Option<Arc<WebhookQueue>>,
    pub flushdb_coordinator: Option<SharedFlushDbCoordinator>,
    pub outbox_notify: Option<Arc<Notify>>,
    pub replication_status: Option<SharedReplicationStatus>,
    pub replication_coordinator: Option<SharedReplicationCoordinator>,
    pub replication_notify: Option<Arc<Notify>>,
}

impl<S: StorageBackend> Clone for HttpState<S> {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            executor: self.executor.clone(),
            auth: self.auth.clone(),
            metrics: Arc::clone(&self.metrics),
            runtime_config: self.runtime_config.as_ref().map(Arc::clone),
            webhook_queue: self.webhook_queue.as_ref().map(Arc::clone),
            flushdb_coordinator: self.flushdb_coordinator.as_ref().map(Arc::clone),
            outbox_notify: self.outbox_notify.as_ref().map(Arc::clone),
            replication_status: self.replication_status.as_ref().map(Arc::clone),
            replication_coordinator: self.replication_coordinator.as_ref().map(Arc::clone),
            replication_notify: self.replication_notify.as_ref().map(Arc::clone),
        }
    }
}
