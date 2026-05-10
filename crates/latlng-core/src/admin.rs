use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub fn flushdb(&self) -> Result<()> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        self.append_log_record(LogRecord::Command(Command::FlushDb))?;
        P::write(&self.collections).clear();
        P::write(&self.geofences).clear_all();
        self.storage.snapshot(Vec::new())?;
        Ok(())
    }

    pub fn gc(&self) {
        let _gate = self.write_control();
        gc_collections_locked::<P>(&self.collections);
    }

    pub fn server_info(&self) -> ServerInfo {
        let _gate = self.read_control();
        let collections = P::read(&self.collections);
        let mut info = ServerInfo {
            version: PRODUCT_VERSION.to_owned(),
            api_version: API_VERSION.to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            storage_format_version: STORAGE_FORMAT_VERSION.to_owned(),
            num_collections: collections.len() as u32,
            read_only: P::read(&self.config).read_only,
            leader: true,
            last_sequence: *P::read(&self.next_sequence),
            ..ServerInfo::default()
        };
        let handles = collections.values().cloned().collect::<Vec<_>>();
        drop(collections);
        for handle in handles {
            let collection = P::read(&*handle);
            for object in collection
                .collection
                .objects
                .values()
                .filter(|object| !is_expired(object))
            {
                info.num_objects += 1;
                if matches!(object.geo, GeoType::Point { .. }) {
                    info.num_points += 1;
                }
            }
        }
        info.heap_bytes = info.num_objects * 128;
        info
    }

    pub fn last_sequence(&self) -> u64 {
        let _gate = self.read_control();
        *P::read(&self.next_sequence)
    }

    pub fn checksum_range(&self, from: u64, to: u64) -> Result<[u8; 16]> {
        let _gate = self.read_control();
        self.storage.checksum(from, to).map_err(CoreError::from)
    }

    pub fn storage_entries_after(
        &self,
        after_seq: u64,
        max_entries: usize,
    ) -> Result<Vec<StorageEntry>> {
        let _gate = self.read_control();
        let mut entries = Vec::new();
        let limit = max_entries.max(1);
        self.storage.replay(after_seq, &mut |entry| {
            if entries.len() < limit {
                entries.push(entry);
            }
            Ok(())
        })?;
        Ok(entries)
    }

    pub fn reset_replication_state(&self) -> Result<()> {
        let _gate = self.write_control();
        P::write(&self.collections).clear();
        P::write(&self.geofences).clear_all();
        self.storage.snapshot(Vec::new())?;
        *P::write(&self.next_sequence) = 0;
        Ok(())
    }

    pub fn apply_replicated_entries(&self, entries: &[StorageEntry]) -> Result<u64> {
        if entries.is_empty() {
            return Ok(self.last_sequence());
        }
        let _gate = self.write_control();
        let expected = *P::read(&self.next_sequence) + 1;
        if let Some(first) = entries.first()
            && first.sequence != expected
        {
            return Err(CoreError::Storage(StorageError::SequenceRegression {
                expected,
                actual: first.sequence,
            }));
        }
        self.storage.append_batch(entries)?;
        for entry in entries.iter().cloned() {
            apply_persisted_entry_to_state::<P>(&self.collections, &self.geofences, entry)?;
        }
        *P::write(&self.next_sequence) = entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(expected.saturating_sub(1));
        gc_collections_locked::<P>(&self.collections);
        Ok(*P::read(&self.next_sequence))
    }

    pub fn aofshrink(&self) -> Result<CompactionResult> {
        let _gate = self.write_control();
        let records = self.snapshot_base_log_records();
        self.rewrite_log_snapshot(&records)
    }

    pub fn timeout(&self, command: &str) -> Option<f64> {
        let _gate = self.read_control();
        P::read(&self.config).timeout_for(command)
    }

    pub fn set_timeout(&self, command: &str, seconds: f64) {
        let _gate = self.write_control();
        P::write(&self.config).set_timeout(command, seconds);
    }
}
