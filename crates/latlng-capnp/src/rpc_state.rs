use std::cell::RefCell;
use std::sync::Arc;

use latlng_auth::{AuthAction, AuthPrincipal};
use latlng_config::{SharedFlushDbCoordinator, SharedRuntimeConfig};
use latlng_core::LatLngNative;
use latlng_core::storage::StorageBackend;
use latlng_native_executor::NativeExecutor;
use latlng_replication::SharedReplicationStatus;
use tokio::sync::Notify;

use crate::codec::{capnp_failed, forbidden_error, unauthorized_error};
use crate::runtime::snapshot_replication_status;
use crate::service::{CapnpAuthConfig, CapnpRuntimeBindings};

pub(crate) struct LatLngRpc<S: StorageBackend> {
    pub(crate) executor: NativeExecutor<S>,
    pub(crate) runtime_config: Option<SharedRuntimeConfig>,
    pub(crate) flushdb_coordinator: Option<SharedFlushDbCoordinator>,
    pub(crate) outbox_notify: Option<Arc<Notify>>,
    pub(crate) replication_notify: Option<Arc<Notify>>,
    pub(crate) replication_status: Option<SharedReplicationStatus>,
    pub(crate) auth: CapnpAuthConfig,
    pub(crate) principal: RefCell<Option<AuthPrincipal>>,
}

impl<S> LatLngRpc<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    pub(crate) fn new(
        executor: NativeExecutor<S>,
        bindings: CapnpRuntimeBindings,
        auth: CapnpAuthConfig,
    ) -> Self {
        Self {
            executor,
            runtime_config: bindings.runtime_config,
            flushdb_coordinator: bindings.flushdb_coordinator,
            outbox_notify: bindings.outbox_notify,
            replication_notify: bindings.replication_notify,
            replication_status: bindings.replication_status,
            principal: RefCell::new(
                (!auth.config().auth_enabled()).then(AuthPrincipal::open_access),
            ),
            auth,
        }
    }

    pub(crate) fn ensure_authenticated(&self) -> Result<AuthPrincipal, capnp::Error> {
        if !self.auth.config().auth_enabled() {
            return Ok(AuthPrincipal::open_access());
        }
        self.principal
            .borrow()
            .clone()
            .ok_or_else(unauthorized_error)
    }

    pub(crate) fn ensure_global_action(
        &self,
        action: AuthAction,
    ) -> Result<AuthPrincipal, capnp::Error> {
        let principal = self.ensure_authenticated()?;
        if principal.allows_global(action) {
            Ok(principal)
        } else {
            Err(forbidden_error())
        }
    }

    pub(crate) fn ensure_collection_action(
        &self,
        action: AuthAction,
        collection: &str,
    ) -> Result<AuthPrincipal, capnp::Error> {
        let principal = self.ensure_authenticated()?;
        if principal.allows(action, collection) {
            Ok(principal)
        } else {
            Err(forbidden_error())
        }
    }

    pub(crate) fn ensure_any_collection_permission(
        &self,
        action: AuthAction,
    ) -> Result<AuthPrincipal, capnp::Error> {
        let principal = self.ensure_authenticated()?;
        if principal.is_admin() || principal.any_collection_permission(action) {
            Ok(principal)
        } else {
            Err(forbidden_error())
        }
    }

    pub(crate) fn ensure_queries_allowed(&self) -> Result<(), capnp::Error> {
        if self
            .replication_status
            .as_ref()
            .map(snapshot_replication_status)
            .is_some_and(|status| !status.queries_allowed())
        {
            return Err(capnp_failed("catching up to leader"));
        }
        Ok(())
    }

    pub(crate) fn ensure_replication_authorized(
        &self,
        credential: &str,
    ) -> Result<(), capnp::Error> {
        let Some(runtime_config) = &self.runtime_config else {
            return Err(capnp_failed("runtime config is not attached"));
        };
        let expected = match runtime_config.read() {
            Ok(guard) => guard.replication_credential.clone(),
            Err(poisoned) => poisoned.into_inner().replication_credential.clone(),
        };
        if expected.as_deref().is_some_and(|value| value == credential) {
            Ok(())
        } else {
            Err(unauthorized_error())
        }
    }

    pub(crate) async fn run_core<T, F>(&self, op: F) -> Result<T, capnp::Error>
    where
        T: Send + 'static,
        F: FnOnce(&LatLngNative<S>) -> latlng_core::Result<T> + Send + 'static,
    {
        self.executor
            .execute(op)
            .await
            .map_err(|error| capnp::Error::failed(error.to_string()))?
            .map_err(|error| capnp::Error::failed(error.to_string()))
    }

    pub(crate) async fn run_value<T, F>(&self, op: F) -> Result<T, capnp::Error>
    where
        T: Send + 'static,
        F: FnOnce(&LatLngNative<S>) -> T + Send + 'static,
    {
        self.executor
            .execute(op)
            .await
            .map_err(|error| capnp::Error::failed(error.to_string()))
    }

    pub(crate) async fn run_core_read<T, F>(&self, op: F) -> Result<T, capnp::Error>
    where
        T: Send + 'static,
        F: FnOnce(&LatLngNative<S>) -> latlng_core::Result<T> + Send + 'static,
    {
        self.ensure_queries_allowed()?;
        self.run_core(op).await
    }

    pub(crate) async fn run_core_mutating<T, F>(&self, op: F) -> Result<T, capnp::Error>
    where
        T: Send + 'static,
        F: FnOnce(&LatLngNative<S>) -> latlng_core::Result<T> + Send + 'static,
    {
        let result = self.run_core(op).await;
        if result.is_ok()
            && let Some(notify) = &self.outbox_notify
        {
            notify.notify_waiters();
        }
        if result.is_ok()
            && let Some(notify) = &self.replication_notify
        {
            notify.notify_waiters();
        }
        result
    }
}
