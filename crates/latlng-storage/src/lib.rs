#![forbid(unsafe_code)]

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, Clone, Error)]
pub enum StorageError {
    #[error("io error: {0}")]
    Io(String),
    #[error("codec error: {0}")]
    Codec(String),
    #[error("sequence regression: expected at least {expected}, got {actual}")]
    SequenceRegression { expected: u64, actual: u64 },
    #[error("backend is closed")]
    Closed,
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Message(String),
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionResult {
    pub before_entries: u64,
    pub after_entries: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageEntry {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub command: Bytes,
}

impl StorageEntry {
    pub fn checksum(entries: &[StorageEntry]) -> [u8; 16] {
        let mut context = md5::Context::new();
        for entry in entries {
            context.consume(entry.sequence.to_le_bytes());
            context.consume(entry.timestamp_ns.to_le_bytes());
            context.consume(&entry.command);
        }
        context.finalize().0
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub trait StorageBackend: Send + Sync + 'static {
    fn stores_command_log(&self) -> bool {
        true
    }

    fn append(&self, entry: &StorageEntry) -> StorageResult<()>;
    /// Appends a logical batch atomically.
    ///
    /// On success, every entry in `entries` must be durable and replayable.
    /// On error, none of the batch may become replayable.
    /// Empty batches must be treated as a no-op.
    fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()>;
    fn replay(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
    ) -> StorageResult<u64>;
    fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()>;
    fn compact(&self) -> StorageResult<CompactionResult>;
    fn last_sequence(&self) -> StorageResult<u64>;
    fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]>;
    fn close(&self) -> StorageResult<()>;
}

#[cfg(target_arch = "wasm32")]
pub trait StorageBackend: 'static {
    fn stores_command_log(&self) -> bool {
        true
    }

    fn append(&self, entry: &StorageEntry) -> StorageResult<()>;
    /// Appends a logical batch atomically.
    ///
    /// On success, every entry in `entries` must be durable and replayable.
    /// On error, none of the batch may become replayable.
    /// Empty batches must be treated as a no-op.
    fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()>;
    fn replay(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
    ) -> StorageResult<u64>;
    fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()>;
    fn compact(&self) -> StorageResult<CompactionResult>;
    fn last_sequence(&self) -> StorageResult<u64>;
    fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]>;
    fn close(&self) -> StorageResult<()>;
}

pub fn assert_strictly_increasing(entries: &[StorageEntry]) -> StorageResult<()> {
    let mut last = None;
    for entry in entries {
        if let Some(previous) = last
            && entry.sequence <= previous
        {
            return Err(StorageError::SequenceRegression {
                expected: previous + 1,
                actual: entry.sequence,
            });
        }
        last = Some(entry.sequence);
    }
    Ok(())
}

pub fn replay_entries(
    entries: &[StorageEntry],
    after_seq: u64,
    callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
) -> StorageResult<u64> {
    let mut last = after_seq;
    for entry in entries.iter().filter(|entry| entry.sequence > after_seq) {
        callback(entry.clone())?;
        last = entry.sequence;
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{StorageEntry, assert_strictly_increasing};

    #[test]
    fn checksum_is_stable() {
        let entries = vec![StorageEntry {
            sequence: 1,
            timestamp_ns: 5,
            command: Bytes::from_static(b"abc"),
        }];
        assert_eq!(
            StorageEntry::checksum(&entries),
            StorageEntry::checksum(&entries)
        );
    }

    #[test]
    fn sequence_validation_catches_regression() {
        let entries = vec![
            StorageEntry {
                sequence: 1,
                timestamp_ns: 1,
                command: Bytes::from_static(b"a"),
            },
            StorageEntry {
                sequence: 1,
                timestamp_ns: 2,
                command: Bytes::from_static(b"b"),
            },
        ];
        assert!(assert_strictly_increasing(&entries).is_err());
    }
}
