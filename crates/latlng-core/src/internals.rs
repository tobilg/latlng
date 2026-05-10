use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub(crate) fn resolve_area(&self, area: Area) -> Result<Area> {
        match area {
            Area::Reference { collection, id } => {
                let handle = self.collection_handle(&collection).ok_or_else(|| {
                    CoreError::ObjectNotFound {
                        collection: collection.clone(),
                        id: id.clone(),
                    }
                })?;
                let object = self
                    .get_live_object_from_handle(&handle, &id)
                    .ok_or(CoreError::ObjectNotFound { collection, id })?;
                Ok(Area::GeoJson(object.geo.to_geojson_value()?))
            }
            other => Ok(other),
        }
    }

    pub(crate) fn get_live_object(&self, collection: &str, id: &str) -> Result<Option<Object>> {
        Ok(self
            .collection_handle(collection)
            .and_then(|handle| self.get_live_object_from_handle(&handle, id)))
    }

    pub(crate) fn ensure_writable(&self) -> Result<()> {
        if P::read(&self.config).read_only {
            return Err(CoreError::ReadOnly);
        }
        Ok(())
    }

    pub(crate) fn reserve_sequences(&self, count: usize) -> Vec<u64> {
        let mut next = P::write(&self.next_sequence);
        (0..count)
            .map(|_| {
                *next += 1;
                *next
            })
            .collect()
    }

    #[allow(dead_code)]
    pub(crate) fn preview_sequences(&self, count: usize) -> Vec<u64> {
        let next = *P::read(&self.next_sequence);
        (0..count).map(|index| next + index as u64 + 1).collect()
    }

    pub(crate) fn current_webhook_retry_count(&self) -> u32 {
        match P::read(&self.config).webhook_retry_count {
            0 => 0,
            value => value,
        }
    }

    pub(crate) fn plan_mutation(
        &self,
        event: &MutationEvent,
    ) -> Result<latlng_geofence::PreparedMutation> {
        let lookup = |collection: &str| {
            self.collection_handle(collection)
                .map(|handle| {
                    let state = P::read(&*handle);
                    state
                        .collection
                        .objects
                        .values()
                        .filter(|object| !is_expired(object))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        P::read(&self.geofences)
            .prepare_mutation(event, &lookup)
            .map_err(|error| CoreError::Message(error.to_string()))
    }

    pub(crate) fn webhook_records_for_prepared(
        &self,
        prepared: &latlng_geofence::PreparedMutation,
        retry_count: u32,
    ) -> Result<Vec<WebhookEnqueueRecord>> {
        let mut records = Vec::new();
        for event in prepared.events() {
            let Some(hook_name) = event.hook.as_deref() else {
                continue;
            };
            let endpoint = self
                .hook_def(hook_name)
                .map(|hook| hook.endpoint)
                .ok_or_else(|| {
                    CoreError::Message(format!("hook missing during enqueue: {hook_name}"))
                })?;
            records.push(WebhookEnqueueRecord {
                job_id: String::new(),
                event: event.clone(),
                endpoint,
                attempts_used: 0,
                max_attempts: retry_count.saturating_add(1),
                next_attempt_at_ms: now_millis(),
            });
        }
        Ok(records)
    }

    pub(crate) fn build_command_batch_records(
        &self,
        command: Command,
        sequences: &[u64],
        webhook_records: &[WebhookEnqueueRecord],
    ) -> (Vec<LogRecord>, Vec<WebhookEnqueueRecord>) {
        let mut records = Vec::with_capacity(1 + webhook_records.len());
        let mut prepared_webhooks = Vec::with_capacity(webhook_records.len());
        records.push(LogRecord::Command(command));
        let command_sequence = sequences[0];
        for (index, (sequence, record)) in sequences
            .iter()
            .copied()
            .skip(1)
            .zip(webhook_records.iter())
            .enumerate()
        {
            let mut enqueue = record.clone();
            enqueue.job_id = opaque_webhook_job_id(sequence);
            enqueue.event.job_id = Some(enqueue.job_id.clone());
            enqueue.event.event_id = Some(opaque_webhook_event_id(
                command_sequence,
                index,
                &enqueue.event,
            ));
            prepared_webhooks.push(enqueue.clone());
            records.push(LogRecord::WebhookEnqueue(enqueue));
        }
        (records, prepared_webhooks)
    }

    pub(crate) fn encode_entries_for_sequences(
        &self,
        records: &[LogRecord],
        sequences: &[u64],
    ) -> Result<Vec<StorageEntry>> {
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
        Ok(entries)
    }

    #[allow(dead_code)]
    pub(crate) fn preview_command_batch(
        &self,
        command: Command,
        webhook_records: &[WebhookEnqueueRecord],
    ) -> Result<(Vec<StorageEntry>, Vec<WebhookEnqueueRecord>)> {
        let sequences = self.preview_sequences(1 + webhook_records.len());
        let (records, prepared_webhooks) =
            self.build_command_batch_records(command, &sequences, webhook_records);
        let entries = self.encode_entries_for_sequences(&records, &sequences)?;
        Ok((entries, prepared_webhooks))
    }

    #[allow(dead_code)]
    pub(crate) fn preview_log_record_entry(&self, record: LogRecord) -> Result<StorageEntry> {
        let sequence = self.preview_sequences(1).into_iter().next().unwrap_or(1);
        let mut entries = self.encode_entries_for_sequences(&[record], &[sequence])?;
        entries
            .pop()
            .ok_or_else(|| CoreError::Message("failed to preview storage entry".to_owned()))
    }

    pub(crate) fn persist_command_batch(
        &self,
        command: Command,
        webhook_records: &[WebhookEnqueueRecord],
    ) -> Result<Vec<u64>> {
        let sequences = self.reserve_sequences(1 + webhook_records.len());
        if !self.storage.stores_command_log() {
            return Ok(sequences);
        }
        let (records, _) = self.build_command_batch_records(command, &sequences, webhook_records);
        let entries = self.encode_entries_for_sequences(&records, &sequences)?;
        self.storage.append_batch(&entries)?;
        Ok(sequences)
    }

    #[allow(dead_code)]
    pub(crate) fn preview_set_command_batch(
        &self,
        req: &SetRequest,
    ) -> Result<(bool, Vec<StorageEntry>, Vec<WebhookEnqueueRecord>)> {
        let _gate = self.read_control();
        self.ensure_writable()?;
        let before = self.collection_handle(&req.collection).and_then(|handle| {
            let collection = P::read(&*handle);
            collection.collection.objects.get(&req.id).cloned()
        });
        let exists = before.is_some();
        match req.condition {
            SetCondition::Nx if exists => return Ok((false, Vec::new(), Vec::new())),
            SetCondition::Xx if !exists => return Ok((false, Vec::new(), Vec::new())),
            SetCondition::Always | SetCondition::Nx | SetCondition::Xx => {}
        }

        let expires_at = req
            .expire_seconds
            .map(|seconds| now_millis().saturating_add(u64::from(seconds) * 1_000));
        let object = Object {
            id: req.id.clone(),
            geo: req.object.clone(),
            fields: field_entries_to_map(&req.fields),
            expires_at,
        };
        let event = MutationEvent {
            command: MutationCommand::Set,
            collection: req.collection.clone(),
            id: req.id.clone(),
            before,
            after: Some(object),
            timestamp_ns: now_nanos(),
        };
        let planned = self.plan_mutation(&event)?;
        let webhook_records =
            self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;
        let (entries, prepared_webhooks) = self.preview_command_batch(
            Command::SetPersisted(PersistedSetRecord {
                collection: req.collection.clone(),
                id: req.id.clone(),
                object: req.object.clone(),
                fields: req.fields.clone(),
                expires_at_ms: expires_at,
            }),
            &webhook_records,
        )?;
        Ok((true, entries, prepared_webhooks))
    }

    #[allow(dead_code)]
    pub(crate) fn preview_del_command_batch(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<(bool, Vec<StorageEntry>, Vec<WebhookEnqueueRecord>)> {
        let _gate = self.read_control();
        self.ensure_writable()?;
        let before = self.collection_handle(collection).and_then(|handle| {
            let collection_state = P::read(&*handle);
            collection_state.collection.objects.get(id).cloned()
        });
        let Some(before) = before else {
            return Ok((false, Vec::new(), Vec::new()));
        };

        let event = MutationEvent {
            command: MutationCommand::Del,
            collection: collection.to_owned(),
            id: id.to_owned(),
            before: Some(before),
            after: None,
            timestamp_ns: now_nanos(),
        };
        let planned = self.plan_mutation(&event)?;
        let webhook_records =
            self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;
        let (entries, prepared_webhooks) = self.preview_command_batch(
            Command::Del {
                collection: collection.to_owned(),
                id: id.to_owned(),
            },
            &webhook_records,
        )?;
        Ok((true, entries, prepared_webhooks))
    }

    #[allow(dead_code)]
    pub(crate) fn preview_drop_collection_command_batch(
        &self,
        collection: &str,
    ) -> Result<(bool, Vec<StorageEntry>, Vec<WebhookEnqueueRecord>)> {
        let _gate = self.read_control();
        self.ensure_writable()?;
        let Some(handle) = self.collection_handle(collection) else {
            return Ok((false, Vec::new(), Vec::new()));
        };
        let previous = {
            let state = P::read(&*handle);
            state.collection.clone()
        };
        let mut webhook_records = Vec::new();
        for object in previous.objects.values() {
            let event = MutationEvent {
                command: MutationCommand::Drop,
                collection: collection.to_owned(),
                id: object.id.clone(),
                before: Some(object.clone()),
                after: None,
                timestamp_ns: now_nanos(),
            };
            let planned = self.plan_mutation(&event)?;
            webhook_records.extend(
                self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?,
            );
        }
        let (entries, prepared_webhooks) = self.preview_command_batch(
            Command::DropCollection {
                collection: collection.to_owned(),
            },
            &webhook_records,
        )?;
        Ok((true, entries, prepared_webhooks))
    }
}
