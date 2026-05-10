#![forbid(unsafe_code)]

use latlng_config::{RuntimeConfig, StorageMode as RuntimeStorageMode};
use latlng_core::storage::{CompactionResult, StorageBackend, StorageEntry, StorageResult};
use latlng_storage_aof::{AofBackend, AofOptions};
use latlng_storage_memory::MemoryBackend;

#[derive(Debug)]
pub enum ServerStorage {
    Memory(MemoryBackend),
    Aof(AofBackend),
}

impl ServerStorage {
    pub fn open(config: &RuntimeConfig) -> StorageResult<Self> {
        match &config.storage {
            RuntimeStorageMode::Memory => Ok(Self::Memory(MemoryBackend::new())),
            RuntimeStorageMode::Aof { path } => Ok(Self::Aof(AofBackend::open_with_options(
                path,
                AofOptions {
                    writer_queue_limit: config.aof_writer_queue_limit,
                    max_group_commit_delay_ms: config.aof_group_commit_delay_ms,
                    max_requests_per_commit_cycle: config.aof_group_commit_max_requests,
                },
            )?)),
        }
    }
}

impl StorageBackend for ServerStorage {
    fn stores_command_log(&self) -> bool {
        match self {
            Self::Memory(backend) => backend.stores_command_log(),
            Self::Aof(backend) => backend.stores_command_log(),
        }
    }

    fn append(&self, entry: &StorageEntry) -> StorageResult<()> {
        match self {
            Self::Memory(backend) => backend.append(entry),
            Self::Aof(backend) => backend.append(entry),
        }
    }

    fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()> {
        match self {
            Self::Memory(backend) => backend.append_batch(entries),
            Self::Aof(backend) => backend.append_batch(entries),
        }
    }

    fn replay(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
    ) -> StorageResult<u64> {
        match self {
            Self::Memory(backend) => backend.replay(after_seq, callback),
            Self::Aof(backend) => backend.replay(after_seq, callback),
        }
    }

    fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()> {
        match self {
            Self::Memory(backend) => backend.snapshot(entries),
            Self::Aof(backend) => backend.snapshot(entries),
        }
    }

    fn compact(&self) -> StorageResult<CompactionResult> {
        match self {
            Self::Memory(backend) => backend.compact(),
            Self::Aof(backend) => backend.compact(),
        }
    }

    fn last_sequence(&self) -> StorageResult<u64> {
        match self {
            Self::Memory(backend) => backend.last_sequence(),
            Self::Aof(backend) => backend.last_sequence(),
        }
    }

    fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]> {
        match self {
            Self::Memory(backend) => backend.checksum(from, to),
            Self::Aof(backend) => backend.checksum(from, to),
        }
    }

    fn close(&self) -> StorageResult<()> {
        match self {
            Self::Memory(backend) => backend.close(),
            Self::Aof(backend) => backend.close(),
        }
    }
}
