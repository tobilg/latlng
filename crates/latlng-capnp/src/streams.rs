use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use latlng_auth::{AuthAction, AuthPrincipal};
use latlng_core::geofence::GeofenceEvent;
use latlng_core::platform::NativePlatform;
use latlng_core::storage::StorageBackend;
use latlng_native_executor::NativeExecutor;
use tokio::sync::{Mutex, Notify, mpsc};

use crate::codec::{capnp_failed, fill_geofence_event};
use crate::{geofence_stream, replication_stream};

type NativeGeofenceReceiver = latlng_core::geofence::GeofenceEventReceiver<NativePlatform>;

pub(crate) struct GeofenceStreamRpc {
    receiver: Mutex<mpsc::UnboundedReceiver<GeofenceEvent>>,
    cancel: Arc<AtomicBool>,
    wake: latlng_core::platform::NativeWakeHandle<GeofenceEvent>,
}

impl GeofenceStreamRpc {
    pub(crate) fn new(receiver: NativeGeofenceReceiver, principal: AuthPrincipal) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = receiver.wake_handle();
        let worker_cancel = Arc::clone(&cancel);
        let mut receiver = receiver;
        tokio::task::spawn_blocking(move || {
            while let Some(event) = receiver.recv_blocking_with_cancel(worker_cancel.as_ref()) {
                if !principal.allows(AuthAction::SubscriptionsRead, &event.collection) {
                    continue;
                }
                if events_tx.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            receiver: Mutex::new(events_rx),
            cancel,
            wake,
        }
    }
}

impl Drop for GeofenceStreamRpc {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.wake.wake();
    }
}

impl geofence_stream::Server for GeofenceStreamRpc {
    async fn next(
        self: capnp::capability::Rc<Self>,
        _: geofence_stream::NextParams,
        mut results: geofence_stream::NextResults,
    ) -> Result<(), capnp::Error> {
        let event = {
            let mut receiver = self.receiver.lock().await;
            receiver.recv().await
        };

        let mut out = results.get();
        match event {
            Some(event) => {
                fill_geofence_event(out.reborrow().init_event(), &event)?;
                out.set_done(false);
            }
            None => {
                out.set_done(true);
            }
        }
        Ok(())
    }
}

pub(crate) struct ReplicationStreamRpc<S: StorageBackend> {
    executor: NativeExecutor<S>,
    after_sequence: Mutex<u64>,
    batch_size: usize,
    notify: Arc<Notify>,
}

impl<S> ReplicationStreamRpc<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    pub(crate) fn new(
        executor: NativeExecutor<S>,
        after_sequence: u64,
        batch_size: usize,
        notify: Arc<Notify>,
    ) -> Self {
        Self {
            executor,
            after_sequence: Mutex::new(after_sequence),
            batch_size: batch_size.max(1),
            notify,
        }
    }
}

impl<S> replication_stream::Server for ReplicationStreamRpc<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    async fn next(
        self: capnp::capability::Rc<Self>,
        _: replication_stream::NextParams,
        mut results: replication_stream::NextResults,
    ) -> Result<(), capnp::Error> {
        loop {
            let after_sequence = *self.after_sequence.lock().await;
            let batch_size = self.batch_size;
            let (entries, leader_last_sequence) = self
                .executor
                .execute(move |db| {
                    let entries = db.storage_entries_after(after_sequence, batch_size)?;
                    Ok::<_, latlng_core::CoreError>((entries, db.last_sequence()))
                })
                .await
                .map_err(|error| capnp_failed(error.to_string()))?
                .map_err(|error| capnp_failed(error.to_string()))?;

            if !entries.is_empty() {
                let next_after_sequence = entries
                    .last()
                    .map(|entry| entry.sequence)
                    .unwrap_or(after_sequence);
                *self.after_sequence.lock().await = next_after_sequence;
                let mut out = results.get();
                let mut encoded = out.reborrow().init_entries(entries.len() as u32);
                for (index, entry) in entries.iter().enumerate() {
                    let payload = bincode::serialize(entry).map_err(capnp_failed)?;
                    encoded.set(index as u32, &payload);
                }
                out.set_leader_last_sequence(leader_last_sequence);
                return Ok(());
            }

            self.notify.notified().await;
        }
    }
}
