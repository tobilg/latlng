use latlng_config::{SharedRuntimeConfig, save_to_path};
use latlng_core::ServerInfo;
use latlng_core::storage::StorageBackend;
use latlng_native_executor::NativeExecutor;
use latlng_replication::{ReplicationStatus, SharedReplicationStatus};

use crate::CapnpError;
use crate::codec::capnp_failed;

pub(crate) fn update_runtime_config(
    runtime_config: Option<&SharedRuntimeConfig>,
    update: impl FnOnce(&mut latlng_config::RuntimeConfig),
) {
    if let Some(runtime) = runtime_config {
        match runtime.write() {
            Ok(mut guard) => update(&mut guard),
            Err(poisoned) => update(&mut poisoned.into_inner()),
        }
    }
}

pub(crate) fn rewrite_runtime_config(
    runtime_config: Option<&SharedRuntimeConfig>,
) -> Result<(), CapnpError> {
    let Some(runtime_config) = runtime_config else {
        return Err(CapnpError::Rpc("runtime config is not attached".to_owned()));
    };
    let snapshot = match runtime_config.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let path = snapshot
        .config_path
        .clone()
        .ok_or_else(|| CapnpError::Rpc("no config file path is configured".to_owned()))?;
    save_to_path(&snapshot, &path).map_err(|error| CapnpError::Rpc(error.to_string()))
}

pub(crate) async fn sync_effective_read_only<S>(
    executor: &NativeExecutor<S>,
    runtime_config: Option<&SharedRuntimeConfig>,
    replication_status: Option<&SharedReplicationStatus>,
) -> Result<(), capnp::Error>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let configured_read_only = runtime_config
        .map(|runtime| match runtime.read() {
            Ok(guard) => guard.read_only,
            Err(poisoned) => poisoned.into_inner().read_only,
        })
        .unwrap_or(false);
    let effective = replication_status
        .map(snapshot_replication_status)
        .map(|status| status.effective_read_only(configured_read_only))
        .unwrap_or(configured_read_only);
    executor
        .execute(move |db| match db.config().write() {
            Ok(mut guard) => guard.read_only = effective,
            Err(poisoned) => poisoned.into_inner().read_only = effective,
        })
        .await
        .map_err(|error| capnp_failed(error.to_string()))
}

pub(crate) fn snapshot_replication_status(status: &SharedReplicationStatus) -> ReplicationStatus {
    match status.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

pub(crate) fn apply_replication_to_server_info(
    info: &mut ServerInfo,
    replication: Option<&ReplicationStatus>,
) {
    if let Some(replication) = replication {
        info.server_id = replication.server_id.clone();
        info.following = replication.following();
        info.caught_up = replication.caught_up;
        info.caught_up_once = replication.caught_up_once;
        info.leader = !replication.is_follower();
        if replication.is_follower() {
            info.read_only = true;
        }
    }
}
