#![forbid(unsafe_code)]

use latlng_storage::{
    CompactionResult, StorageBackend, StorageEntry, StorageError, StorageResult,
    assert_strictly_increasing, replay_entries,
};
use spin::RwLock;

#[derive(Debug, Default)]
pub struct MemoryBackend {
    inner: RwLock<MemoryState>,
    record_log: bool,
}

#[derive(Debug, Default)]
struct MemoryState {
    entries: Vec<StorageEntry>,
    closed: bool,
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recording() -> Self {
        Self {
            inner: RwLock::new(MemoryState::default()),
            record_log: true,
        }
    }

    pub fn entries(&self) -> Vec<StorageEntry> {
        self.inner.read().entries.clone()
    }

    fn ensure_open(&self) -> StorageResult<()> {
        if self.inner.read().closed {
            return Err(StorageError::Closed);
        }
        Ok(())
    }
}

impl StorageBackend for MemoryBackend {
    fn stores_command_log(&self) -> bool {
        self.record_log
    }

    fn append(&self, entry: &StorageEntry) -> StorageResult<()> {
        self.ensure_open()?;
        let mut inner = self.inner.write();
        if let Some(last) = inner.entries.last()
            && entry.sequence <= last.sequence
        {
            return Err(StorageError::SequenceRegression {
                expected: last.sequence + 1,
                actual: entry.sequence,
            });
        }
        inner.entries.push(entry.clone());
        Ok(())
    }

    fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()> {
        self.ensure_open()?;
        assert_strictly_increasing(entries)?;
        let mut inner = self.inner.write();
        if let Some(last) = inner.entries.last()
            && let Some(first) = entries.first()
            && first.sequence <= last.sequence
        {
            return Err(StorageError::SequenceRegression {
                expected: last.sequence + 1,
                actual: first.sequence,
            });
        }
        inner.entries.extend(entries.iter().cloned());
        Ok(())
    }

    fn replay(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
    ) -> StorageResult<u64> {
        let entries = self.inner.read().entries.clone();
        replay_entries(&entries, after_seq, callback)
    }

    fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()> {
        self.ensure_open()?;
        assert_strictly_increasing(&entries)?;
        self.inner.write().entries = entries;
        Ok(())
    }

    fn compact(&self) -> StorageResult<CompactionResult> {
        let inner = self.inner.read();
        let before_bytes = inner
            .entries
            .iter()
            .map(|entry| entry.command.len() as u64)
            .sum::<u64>();
        Ok(CompactionResult {
            before_entries: inner.entries.len() as u64,
            after_entries: inner.entries.len() as u64,
            before_bytes,
            after_bytes: before_bytes,
        })
    }

    fn last_sequence(&self) -> StorageResult<u64> {
        Ok(self
            .inner
            .read()
            .entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(0))
    }

    fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]> {
        let entries = self
            .inner
            .read()
            .entries
            .iter()
            .filter(|entry| entry.sequence >= from && entry.sequence <= to)
            .cloned()
            .collect::<Vec<_>>();
        Ok(StorageEntry::checksum(&entries))
    }

    fn close(&self) -> StorageResult<()> {
        self.inner.write().closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use latlng_storage::{StorageBackend, StorageEntry};

    use super::MemoryBackend;

    #[test]
    fn memory_backend_roundtrip() {
        let backend = MemoryBackend::new();
        backend
            .append(&StorageEntry {
                sequence: 1,
                timestamp_ns: 1,
                command: Bytes::from_static(b"one"),
            })
            .unwrap();

        let mut seen = Vec::new();
        backend
            .replay(0, &mut |entry| {
                seen.push(entry.sequence);
                Ok(())
            })
            .unwrap();

        assert_eq!(seen, vec![1]);
        assert_eq!(backend.last_sequence().unwrap(), 1);
    }

    #[test]
    fn memory_backend_defaults_to_volatile_log_mode() {
        assert!(!MemoryBackend::new().stores_command_log());
        assert!(MemoryBackend::recording().stores_command_log());
    }
}
