#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use latlng_storage::{
    CompactionResult, StorageBackend, StorageEntry, StorageError, StorageResult,
    assert_strictly_increasing, replay_entries,
};
use rusqlite::{Connection, params};

pub use latlng_storage as storage;

#[derive(Debug)]
pub struct SqliteBackend {
    _path: PathBuf,
    connection: Mutex<Connection>,
}

impl SqliteBackend {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path).map_err(sql_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE IF NOT EXISTS entries (
                   sequence INTEGER PRIMARY KEY,
                   timestamp_ns INTEGER NOT NULL,
                   command BLOB NOT NULL
                 );",
            )
            .map_err(sql_error)?;
        Ok(Self {
            _path: path,
            connection: Mutex::new(connection),
        })
    }
}

impl StorageBackend for SqliteBackend {
    fn append(&self, entry: &StorageEntry) -> StorageResult<()> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        connection
            .execute(
                "INSERT INTO entries(sequence, timestamp_ns, command) VALUES (?1, ?2, ?3)",
                params![entry.sequence, entry.timestamp_ns, entry.command.as_ref()],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        assert_strictly_increasing(entries)?;
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let transaction = connection.transaction().map_err(sql_error)?;
        let last_sequence = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM entries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map_err(sql_error)?;
        if let Some(first) = entries.first()
            && last_sequence > 0
            && first.sequence <= last_sequence
        {
            return Err(StorageError::SequenceRegression {
                expected: last_sequence + 1,
                actual: first.sequence,
            });
        }
        {
            let mut insert = transaction
                .prepare("INSERT INTO entries(sequence, timestamp_ns, command) VALUES (?1, ?2, ?3)")
                .map_err(sql_error)?;
            for entry in entries {
                insert
                    .execute(params![
                        entry.sequence,
                        entry.timestamp_ns,
                        entry.command.as_ref()
                    ])
                    .map_err(sql_error)?;
            }
        }
        transaction.commit().map_err(sql_error)?;
        Ok(())
    }

    fn replay(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
    ) -> StorageResult<u64> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut statement = connection
            .prepare(
                "SELECT sequence, timestamp_ns, command
                 FROM entries
                 WHERE sequence > ?1
                 ORDER BY sequence ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![after_seq], |row| {
                Ok(StorageEntry {
                    sequence: row.get::<_, u64>(0)?,
                    timestamp_ns: row.get::<_, u64>(1)?,
                    command: row.get::<_, Vec<u8>>(2)?.into(),
                })
            })
            .map_err(sql_error)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(sql_error)?);
        }
        replay_entries(&entries, after_seq, callback)
    }

    fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let transaction = connection.transaction().map_err(sql_error)?;
        transaction
            .execute("DELETE FROM entries", [])
            .map_err(sql_error)?;
        {
            let mut insert = transaction
                .prepare("INSERT INTO entries(sequence, timestamp_ns, command) VALUES (?1, ?2, ?3)")
                .map_err(sql_error)?;
            for entry in &entries {
                insert
                    .execute(params![
                        entry.sequence,
                        entry.timestamp_ns,
                        entry.command.as_ref()
                    ])
                    .map_err(sql_error)?;
            }
        }
        transaction.commit().map_err(sql_error)?;
        Ok(())
    }

    fn compact(&self) -> StorageResult<CompactionResult> {
        let before_entries = self.last_sequence()?;
        let before_bytes = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(command)), 0) FROM entries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map_err(sql_error)?;
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .execute_batch("VACUUM;")
            .map_err(sql_error)?;
        Ok(CompactionResult {
            before_entries,
            after_entries: before_entries,
            before_bytes,
            after_bytes: before_bytes,
        })
    }

    fn last_sequence(&self) -> StorageResult<u64> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM entries",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map_err(sql_error)
    }

    fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut statement = connection
            .prepare(
                "SELECT sequence, timestamp_ns, command
                 FROM entries
                 WHERE sequence >= ?1 AND sequence <= ?2
                 ORDER BY sequence ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![from, to], |row| {
                Ok(StorageEntry {
                    sequence: row.get::<_, u64>(0)?,
                    timestamp_ns: row.get::<_, u64>(1)?,
                    command: row.get::<_, Vec<u8>>(2)?.into(),
                })
            })
            .map_err(sql_error)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(sql_error)?);
        }
        Ok(StorageEntry::checksum(&entries))
    }

    fn close(&self) -> StorageResult<()> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .execute_batch("PRAGMA optimize;")
            .map_err(sql_error)?;
        Ok(())
    }
}

fn sql_error(error: rusqlite::Error) -> StorageError {
    StorageError::Message(error.to_string())
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use latlng_storage::{StorageBackend, StorageEntry};
    use tempfile::tempdir;

    use super::SqliteBackend;

    #[test]
    fn sqlite_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("latlng.sqlite");
        let backend = SqliteBackend::open(&path).unwrap();
        backend
            .append(&StorageEntry {
                sequence: 1,
                timestamp_ns: 1,
                command: Bytes::from_static(b"abc"),
            })
            .unwrap();
        backend.close().unwrap();

        let reopened = SqliteBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 1);
    }

    #[test]
    fn sqlite_append_batch_rolls_back_on_mid_batch_failure() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("latlng.sqlite");
        let backend = SqliteBackend::open(&path).unwrap();
        backend
            .append(&StorageEntry {
                sequence: 1,
                timestamp_ns: 1,
                command: Bytes::from_static(b"one"),
            })
            .unwrap();

        backend
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_batch_on_three
                 BEFORE INSERT ON entries
                 WHEN NEW.sequence = 3
                 BEGIN
                   SELECT RAISE(ABORT, 'injected batch failure');
                 END;",
            )
            .unwrap();

        let error = backend.append_batch(&[
            StorageEntry {
                sequence: 2,
                timestamp_ns: 2,
                command: Bytes::from_static(b"two"),
            },
            StorageEntry {
                sequence: 3,
                timestamp_ns: 3,
                command: Bytes::from_static(b"three"),
            },
        ]);
        assert!(error.is_err());

        let mut seen = Vec::new();
        backend
            .replay(0, &mut |entry| {
                seen.push(entry.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, vec![1]);
    }
}
