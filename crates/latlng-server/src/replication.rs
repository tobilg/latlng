#![forbid(unsafe_code)]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use capnp::message::ReaderOptions;
use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use latlng_capnp::schema::latlng_capnp::{lat_lng, replication_stream};
use latlng_config::SharedRuntimeConfig;
use latlng_core::{
    LatLngNative,
    storage::{StorageBackend, StorageEntry},
};
use latlng_native_executor::NativeExecutor;
use latlng_replication::{
    FollowTarget, ReplicationCoordinator, ReplicationFuture, ReplicationStatus,
    SharedReplicationCoordinator, SharedReplicationStatus,
};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{error, warn};

enum ControlCommand {
    Follow {
        target: FollowTarget,
        respond: oneshot::Sender<Result<(), String>>,
    },
    Unfollow {
        respond: oneshot::Sender<Result<(), String>>,
    },
}

struct ReplicationManager<S: StorageBackend> {
    sender: mpsc::UnboundedSender<ControlCommand>,
    _marker: std::marker::PhantomData<S>,
}

impl<S> ReplicationCoordinator for ReplicationManager<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    fn follow(&self, host: String, port: u16) -> ReplicationFuture<'_> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(ControlCommand::Follow {
                    target: FollowTarget { host, port },
                    respond: tx,
                })
                .map_err(|_| "replication manager is unavailable".to_owned())?;
            rx.await
                .map_err(|_| "replication manager is unavailable".to_owned())?
        })
    }

    fn unfollow(&self) -> ReplicationFuture<'_> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.sender
                .send(ControlCommand::Unfollow { respond: tx })
                .map_err(|_| "replication manager is unavailable".to_owned())?;
            rx.await
                .map_err(|_| "replication manager is unavailable".to_owned())?
        })
    }
}

pub async fn start_replication_manager<S>(
    _db: Arc<LatLngNative<S>>,
    executor: NativeExecutor<S>,
    runtime_config: SharedRuntimeConfig,
    wake_notify: Arc<Notify>,
) -> Result<(SharedReplicationStatus, SharedReplicationCoordinator), String>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let initial_target = configured_follow_target(&runtime_config);
    let server_id = match runtime_config.read() {
        Ok(guard) => guard.server_id.clone(),
        Err(poisoned) => poisoned.into_inner().server_id.clone(),
    };
    let status = Arc::new(RwLock::new(match initial_target.clone() {
        Some(target) => ReplicationStatus::follower(server_id, target),
        None => ReplicationStatus::leader(server_id),
    }));
    sync_effective_read_only(&executor, &runtime_config, &status).await?;

    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::task::spawn_local(replication_manager_loop(
        executor.clone(),
        runtime_config,
        wake_notify,
        status.clone(),
        receiver,
        initial_target,
    ));

    Ok((
        status,
        Arc::new(ReplicationManager::<S> {
            sender,
            _marker: std::marker::PhantomData,
        }) as SharedReplicationCoordinator,
    ))
}

async fn replication_manager_loop<S>(
    executor: NativeExecutor<S>,
    runtime_config: SharedRuntimeConfig,
    wake_notify: Arc<Notify>,
    status: SharedReplicationStatus,
    mut receiver: mpsc::UnboundedReceiver<ControlCommand>,
    initial_target: Option<FollowTarget>,
) where
    S: StorageBackend + Send + Sync + 'static,
{
    let mut worker = initial_target.map(|target| {
        tokio::task::spawn_local(run_follow_loop(
            executor.clone(),
            runtime_config.clone(),
            wake_notify.clone(),
            status.clone(),
            target,
        ))
    });

    while let Some(command) = receiver.recv().await {
        match command {
            ControlCommand::Follow { target, respond } => {
                let result = async {
                    if target.host.trim().is_empty() || target.port == 0 {
                        return Err(
                            "follow target must include a host and non-zero port".to_owned()
                        );
                    }
                    validate_follow_target(&runtime_config, &status, &target).await?;
                    match runtime_config.write() {
                        Ok(mut guard) => {
                            guard.follow_host = Some(target.host.clone());
                            guard.follow_port = Some(target.port);
                        }
                        Err(poisoned) => {
                            let mut guard = poisoned.into_inner();
                            guard.follow_host = Some(target.host.clone());
                            guard.follow_port = Some(target.port);
                        }
                    }
                    {
                        let mut guard = status
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *guard =
                            ReplicationStatus::follower(guard.server_id.clone(), target.clone());
                    }
                    sync_effective_read_only(&executor, &runtime_config, &status).await?;
                    wake_notify.notify_waiters();
                    if let Some(handle) = worker.take() {
                        handle.abort();
                    }
                    worker = Some(tokio::task::spawn_local(run_follow_loop(
                        executor.clone(),
                        runtime_config.clone(),
                        wake_notify.clone(),
                        status.clone(),
                        target,
                    )));
                    Ok(())
                }
                .await;
                let _ = respond.send(result);
            }
            ControlCommand::Unfollow { respond } => {
                let result = async {
                    match runtime_config.write() {
                        Ok(mut guard) => {
                            guard.follow_host = None;
                            guard.follow_port = None;
                        }
                        Err(poisoned) => {
                            let mut guard = poisoned.into_inner();
                            guard.follow_host = None;
                            guard.follow_port = None;
                        }
                    }
                    if let Some(handle) = worker.take() {
                        handle.abort();
                    }
                    {
                        let mut guard = status
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *guard = ReplicationStatus::leader(guard.server_id.clone());
                    }
                    sync_effective_read_only(&executor, &runtime_config, &status).await?;
                    wake_notify.notify_waiters();
                    Ok(())
                }
                .await;
                let _ = respond.send(result);
            }
        }
    }
}

async fn run_follow_loop<S>(
    executor: NativeExecutor<S>,
    runtime_config: SharedRuntimeConfig,
    wake_notify: Arc<Notify>,
    status: SharedReplicationStatus,
    target: FollowTarget,
) where
    S: StorageBackend + Send + Sync + 'static,
{
    loop {
        match follow_once(
            executor.clone(),
            runtime_config.clone(),
            status.clone(),
            &target,
        )
        .await
        {
            Ok(()) => {}
            Err(error) => {
                {
                    let mut guard = status
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !guard.is_follower() {
                        return;
                    }
                    guard.caught_up = false;
                    guard.reconnects_total = guard.reconnects_total.saturating_add(1);
                    guard.last_error = Some(error.clone());
                }
                warn!(target = %target.display(), error = %error, "replication follow attempt failed");
            }
        }
        wake_notify.notify_waiters();
        let backoff = match runtime_config.read() {
            Ok(guard) => guard.replication_reconnect_backoff_ms.max(1),
            Err(poisoned) => poisoned
                .into_inner()
                .replication_reconnect_backoff_ms
                .max(1),
        };
        tokio::time::sleep(Duration::from_millis(backoff)).await;
    }
}

async fn validate_follow_target(
    runtime_config: &SharedRuntimeConfig,
    status: &SharedReplicationStatus,
    target: &FollowTarget,
) -> Result<(), String> {
    let credential = configured_replication_credential(runtime_config)?;
    let client = connect_capnp([target.host.as_str(), &target.port.to_string()].join(":"))
        .await
        .map_err(|error| error.to_string())?;
    let leader_info = fetch_replication_info(&client, &credential).await?;
    let local_server_id = {
        let guard = status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.server_id.clone()
    };
    if leader_info.server_id == local_server_id {
        return Err("cannot follow self".to_owned());
    }
    if !leader_info.leader || !leader_info.following.is_empty() {
        return Err("cannot follow a follower".to_owned());
    }
    Ok(())
}

async fn follow_once<S>(
    executor: NativeExecutor<S>,
    runtime_config: SharedRuntimeConfig,
    status: SharedReplicationStatus,
    target: &FollowTarget,
) -> Result<(), String>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let credential = configured_replication_credential(&runtime_config)?;
    let client = connect_capnp([target.host.as_str(), &target.port.to_string()].join(":"))
        .await
        .map_err(|error| error.to_string())?;

    let leader_info = fetch_replication_info(&client, &credential).await?;
    let local_server_id = {
        let guard = status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.server_id.clone()
    };
    if leader_info.server_id == local_server_id {
        return Err("cannot follow self".to_owned());
    }
    if !leader_info.leader || !leader_info.following.is_empty() {
        return Err("cannot follow a follower".to_owned());
    }

    {
        let mut guard = status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.leader_id = Some(leader_info.server_id.clone());
        guard.caught_up = false;
        guard.caught_up_once = false;
        guard.leader_last_sequence = leader_info.last_sequence;
        guard.last_error = None;
    }

    let mut local_last_sequence = executor
        .execute(|db| db.last_sequence())
        .await
        .map_err(|error| error.to_string())?;
    {
        let mut guard = status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.local_last_sequence = local_last_sequence;
        guard.leader_last_sequence = leader_info.last_sequence;
    }

    if local_last_sequence > 0 {
        let local_checksum = executor
            .execute(move |db| db.checksum_range(1, local_last_sequence))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let remote_checksum =
            fetch_replication_checksum(&client, &credential, 1, local_last_sequence).await?;
        if local_checksum != remote_checksum {
            {
                let mut guard = status
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.checksum_mismatches_total = guard.checksum_mismatches_total.saturating_add(1);
                guard.resyncs_total = guard.resyncs_total.saturating_add(1);
                guard.caught_up = false;
            }
            executor
                .execute(|db| db.reset_replication_state())
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            local_last_sequence = 0;
            let mut guard = status
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.local_last_sequence = 0;
        }
    }

    if local_last_sequence >= leader_info.last_sequence {
        let mut guard = status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.caught_up = true;
        guard.caught_up_once = true;
        guard.local_last_sequence = local_last_sequence;
        guard.leader_last_sequence = leader_info.last_sequence;
    }

    let stream = open_replication_stream(
        &client,
        &credential,
        local_last_sequence,
        replication_batch_size(&runtime_config),
    )
    .await?;
    follow_stream(
        executor,
        status,
        stream,
        local_last_sequence,
        leader_info.server_id,
    )
    .await
}

async fn follow_stream<S>(
    executor: NativeExecutor<S>,
    status: SharedReplicationStatus,
    stream: replication_stream::Client,
    mut cursor: u64,
    leader_id: String,
) -> Result<(), String>
where
    S: StorageBackend + Send + Sync + 'static,
{
    loop {
        let response = stream
            .next_request()
            .send()
            .promise
            .await
            .map_err(|error| error.to_string())?;
        let response = response.get().map_err(|error| error.to_string())?;
        let leader_last_sequence = response.get_leader_last_sequence();
        let entries_reader = response.get_entries().map_err(|error| error.to_string())?;
        let mut entries = Vec::with_capacity(entries_reader.len() as usize);
        for payload in entries_reader.iter() {
            let bytes = payload.map_err(|error| error.to_string())?;
            let entry =
                bincode::deserialize::<StorageEntry>(bytes).map_err(|error| error.to_string())?;
            entries.push(entry);
        }

        if !entries.is_empty() {
            cursor = executor
                .execute(move |db| db.apply_replicated_entries(&entries))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        }

        let mut guard = status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.leader_id.as_deref() != Some(leader_id.as_str()) {
            return Err("replication leader changed unexpectedly".to_owned());
        }
        guard.caught_up = cursor >= leader_last_sequence;
        if guard.caught_up {
            guard.caught_up_once = true;
        }
        guard.local_last_sequence = cursor;
        guard.leader_last_sequence = leader_last_sequence;
        guard.last_error = None;
    }
}

#[derive(Debug, Clone)]
struct RemoteReplicationInfo {
    server_id: String,
    following: String,
    leader: bool,
    last_sequence: u64,
}

async fn fetch_replication_info(
    client: &lat_lng::Client,
    credential: &str,
) -> Result<RemoteReplicationInfo, String> {
    let mut request = client.replication_info_request();
    request.get().set_credential(credential);
    let response = request
        .send()
        .promise
        .await
        .map_err(|error| error.to_string())?;
    let response = response.get().map_err(|error| error.to_string())?;
    if !response.get_ok() {
        return Err(read_text(response.get_error())?);
    }
    let info = response.get_info().map_err(|error| error.to_string())?;
    Ok(RemoteReplicationInfo {
        server_id: read_text(info.get_server_id())?,
        following: read_text(info.get_following())?,
        leader: info.get_leader(),
        last_sequence: info.get_last_sequence(),
    })
}

async fn fetch_replication_checksum(
    client: &lat_lng::Client,
    credential: &str,
    from: u64,
    to: u64,
) -> Result<[u8; 16], String> {
    let mut request = client.replication_checksum_request();
    {
        let mut params = request.get();
        params.set_credential(credential);
        params.set_from(from);
        params.set_to(to);
    }
    let response = request
        .send()
        .promise
        .await
        .map_err(|error| error.to_string())?;
    let response = response.get().map_err(|error| error.to_string())?;
    if !response.get_ok() {
        return Err(read_text(response.get_error())?);
    }
    let data = response.get_checksum().map_err(|error| error.to_string())?;
    if data.len() != 16 {
        return Err("replication checksum must contain exactly 16 bytes".to_owned());
    }
    let mut checksum = [0_u8; 16];
    checksum.copy_from_slice(data);
    Ok(checksum)
}

async fn open_replication_stream(
    client: &lat_lng::Client,
    credential: &str,
    after_sequence: u64,
    batch_size: usize,
) -> Result<replication_stream::Client, String> {
    let mut request = client.replication_stream_request();
    {
        let mut params = request.get();
        params.set_credential(credential);
        params.set_after_sequence(after_sequence);
        params.set_batch_size(batch_size as u32);
    }
    let response = request
        .send()
        .promise
        .await
        .map_err(|error| error.to_string())?;
    let response = response.get().map_err(|error| error.to_string())?;
    if !response.get_ok() {
        return Err(read_text(response.get_error())?);
    }
    response.get_stream().map_err(|error| error.to_string())
}

async fn connect_capnp(addr: String) -> Result<lat_lng::Client, std::io::Error> {
    let stream = TcpStream::connect(addr).await?;
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
        if let Err(error) = rpc_system.await {
            error!(error = %error, "replication rpc client stopped");
        }
    });
    Ok(client)
}

fn read_text(value: capnp::Result<capnp::text::Reader<'_>>) -> Result<String, String> {
    value
        .map_err(|error| error.to_string())?
        .to_str()
        .map(|value| value.to_owned())
        .map_err(|error| error.to_string())
}

fn configured_follow_target(runtime_config: &SharedRuntimeConfig) -> Option<FollowTarget> {
    match runtime_config.read() {
        Ok(guard) => match (&guard.follow_host, guard.follow_port) {
            (Some(host), Some(port)) if !host.trim().is_empty() && port > 0 => Some(FollowTarget {
                host: host.clone(),
                port,
            }),
            _ => None,
        },
        Err(poisoned) => {
            let guard = poisoned.into_inner();
            match (&guard.follow_host, guard.follow_port) {
                (Some(host), Some(port)) if !host.trim().is_empty() && port > 0 => {
                    Some(FollowTarget {
                        host: host.clone(),
                        port,
                    })
                }
                _ => None,
            }
        }
    }
}

fn configured_replication_credential(
    runtime_config: &SharedRuntimeConfig,
) -> Result<String, String> {
    match runtime_config.read() {
        Ok(guard) => guard
            .replication_credential
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "replication credential is not configured".to_owned()),
        Err(poisoned) => poisoned
            .into_inner()
            .replication_credential
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "replication credential is not configured".to_owned()),
    }
}

fn replication_batch_size(runtime_config: &SharedRuntimeConfig) -> usize {
    match runtime_config.read() {
        Ok(guard) => guard.replication_batch_size.max(1),
        Err(poisoned) => poisoned.into_inner().replication_batch_size.max(1),
    }
}

async fn sync_effective_read_only<S>(
    executor: &NativeExecutor<S>,
    runtime_config: &SharedRuntimeConfig,
    status: &SharedReplicationStatus,
) -> Result<(), String>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let configured_read_only = match runtime_config.read() {
        Ok(guard) => guard.read_only,
        Err(poisoned) => poisoned.into_inner().read_only,
    };
    let effective = {
        let guard = status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.effective_read_only(configured_read_only)
    };
    executor
        .execute(move |db| match db.config().write() {
            Ok(mut guard) => guard.read_only = effective,
            Err(poisoned) => poisoned.into_inner().read_only = effective,
        })
        .await
        .map_err(|error| error.to_string())
}
