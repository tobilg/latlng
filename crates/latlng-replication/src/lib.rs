#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use latlng_storage::{StorageBackend, StorageEntry, StorageResult};
use serde::{Deserialize, Serialize};

pub use latlng_storage as storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationRole {
    #[default]
    Leader,
    Follower,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowTarget {
    pub host: String,
    pub port: u16,
}

impl FollowTarget {
    pub fn display(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicationStatus {
    pub server_id: String,
    pub role: ReplicationRole,
    pub follow_target: Option<FollowTarget>,
    pub leader_id: Option<String>,
    pub caught_up: bool,
    pub caught_up_once: bool,
    #[serde(default)]
    pub local_last_sequence: u64,
    #[serde(default)]
    pub leader_last_sequence: u64,
    #[serde(default)]
    pub reconnects_total: u64,
    #[serde(default)]
    pub checksum_mismatches_total: u64,
    #[serde(default)]
    pub resyncs_total: u64,
    pub last_error: Option<String>,
}

impl ReplicationStatus {
    pub fn leader(server_id: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
            role: ReplicationRole::Leader,
            follow_target: None,
            leader_id: None,
            caught_up: true,
            caught_up_once: true,
            local_last_sequence: 0,
            leader_last_sequence: 0,
            reconnects_total: 0,
            checksum_mismatches_total: 0,
            resyncs_total: 0,
            last_error: None,
        }
    }

    pub fn follower(server_id: impl Into<String>, target: FollowTarget) -> Self {
        Self {
            server_id: server_id.into(),
            role: ReplicationRole::Follower,
            follow_target: Some(target),
            leader_id: None,
            caught_up: false,
            caught_up_once: false,
            local_last_sequence: 0,
            leader_last_sequence: 0,
            reconnects_total: 0,
            checksum_mismatches_total: 0,
            resyncs_total: 0,
            last_error: None,
        }
    }

    pub fn is_follower(&self) -> bool {
        matches!(self.role, ReplicationRole::Follower)
    }

    pub fn queries_allowed(&self) -> bool {
        !self.is_follower() || self.caught_up_once
    }

    pub fn effective_read_only(&self, configured_read_only: bool) -> bool {
        configured_read_only || self.is_follower()
    }

    pub fn following(&self) -> Option<String> {
        self.follow_target.as_ref().map(FollowTarget::display)
    }
}

pub type SharedReplicationStatus = Arc<RwLock<ReplicationStatus>>;

pub type ReplicationFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
pub type SharedReplicationCoordinator = Arc<dyn ReplicationCoordinator>;

pub trait ReplicationCoordinator: Send + Sync {
    fn follow(&self, host: String, port: u16) -> ReplicationFuture<'_>;
    fn unfollow(&self) -> ReplicationFuture<'_>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicationChunk {
    pub entries: Vec<StorageEntry>,
    pub checksum: [u8; 16],
    pub last_sequence: u64,
}

pub fn fetch_chunk<B: StorageBackend>(
    backend: &B,
    after_sequence: u64,
    max_entries: usize,
) -> StorageResult<ReplicationChunk> {
    let mut entries = Vec::new();
    let mut last_sequence = after_sequence;
    backend.replay(after_sequence, &mut |entry| {
        last_sequence = entry.sequence;
        if entries.len() < max_entries.max(1) {
            entries.push(entry);
        }
        Ok(())
    })?;
    let checksum = StorageEntry::checksum(&entries);
    Ok(ReplicationChunk {
        entries,
        checksum,
        last_sequence,
    })
}

pub fn apply_chunk<B: StorageBackend>(backend: &B, chunk: &ReplicationChunk) -> StorageResult<()> {
    backend.append_batch(&chunk.entries)
}

pub fn verify_range<B: StorageBackend>(
    backend: &B,
    from: u64,
    to: u64,
    expected: [u8; 16],
) -> StorageResult<bool> {
    Ok(backend.checksum(from, to)? == expected)
}
