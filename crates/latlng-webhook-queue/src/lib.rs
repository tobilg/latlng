#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use latlng_core::{
    LogRecord, WebhookAckRecord, WebhookDeadLetterRecord, WebhookEnqueueRecord,
    WebhookRetryScheduledRecord,
};
use latlng_geofence::GeofenceEvent;
use rusqlite::{Connection, params};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("sqlite error: {0}")]
    Sql(String),
    #[error("codec error: {0}")]
    Codec(String),
}

pub type QueueResult<T> = Result<T, QueueError>;

#[derive(Debug, Clone, PartialEq)]
pub struct QueueJob {
    pub job_id: String,
    pub event_id: String,
    pub hook_name: String,
    pub endpoint: String,
    pub event: GeofenceEvent,
    pub attempts_used: u32,
    pub max_attempts: u32,
    pub next_attempt_at_ms: u64,
    pub last_error: Option<String>,
    pub created_from_sequence: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub pending: u64,
    pub leased: u64,
    pub dead_letter: u64,
    pub oldest_pending_age_ms: Option<u64>,
}

#[derive(Debug)]
pub struct WebhookQueue {
    _path: PathBuf,
    connection: Mutex<Connection>,
}

impl WebhookQueue {
    pub fn open(path: impl AsRef<Path>) -> QueueResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(&path).map_err(sql_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS jobs (
               job_id TEXT PRIMARY KEY,
               event_id TEXT NOT NULL,
               hook_name TEXT NOT NULL,
               endpoint TEXT NOT NULL,
               payload BLOB NOT NULL,
               state TEXT NOT NULL,
               attempts_used INTEGER NOT NULL,
               max_attempts INTEGER NOT NULL,
               next_attempt_at_ms INTEGER NOT NULL,
               lease_owner TEXT,
               lease_expires_at_ms INTEGER,
               last_error TEXT,
               created_from_sequence INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS jobs_due_idx
               ON jobs(state, next_attempt_at_ms, created_from_sequence);
             CREATE INDEX IF NOT EXISTS jobs_lease_idx
               ON jobs(state, lease_expires_at_ms);
             CREATE INDEX IF NOT EXISTS jobs_hook_idx
               ON jobs(hook_name);",
            )
            .map_err(sql_error)?;
        Ok(Self {
            _path: path,
            connection: Mutex::new(connection),
        })
    }

    pub fn reset(&self) -> QueueResult<()> {
        self.connection()
            .execute("DELETE FROM jobs", [])
            .map_err(sql_error)?;
        Ok(())
    }

    pub fn apply_log_record(&self, sequence: u64, record: &LogRecord) -> QueueResult<()> {
        match record {
            LogRecord::Command(_) => Ok(()),
            LogRecord::WebhookEnqueue(record) => self.apply_enqueue(sequence, record),
            LogRecord::WebhookAck(record) => self.apply_ack(record),
            LogRecord::WebhookRetryScheduled(record) => self.apply_retry(record),
            LogRecord::WebhookDeadLetter(record) => self.apply_dead_letter(record),
        }
    }

    pub fn release_expired_leases(&self, now_ms: u64) -> QueueResult<u64> {
        let changed = self
            .connection()
            .execute(
                "UPDATE jobs
                 SET state = 'pending',
                     lease_owner = NULL,
                     lease_expires_at_ms = NULL,
                     updated_at_ms = ?1
                 WHERE state = 'leased' AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms <= ?1",
                params![now_ms],
            )
            .map_err(sql_error)? as u64;
        Ok(changed)
    }

    pub fn lease_due(
        &self,
        limit: usize,
        lease_ms: u64,
        worker_id: &str,
        now_ms: u64,
    ) -> QueueResult<Vec<QueueJob>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut connection = self.connection();
        let transaction = connection.transaction().map_err(sql_error)?;
        let jobs = {
            let mut statement = transaction
                .prepare(
                    "SELECT job_id, event_id, hook_name, endpoint, payload, attempts_used,
                            max_attempts, next_attempt_at_ms, last_error, created_from_sequence
                     FROM jobs
                     WHERE state = 'pending' AND next_attempt_at_ms <= ?1
                     ORDER BY next_attempt_at_ms ASC, created_from_sequence ASC
                     LIMIT ?2",
                )
                .map_err(sql_error)?;
            let rows = statement
                .query_map(params![now_ms, limit as i64], |row| {
                    Ok(QueueJob {
                        job_id: row.get::<_, String>(0)?,
                        event_id: row.get::<_, String>(1)?,
                        hook_name: row.get::<_, String>(2)?,
                        endpoint: row.get::<_, String>(3)?,
                        event: serde_json::from_slice::<GeofenceEvent>(&row.get::<_, Vec<u8>>(4)?)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Blob,
                                    Box::new(error),
                                )
                            })?,
                        attempts_used: row.get::<_, u32>(5)?,
                        max_attempts: row.get::<_, u32>(6)?,
                        next_attempt_at_ms: row.get::<_, u64>(7)?,
                        last_error: row.get::<_, Option<String>>(8)?,
                        created_from_sequence: row.get::<_, u64>(9)?,
                    })
                })
                .map_err(sql_error)?;
            let mut jobs = Vec::new();
            for row in rows {
                jobs.push(row.map_err(sql_error)?);
            }
            jobs
        };

        let lease_expires_at_ms = now_ms.saturating_add(lease_ms.max(1));
        for job in &jobs {
            transaction
                .execute(
                    "UPDATE jobs
                     SET state = 'leased',
                         lease_owner = ?2,
                         lease_expires_at_ms = ?3,
                         updated_at_ms = ?1
                     WHERE job_id = ?4",
                    params![now_ms, worker_id, lease_expires_at_ms, job.job_id],
                )
                .map_err(sql_error)?;
        }
        transaction.commit().map_err(sql_error)?;
        Ok(jobs)
    }

    pub fn stats(&self, now_ms: u64) -> QueueResult<QueueStats> {
        let connection = self.connection();
        let pending = count_by_state(&connection, "pending")?;
        let leased = count_by_state(&connection, "leased")?;
        let dead_letter = count_by_state(&connection, "dead_letter")?;
        let oldest_pending_at = connection
            .query_row(
                "SELECT MIN(next_attempt_at_ms) FROM jobs WHERE state = 'pending'",
                [],
                |row| row.get::<_, Option<u64>>(0),
            )
            .map_err(sql_error)?;
        Ok(QueueStats {
            pending,
            leased,
            dead_letter,
            oldest_pending_age_ms: oldest_pending_at.map(|oldest| now_ms.saturating_sub(oldest)),
        })
    }

    pub fn next_due_at_ms(&self) -> QueueResult<Option<u64>> {
        self.connection()
            .query_row(
                "SELECT MIN(next_attempt_at_ms) FROM jobs WHERE state = 'pending'",
                [],
                |row| row.get::<_, Option<u64>>(0),
            )
            .map_err(sql_error)
    }

    pub fn snapshot_log_records(&self) -> QueueResult<Vec<LogRecord>> {
        let connection = self.connection();
        let mut statement = connection
            .prepare(
                "SELECT job_id, payload, endpoint, attempts_used, max_attempts, next_attempt_at_ms,
                        last_error, state
                 FROM jobs
                 WHERE state IN ('pending', 'leased', 'dead_letter')
                 ORDER BY created_from_sequence ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(sql_error)?;
        let mut records = Vec::new();
        for row in rows {
            let (
                job_id,
                payload,
                endpoint,
                attempts_used,
                max_attempts,
                next_attempt_at_ms,
                last_error,
                state,
            ) = row.map_err(sql_error)?;
            let event = serde_json::from_slice::<GeofenceEvent>(&payload)
                .map_err(|error| QueueError::Codec(error.to_string()))?;
            records.push(LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                job_id: job_id.clone(),
                event,
                endpoint,
                attempts_used,
                max_attempts,
                next_attempt_at_ms,
            }));
            if state == "dead_letter" {
                records.push(LogRecord::WebhookDeadLetter(WebhookDeadLetterRecord {
                    job_id,
                    attempts_used,
                    last_error: last_error.unwrap_or_else(|| "dead-lettered".to_owned()),
                }));
            }
        }
        Ok(records)
    }

    fn apply_enqueue(&self, sequence: u64, record: &WebhookEnqueueRecord) -> QueueResult<()> {
        let payload = serde_json::to_vec(&record.event)
            .map_err(|error| QueueError::Codec(error.to_string()))?;
        let event_id = record
            .event
            .event_id
            .clone()
            .unwrap_or_else(|| record.job_id.clone());
        let hook_name = record
            .event
            .hook
            .clone()
            .unwrap_or_else(|| record.job_id.clone());
        self.connection()
            .execute(
                "INSERT INTO jobs(
                   job_id, event_id, hook_name, endpoint, payload, state, attempts_used,
                   max_attempts, next_attempt_at_ms, lease_owner, lease_expires_at_ms,
                   last_error, created_from_sequence, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, NULL, NULL, NULL, ?9, ?10)
                 ON CONFLICT(job_id) DO UPDATE SET
                   event_id = excluded.event_id,
                   hook_name = excluded.hook_name,
                   endpoint = excluded.endpoint,
                   payload = excluded.payload,
                   state = 'pending',
                   attempts_used = excluded.attempts_used,
                   max_attempts = excluded.max_attempts,
                   next_attempt_at_ms = excluded.next_attempt_at_ms,
                   lease_owner = NULL,
                   lease_expires_at_ms = NULL,
                   last_error = NULL,
                   created_from_sequence = excluded.created_from_sequence,
                   updated_at_ms = excluded.updated_at_ms",
                params![
                    record.job_id,
                    event_id,
                    hook_name,
                    record.endpoint,
                    payload,
                    record.attempts_used,
                    record.max_attempts,
                    record.next_attempt_at_ms,
                    sequence,
                    now_ms(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn apply_ack(&self, record: &WebhookAckRecord) -> QueueResult<()> {
        self.connection()
            .execute("DELETE FROM jobs WHERE job_id = ?1", params![record.job_id])
            .map_err(sql_error)?;
        Ok(())
    }

    fn apply_retry(&self, record: &WebhookRetryScheduledRecord) -> QueueResult<()> {
        self.connection()
            .execute(
                "UPDATE jobs
                 SET state = 'pending',
                     attempts_used = ?2,
                     next_attempt_at_ms = ?3,
                     lease_owner = NULL,
                     lease_expires_at_ms = NULL,
                     last_error = ?4,
                     updated_at_ms = ?5
                 WHERE job_id = ?1",
                params![
                    record.job_id,
                    record.attempts_used,
                    record.next_attempt_at_ms,
                    record.last_error,
                    now_ms(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn apply_dead_letter(&self, record: &WebhookDeadLetterRecord) -> QueueResult<()> {
        self.connection()
            .execute(
                "UPDATE jobs
                 SET state = 'dead_letter',
                     attempts_used = ?2,
                     lease_owner = NULL,
                     lease_expires_at_ms = NULL,
                     last_error = ?3,
                     updated_at_ms = ?4
                 WHERE job_id = ?1",
                params![
                    record.job_id,
                    record.attempts_used,
                    record.last_error,
                    now_ms(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn count_by_state(connection: &Connection, state: &str) -> QueueResult<u64> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE state = ?1",
            params![state],
            |row| row.get::<_, u64>(0),
        )
        .map_err(sql_error)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn io_error(error: std::io::Error) -> QueueError {
    QueueError::Sql(error.to_string())
}

fn sql_error(error: rusqlite::Error) -> QueueError {
    QueueError::Sql(error.to_string())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{QueueStats, WebhookQueue};
    use latlng_core::{
        LogRecord, WebhookAckRecord, WebhookDeadLetterRecord, WebhookEnqueueRecord,
        WebhookRetryScheduledRecord,
    };
    use latlng_geofence::{DetectType, GeofenceEvent, MutationCommand};

    #[test]
    fn queue_roundtrip_applies_enqueue_retry_and_ack() {
        let dir = tempdir().unwrap();
        let queue = WebhookQueue::open(dir.path().join("queue.sqlite")).unwrap();
        let event = sample_event();
        queue
            .apply_log_record(
                2,
                &LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                    job_id: "job-1".to_owned(),
                    event: event.clone(),
                    endpoint: "https://example.com".to_owned(),
                    attempts_used: 0,
                    max_attempts: 9,
                    next_attempt_at_ms: 10,
                }),
            )
            .unwrap();
        let jobs = queue.lease_due(1, 1_000, "worker", 10).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "job-1");

        queue.release_expired_leases(2_000).unwrap();
        queue
            .apply_log_record(
                3,
                &LogRecord::WebhookRetryScheduled(WebhookRetryScheduledRecord {
                    job_id: "job-1".to_owned(),
                    attempts_used: 1,
                    next_attempt_at_ms: 5_000,
                    last_error: "timeout".to_owned(),
                }),
            )
            .unwrap();
        let stats = queue.stats(6_000).unwrap();
        assert_eq!(
            stats,
            QueueStats {
                pending: 1,
                leased: 0,
                dead_letter: 0,
                oldest_pending_age_ms: Some(1_000),
            }
        );

        queue
            .apply_log_record(
                4,
                &LogRecord::WebhookAck(WebhookAckRecord {
                    job_id: "job-1".to_owned(),
                }),
            )
            .unwrap();
        assert_eq!(queue.stats(6_000).unwrap().pending, 0);
    }

    #[test]
    fn queue_tracks_dead_letter_jobs() {
        let dir = tempdir().unwrap();
        let queue = WebhookQueue::open(dir.path().join("queue.sqlite")).unwrap();
        queue
            .apply_log_record(
                1,
                &LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                    job_id: "job-2".to_owned(),
                    event: sample_event(),
                    endpoint: "https://example.com".to_owned(),
                    attempts_used: 8,
                    max_attempts: 9,
                    next_attempt_at_ms: 1,
                }),
            )
            .unwrap();
        queue
            .apply_log_record(
                2,
                &LogRecord::WebhookDeadLetter(WebhookDeadLetterRecord {
                    job_id: "job-2".to_owned(),
                    attempts_used: 9,
                    last_error: "status 500".to_owned(),
                }),
            )
            .unwrap();
        assert_eq!(queue.stats(10).unwrap().dead_letter, 1);
        let snapshot = queue.snapshot_log_records().unwrap();
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn queue_reports_next_due_pending_job() {
        let dir = tempdir().unwrap();
        let queue = WebhookQueue::open(dir.path().join("queue.sqlite")).unwrap();
        assert_eq!(queue.next_due_at_ms().unwrap(), None);

        queue
            .apply_log_record(
                1,
                &LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                    job_id: "job-3".to_owned(),
                    event: sample_event(),
                    endpoint: "https://example.com".to_owned(),
                    attempts_used: 0,
                    max_attempts: 9,
                    next_attempt_at_ms: 250,
                }),
            )
            .unwrap();
        queue
            .apply_log_record(
                2,
                &LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                    job_id: "job-4".to_owned(),
                    event: sample_event(),
                    endpoint: "https://example.com".to_owned(),
                    attempts_used: 0,
                    max_attempts: 9,
                    next_attempt_at_ms: 100,
                }),
            )
            .unwrap();

        assert_eq!(queue.next_due_at_ms().unwrap(), Some(100));

        let leased = queue.lease_due(1, 1_000, "worker", 100).unwrap();
        assert_eq!(leased.len(), 1);
        assert_eq!(leased[0].job_id, "job-4");
        assert_eq!(queue.next_due_at_ms().unwrap(), Some(250));
    }

    fn sample_event() -> GeofenceEvent {
        GeofenceEvent {
            command: MutationCommand::Set,
            detect: DetectType::Enter,
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: latlng_core::geo::GeoType::point(52.52, 13.405),
            fields: latlng_core::geo::FieldMap::new(),
            timestamp_ns: 1,
            event_id: Some("evt-1".to_owned()),
            job_id: Some("job-1".to_owned()),
            hook: Some("hook-1".to_owned()),
            group: Some("hook-1".to_owned()),
            nearby: None,
            generation: 0,
        }
    }
}
