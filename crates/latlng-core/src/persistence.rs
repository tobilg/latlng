use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub fn replay_log(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(u64, LogRecord) -> Result<()>,
    ) -> Result<u64> {
        let mut replay_error = None;
        let last = self
            .storage
            .replay(after_seq, &mut |entry| {
                let record = match decode_log_record(&entry.command) {
                    Ok(record) => record,
                    Err(error) => {
                        replay_error = Some(error);
                        return Err(StorageError::Message("log decode failed".to_owned()));
                    }
                };
                if let Err(error) = callback(entry.sequence, record) {
                    replay_error = Some(error);
                    return Err(StorageError::Message("log callback failed".to_owned()));
                }
                Ok(())
            })
            .map_err(CoreError::from)?;
        if let Some(error) = replay_error {
            return Err(error);
        }
        Ok(last)
    }

    pub fn append_log_record(&self, record: LogRecord) -> Result<u64> {
        Ok(self.append_log_records(std::slice::from_ref(&record))?[0])
    }

    pub fn append_log_records(&self, records: &[LogRecord]) -> Result<Vec<u64>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        if !self.storage.stores_command_log() {
            return Ok(self.reserve_sequences(records.len()));
        }
        let sequences = self.reserve_sequences(records.len());
        let entries = records
            .iter()
            .zip(sequences.iter().copied())
            .map(|(record, sequence)| {
                Ok(StorageEntry {
                    sequence,
                    timestamp_ns: now_nanos(),
                    command: Bytes::from(encode_log_record(record)?),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.storage.append_batch(&entries)?;
        Ok(sequences)
    }

    pub fn snapshot_base_log_records(&self) -> Vec<LogRecord> {
        let mut records = Vec::new();
        let collections = P::read(&self.collections);
        let mut collection_names = collections.keys().cloned().collect::<Vec<_>>();
        collection_names.sort();
        for collection_name in collection_names {
            let Some(handle) = collections.get(&collection_name).cloned() else {
                continue;
            };
            records.push(LogRecord::Command(Command::CreateCollection {
                collection: collection_name.clone(),
            }));
            let collection = P::read(&*handle);
            let mut object_ids = collection
                .collection
                .objects
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            object_ids.sort();
            for id in object_ids {
                let Some(object) = collection.collection.objects.get(&id) else {
                    continue;
                };
                if is_expired(object) {
                    continue;
                }
                let fields = object
                    .fields
                    .iter()
                    .map(|(name, value)| FieldEntry {
                        name: name.to_owned(),
                        value: value.clone(),
                    })
                    .collect::<Vec<_>>();
                records.push(LogRecord::Command(Command::SetPersisted(
                    PersistedSetRecord {
                        collection: collection_name.clone(),
                        id: id.clone(),
                        object: object.geo.clone(),
                        fields,
                        expires_at_ms: object.expires_at,
                    },
                )));
            }
        }
        drop(collections);

        for channel in self.channel_defs() {
            records.push(LogRecord::Command(Command::SetChannel {
                name: channel.name,
                def: channel.def,
            }));
        }
        for hook in self.hook_defs() {
            records.push(LogRecord::Command(Command::SetHook {
                name: hook.name,
                endpoint: hook.endpoint,
                def: hook.def,
            }));
        }
        records
    }

    pub fn rewrite_log_snapshot(&self, records: &[LogRecord]) -> Result<CompactionResult> {
        if !self.storage.stores_command_log() {
            return Ok(CompactionResult {
                before_entries: 0,
                after_entries: 0,
                before_bytes: 0,
                after_bytes: 0,
            });
        }

        let mut before_entries = 0_u64;
        let mut before_bytes = 0_u64;
        let current_last_sequence = self.storage.last_sequence()?;
        self.storage.replay(0, &mut |entry| {
            before_entries += 1;
            before_bytes += 8 + entry.command.len() as u64;
            Ok(())
        })?;

        let first_sequence = if records.is_empty() {
            0
        } else if current_last_sequence >= records.len() as u64 {
            current_last_sequence - records.len() as u64 + 1
        } else {
            1
        };
        let entries = records
            .iter()
            .enumerate()
            .map(|(index, record)| {
                Ok(StorageEntry {
                    sequence: first_sequence + index as u64,
                    timestamp_ns: now_nanos(),
                    command: Bytes::from(encode_log_record(record)?),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let after_entries = entries.len() as u64;
        let after_bytes = entries
            .iter()
            .map(|entry| 8 + entry.command.len() as u64)
            .sum::<u64>();
        self.storage.snapshot(entries)?;
        *P::write(&self.next_sequence) = if after_entries == 0 {
            current_last_sequence
        } else {
            first_sequence + after_entries - 1
        };
        Ok(CompactionResult {
            before_entries,
            after_entries,
            before_bytes,
            after_bytes,
        })
    }
}
