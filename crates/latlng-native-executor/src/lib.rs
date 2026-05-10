#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, unbounded};
use latlng_core::{LatLngNative, storage::StorageBackend};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

pub const DEFAULT_QUEUE_MULTIPLIER: usize = 64;

#[derive(Debug, Error)]
pub enum NativeExecutorError {
    #[error("failed to spawn native executor worker: {0}")]
    Spawn(String),
    #[error("native executor is shut down")]
    Closed,
    #[error("native executor task panicked")]
    Panicked,
}

trait ExecutorJob<S: StorageBackend>: Send {
    fn run(self: Box<Self>, db: &LatLngNative<S>);
}

impl<S, F> ExecutorJob<S> for F
where
    S: StorageBackend,
    F: FnOnce(&LatLngNative<S>) + Send + 'static,
{
    fn run(self: Box<Self>, db: &LatLngNative<S>) {
        (*self)(db);
    }
}

struct ExecutorInner<S: StorageBackend> {
    sender: Mutex<Option<Sender<JobEnvelope<S>>>>,
    queue_permits: Arc<Semaphore>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl<S> Drop for ExecutorInner<S>
where
    S: StorageBackend,
{
    fn drop(&mut self) {
        self.sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while let Some(worker) = workers.pop() {
            let _ = worker.join();
        }
    }
}

pub struct NativeExecutor<S: StorageBackend> {
    inner: Arc<ExecutorInner<S>>,
}

struct JobEnvelope<S: StorageBackend> {
    job: Box<dyn ExecutorJob<S>>,
    queue_permit: OwnedSemaphorePermit,
}

impl<S> Clone for NativeExecutor<S>
where
    S: StorageBackend,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> NativeExecutor<S>
where
    S: StorageBackend + Send + Sync + 'static,
{
    pub fn with_defaults(db: Arc<LatLngNative<S>>) -> Result<Self, NativeExecutorError> {
        let threads = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4)
            .max(1);
        let queue_limit = threads.saturating_mul(DEFAULT_QUEUE_MULTIPLIER).max(1);
        Self::new(db, threads, queue_limit)
    }

    pub fn new(
        db: Arc<LatLngNative<S>>,
        threads: usize,
        queue_limit: usize,
    ) -> Result<Self, NativeExecutorError> {
        let threads = threads.max(1);
        let queue_limit = queue_limit.max(1);
        let (sender, receiver) = unbounded::<JobEnvelope<S>>();
        let queue_permits = Arc::new(Semaphore::new(queue_limit));
        let mut workers = Vec::with_capacity(threads);

        for index in 0..threads {
            let worker_db = Arc::clone(&db);
            let worker_rx = receiver.clone();
            let name = format!("latlng-core-{index}");
            let handle = thread::Builder::new()
                .name(name)
                .spawn(move || {
                    while let Ok(envelope) = worker_rx.recv() {
                        drop(envelope.queue_permit);
                        envelope.job.run(worker_db.as_ref());
                    }
                })
                .map_err(|error| NativeExecutorError::Spawn(error.to_string()))?;
            workers.push(handle);
        }

        Ok(Self {
            inner: Arc::new(ExecutorInner {
                sender: Mutex::new(Some(sender)),
                queue_permits,
                workers: Mutex::new(workers),
            }),
        })
    }

    pub async fn execute<T, F>(&self, op: F) -> Result<T, NativeExecutorError>
    where
        T: Send + 'static,
        F: FnOnce(&LatLngNative<S>) -> T + Send + 'static,
    {
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
            .ok_or(NativeExecutorError::Closed)?;
        let queue_permit = Arc::clone(&self.inner.queue_permits)
            .acquire_owned()
            .await
            .map_err(|_| NativeExecutorError::Closed)?;

        let (result_tx, result_rx) = oneshot::channel();
        let job = Box::new(move |db: &LatLngNative<S>| {
            let result = catch_unwind(AssertUnwindSafe(|| op(db)))
                .map_err(|_| NativeExecutorError::Panicked);
            let _ = result_tx.send(result);
        }) as Box<dyn ExecutorJob<S>>;

        sender
            .send(JobEnvelope { job, queue_permit })
            .map_err(|_| NativeExecutorError::Closed)?;

        result_rx.await.map_err(|_| NativeExecutorError::Closed)?
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use latlng_core::{LatLng, LatLngNative, SetCondition, SetRequest, geo::GeoType};
    use latlng_storage_memory::MemoryBackend;
    use tokio::task::JoinSet;
    use tokio::time::{sleep, timeout};

    use super::{DEFAULT_QUEUE_MULTIPLIER, NativeExecutor, NativeExecutorError};

    #[tokio::test]
    async fn executor_runs_core_calls() {
        let db: LatLngNative<MemoryBackend> = LatLng::builder()
            .storage(MemoryBackend::new())
            .build()
            .unwrap();
        let db = Arc::new(db);
        let executor = NativeExecutor::with_defaults(Arc::clone(&db)).unwrap();

        let stored = executor
            .execute(move |db| {
                db.set(SetRequest {
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: Vec::new(),
                    expire_seconds: None,
                    condition: SetCondition::Always,
                })
            })
            .await
            .unwrap()
            .unwrap();
        assert!(stored);
    }

    #[tokio::test]
    async fn executor_applies_backpressure_when_queue_is_full() {
        let db: LatLngNative<MemoryBackend> = LatLng::builder()
            .storage(MemoryBackend::new())
            .build()
            .unwrap();
        let db = Arc::new(db);
        let executor = NativeExecutor::new(Arc::clone(&db), 1, 1).unwrap();

        let first: tokio::task::JoinHandle<Result<u8, NativeExecutorError>> = {
            let executor = executor.clone();
            tokio::spawn(async move {
                executor
                    .execute(|_| {
                        std::thread::sleep(Duration::from_millis(150));
                        1_u8
                    })
                    .await
            })
        };
        let second: tokio::task::JoinHandle<Result<u8, NativeExecutorError>> = {
            let executor = executor.clone();
            tokio::spawn(async move {
                executor
                    .execute(|_| {
                        std::thread::sleep(Duration::from_millis(150));
                        2_u8
                    })
                    .await
            })
        };

        sleep(Duration::from_millis(20)).await;
        let third = executor.execute(|_| 3_u8);
        assert!(timeout(Duration::from_millis(60), third).await.is_err());

        assert_eq!(first.await.unwrap().unwrap(), 1);
        assert_eq!(second.await.unwrap().unwrap(), 2);
        assert_eq!(executor.execute(|_| 3_u8).await.unwrap(), 3);
    }

    #[test]
    fn defaults_produce_non_zero_capacity() {
        let threads = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4)
            .max(1);
        let queue = threads.saturating_mul(DEFAULT_QUEUE_MULTIPLIER).max(1);
        assert!(threads >= 1);
        assert!(queue >= 1);
    }

    #[tokio::test]
    async fn executor_reports_panics() {
        let db: LatLngNative<MemoryBackend> = LatLng::builder()
            .storage(MemoryBackend::new())
            .build()
            .unwrap();
        let executor = NativeExecutor::new(Arc::new(db), 1, 1).unwrap();
        let error = executor
            .execute::<(), _>(|_| panic!("boom"))
            .await
            .unwrap_err();
        assert!(matches!(error, NativeExecutorError::Panicked));
    }

    #[tokio::test]
    async fn executor_handles_burst_submission_without_deadlock() {
        let db: LatLngNative<MemoryBackend> = LatLng::builder()
            .storage(MemoryBackend::new())
            .build()
            .unwrap();
        let executor = NativeExecutor::new(Arc::new(db), 2, 4).unwrap();
        let mut tasks = JoinSet::new();

        for value in 0_u16..64 {
            let executor = executor.clone();
            tasks.spawn(async move {
                executor
                    .execute(move |_| {
                        std::thread::sleep(Duration::from_millis(5));
                        value
                    })
                    .await
            });
        }

        let mut seen = Vec::new();
        timeout(Duration::from_secs(5), async {
            while let Some(result) = tasks.join_next().await {
                seen.push(
                    result
                        .expect("task join should succeed")
                        .expect("executor should succeed"),
                );
            }
        })
        .await
        .expect("burst submission should complete without deadlock");

        seen.sort_unstable();
        assert_eq!(seen.len(), 64);
        assert_eq!(seen.first().copied(), Some(0));
        assert_eq!(seen.last().copied(), Some(63));
    }

    #[test]
    fn executor_drop_drains_queued_work_without_deadlock() {
        let thread = std::thread::spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let db: LatLngNative<MemoryBackend> = LatLng::builder()
                    .storage(MemoryBackend::new())
                    .build()
                    .unwrap();
                let executor = NativeExecutor::new(Arc::new(db), 1, 1).unwrap();

                let first_executor = executor.clone();
                let second_executor = executor.clone();
                let first = tokio::spawn(async move {
                    first_executor
                        .execute(|_| {
                            std::thread::sleep(Duration::from_millis(75));
                            1_u8
                        })
                        .await
                });
                let second = tokio::spawn(async move { second_executor.execute(|_| 2_u8).await });

                drop(executor);

                assert_eq!(first.await.unwrap().unwrap(), 1);
                assert_eq!(second.await.unwrap().unwrap(), 2);
            });
        });

        thread.join().expect("executor drop should not deadlock");
    }
}
