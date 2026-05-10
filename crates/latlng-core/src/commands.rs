use super::*;

impl<P: Platform, S: StorageBackend> LatLng<P, S> {
    pub(crate) fn try_set_local(&self, req: &SetRequest) -> Result<Option<bool>> {
        self.ensure_writable()?;
        if self.collection_requires_exclusive_geofence_path(&req.collection) {
            return Ok(None);
        }
        let Some(handle) = self.collection_handle(&req.collection) else {
            return Ok(None);
        };

        if !self.storage.stores_command_log()
            && matches!(req.condition, SetCondition::Always)
            && req.expire_seconds.is_none()
            && matches!(req.object, GeoType::Point { .. })
            && !self.collection_has_geofence_side_effects(&req.collection)
        {
            let object = Object {
                id: req.id.clone(),
                geo: req.object.clone(),
                fields: field_entries_to_map(&req.fields),
                expires_at: None,
            };
            let mut collection = P::write(&*handle);
            self.reserve_sequences(1);
            collection.collection.upsert(object)?;
            collection.version = collection.version.saturating_add(1);
            return Ok(Some(true));
        }

        loop {
            let (version, before) = {
                let collection = P::read(&*handle);
                (
                    collection.version,
                    collection.collection.objects.get(&req.id).cloned(),
                )
            };
            let exists = before.is_some();
            match req.condition {
                SetCondition::Nx if exists => return Ok(Some(false)),
                SetCondition::Xx if !exists => return Ok(Some(false)),
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
                after: Some(object.clone()),
                timestamp_ns: now_nanos(),
            };
            let planned = self.plan_mutation(&event)?;
            let webhook_records =
                self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;

            let mut collection = P::write(&*handle);
            if collection.version != version {
                continue;
            }
            self.persist_command_batch(
                Command::SetPersisted(PersistedSetRecord {
                    collection: req.collection.clone(),
                    id: req.id.clone(),
                    object: req.object.clone(),
                    fields: req.fields.clone(),
                    expires_at_ms: expires_at,
                }),
                &webhook_records,
            )?;
            collection.collection.upsert(object)?;
            collection.version = collection.version.saturating_add(1);
            drop(collection);
            P::write(&self.geofences).apply_prepared_mutation(planned);
            return Ok(Some(true));
        }
    }

    pub(crate) fn set_exclusive(&self, req: SetRequest) -> Result<bool> {
        self.ensure_writable()?;
        let before = self.collection_handle(&req.collection).and_then(|handle| {
            let collection = P::read(&*handle);
            collection.collection.objects.get(&req.id).cloned()
        });
        let exists = before.is_some();
        match req.condition {
            SetCondition::Nx if exists => return Ok(false),
            SetCondition::Xx if !exists => return Ok(false),
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
            after: Some(object.clone()),
            timestamp_ns: now_nanos(),
        };
        let planned = self.plan_mutation(&event)?;
        let webhook_records =
            self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;
        self.persist_command_batch(
            Command::SetPersisted(PersistedSetRecord {
                collection: req.collection.clone(),
                id: req.id.clone(),
                object: req.object.clone(),
                fields: req.fields.clone(),
                expires_at_ms: expires_at,
            }),
            &webhook_records,
        )?;
        let handle = self.ensure_collection_cell(&req.collection);
        let mut collection = P::write(&*handle);
        collection.collection.upsert(object)?;
        collection.version = collection.version.saturating_add(1);
        drop(collection);
        P::write(&self.geofences).apply_prepared_mutation(planned);
        Ok(true)
    }

    pub(crate) fn try_del_local(&self, collection: &str, id: &str) -> Result<Option<bool>> {
        self.ensure_writable()?;
        if self.collection_requires_exclusive_geofence_path(collection) {
            return Ok(None);
        }
        let Some(handle) = self.collection_handle(collection) else {
            return Ok(Some(false));
        };

        loop {
            let (version, before) = {
                let state = P::read(&*handle);
                (state.version, state.collection.objects.get(id).cloned())
            };
            let Some(before) = before else {
                return Ok(Some(false));
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

            let mut state = P::write(&*handle);
            if state.version != version {
                continue;
            }
            self.persist_command_batch(
                Command::Del {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                },
                &webhook_records,
            )?;
            let removed = state.collection.remove(id).is_some();
            if removed {
                state.version = state.version.saturating_add(1);
            }
            drop(state);
            P::write(&self.geofences).apply_prepared_mutation(planned);
            return Ok(Some(removed));
        }
    }

    pub(crate) fn try_fset_local(
        &self,
        collection: &str,
        id: &str,
        fields: &[FieldEntry],
        xx: bool,
    ) -> Result<Option<bool>> {
        self.ensure_writable()?;
        if self.collection_requires_exclusive_geofence_path(collection) {
            return Ok(None);
        }
        let handle = self.existing_collection_handle(collection)?;

        loop {
            let (version, before) = {
                let state = P::read(&*handle);
                (state.version, state.collection.objects.get(id).cloned())
            };
            let Some(before) = before else {
                return if xx {
                    Ok(Some(false))
                } else {
                    Err(CoreError::ObjectNotFound {
                        collection: collection.to_owned(),
                        id: id.to_owned(),
                    })
                };
            };
            let mut after = before.clone();
            for field in fields {
                after.fields.insert(field.name.clone(), field.value.clone());
            }
            let event = MutationEvent {
                command: MutationCommand::Fset,
                collection: collection.to_owned(),
                id: id.to_owned(),
                before: Some(before),
                after: Some(after),
                timestamp_ns: now_nanos(),
            };
            let planned = self.plan_mutation(&event)?;
            let webhook_records =
                self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;

            let mut state = P::write(&*handle);
            if state.version != version {
                continue;
            }
            self.persist_command_batch(
                Command::Fset {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                    fields: fields.to_vec(),
                    xx,
                },
                &webhook_records,
            )?;
            if !state.collection.insert_fields(id, fields) {
                continue;
            };
            state.version = state.version.saturating_add(1);
            drop(state);
            P::write(&self.geofences).apply_prepared_mutation(planned);
            return Ok(Some(true));
        }
    }

    pub(crate) fn fset_exclusive(
        &self,
        collection: &str,
        id: &str,
        fields: &[FieldEntry],
        xx: bool,
    ) -> Result<bool> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        let before = {
            let state = P::read(&*handle);
            let Some(object) = state.collection.objects.get(id) else {
                return if xx {
                    Ok(false)
                } else {
                    Err(CoreError::ObjectNotFound {
                        collection: collection.to_owned(),
                        id: id.to_owned(),
                    })
                };
            };
            object.clone()
        };
        let mut after = before.clone();
        for field in fields {
            after.fields.insert(field.name.clone(), field.value.clone());
        }
        let event = MutationEvent {
            command: MutationCommand::Fset,
            collection: collection.to_owned(),
            id: id.to_owned(),
            before: Some(before),
            after: Some(after),
            timestamp_ns: now_nanos(),
        };
        let planned = self.plan_mutation(&event)?;
        let webhook_records =
            self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;
        self.persist_command_batch(
            Command::Fset {
                collection: collection.to_owned(),
                id: id.to_owned(),
                fields: fields.to_vec(),
                xx,
            },
            &webhook_records,
        )?;
        let mut state = P::write(&*handle);
        if !state.collection.insert_fields(id, fields) {
            return if xx {
                Ok(false)
            } else {
                Err(CoreError::ObjectNotFound {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                })
            };
        }
        state.version = state.version.saturating_add(1);
        drop(state);
        P::write(&self.geofences).apply_prepared_mutation(planned);
        Ok(true)
    }

    pub(crate) fn try_expire_local(
        &self,
        collection: &str,
        id: &str,
        seconds: u32,
    ) -> Result<bool> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        let expires_at_ms = now_millis().saturating_add(u64::from(seconds) * 1_000);

        loop {
            let version = {
                let state = P::read(&*handle);
                if !state.collection.objects.contains_key(id) {
                    return Err(CoreError::ObjectNotFound {
                        collection: collection.to_owned(),
                        id: id.to_owned(),
                    });
                }
                state.version
            };

            let mut state = P::write(&*handle);
            if state.version != version {
                continue;
            }
            self.append_log_record(LogRecord::Command(Command::ExpireAt {
                collection: collection.to_owned(),
                id: id.to_owned(),
                expires_at_ms,
            }))?;
            let Some(object) = state.collection.objects.get_mut(id) else {
                continue;
            };
            object.expires_at = Some(expires_at_ms);
            state.version = state.version.saturating_add(1);
            return Ok(true);
        }
    }

    pub(crate) fn expire_exclusive(&self, collection: &str, id: &str, seconds: u32) -> Result<()> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        let expires_at_ms = now_millis().saturating_add(u64::from(seconds) * 1_000);
        self.append_log_record(LogRecord::Command(Command::ExpireAt {
            collection: collection.to_owned(),
            id: id.to_owned(),
            expires_at_ms,
        }))?;
        let mut state = P::write(&*handle);
        let object =
            state
                .collection
                .objects
                .get_mut(id)
                .ok_or_else(|| CoreError::ObjectNotFound {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                })?;
        object.expires_at = Some(expires_at_ms);
        state.version = state.version.saturating_add(1);
        Ok(())
    }

    pub(crate) fn try_persist_local(&self, collection: &str, id: &str) -> Result<bool> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;

        loop {
            let version = {
                let state = P::read(&*handle);
                if !state.collection.objects.contains_key(id) {
                    return Err(CoreError::ObjectNotFound {
                        collection: collection.to_owned(),
                        id: id.to_owned(),
                    });
                }
                state.version
            };
            let mut state = P::write(&*handle);
            if state.version != version {
                continue;
            }
            self.append_log_record(LogRecord::Command(Command::Persist {
                collection: collection.to_owned(),
                id: id.to_owned(),
            }))?;
            let Some(object) = state.collection.objects.get_mut(id) else {
                continue;
            };
            object.expires_at = None;
            state.version = state.version.saturating_add(1);
            return Ok(true);
        }
    }

    pub(crate) fn persist_exclusive(&self, collection: &str, id: &str) -> Result<()> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        self.append_log_record(LogRecord::Command(Command::Persist {
            collection: collection.to_owned(),
            id: id.to_owned(),
        }))?;
        let mut state = P::write(&*handle);
        let object =
            state
                .collection
                .objects
                .get_mut(id)
                .ok_or_else(|| CoreError::ObjectNotFound {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                })?;
        object.expires_at = None;
        state.version = state.version.saturating_add(1);
        Ok(())
    }

    pub(crate) fn try_jset_local(
        &self,
        collection: &str,
        id: &str,
        path: &str,
        value: &str,
        raw: bool,
    ) -> Result<bool> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        let payload = if raw {
            serde_json::from_str(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()))
        } else {
            serde_json::Value::String(value.to_owned())
        };

        loop {
            let version =
                {
                    let state = P::read(&*handle);
                    let object = state.collection.objects.get(id).ok_or_else(|| {
                        CoreError::ObjectNotFound {
                            collection: collection.to_owned(),
                            id: id.to_owned(),
                        }
                    })?;
                    if !matches!(object.geo, GeoType::GeoJson(_)) {
                        return Err(CoreError::Message(
                            "JSET requires a GeoJSON object".to_owned(),
                        ));
                    }
                    state.version
                };
            let mut state = P::write(&*handle);
            if state.version != version {
                continue;
            }
            self.append_log_record(LogRecord::Command(Command::Jset {
                collection: collection.to_owned(),
                id: id.to_owned(),
                path: path.to_owned(),
                value: value.to_owned(),
                raw,
            }))?;
            let json = match state
                .collection
                .objects
                .get_mut(id)
                .map(|object| &mut object.geo)
            {
                Some(GeoType::GeoJson(value)) => value,
                Some(_) => {
                    return Err(CoreError::Message(
                        "JSET requires a GeoJSON object".to_owned(),
                    ));
                }
                None => continue,
            };
            set_json_path(json, path, payload.clone())?;
            state.version = state.version.saturating_add(1);
            return Ok(true);
        }
    }

    pub(crate) fn jset_exclusive(
        &self,
        collection: &str,
        id: &str,
        path: &str,
        value: &str,
        raw: bool,
    ) -> Result<()> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        self.append_log_record(LogRecord::Command(Command::Jset {
            collection: collection.to_owned(),
            id: id.to_owned(),
            path: path.to_owned(),
            value: value.to_owned(),
            raw,
        }))?;
        let payload = if raw {
            serde_json::from_str(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()))
        } else {
            serde_json::Value::String(value.to_owned())
        };
        let mut state = P::write(&*handle);
        let json = match state
            .collection
            .objects
            .get_mut(id)
            .map(|object| &mut object.geo)
        {
            Some(GeoType::GeoJson(value)) => value,
            Some(_) => {
                return Err(CoreError::Message(
                    "JSET requires a GeoJSON object".to_owned(),
                ));
            }
            None => {
                return Err(CoreError::ObjectNotFound {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                });
            }
        };
        set_json_path(json, path, payload)?;
        state.version = state.version.saturating_add(1);
        Ok(())
    }

    pub(crate) fn try_jdel_local(
        &self,
        collection: &str,
        id: &str,
        path: &str,
    ) -> Result<Option<bool>> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;

        loop {
            let version =
                {
                    let state = P::read(&*handle);
                    let object = state.collection.objects.get(id).ok_or_else(|| {
                        CoreError::ObjectNotFound {
                            collection: collection.to_owned(),
                            id: id.to_owned(),
                        }
                    })?;
                    if !matches!(object.geo, GeoType::GeoJson(_)) {
                        return Err(CoreError::Message(
                            "JDEL requires a GeoJSON object".to_owned(),
                        ));
                    }
                    state.version
                };
            let mut state = P::write(&*handle);
            if state.version != version {
                continue;
            }
            self.append_log_record(LogRecord::Command(Command::Jdel {
                collection: collection.to_owned(),
                id: id.to_owned(),
                path: path.to_owned(),
            }))?;
            let json = match state
                .collection
                .objects
                .get_mut(id)
                .map(|object| &mut object.geo)
            {
                Some(GeoType::GeoJson(value)) => value,
                Some(_) => {
                    return Err(CoreError::Message(
                        "JDEL requires a GeoJSON object".to_owned(),
                    ));
                }
                None => continue,
            };
            let deleted = delete_json_path(json, path)?;
            state.version = state.version.saturating_add(1);
            return Ok(Some(deleted));
        }
    }

    pub(crate) fn jdel_exclusive(&self, collection: &str, id: &str, path: &str) -> Result<bool> {
        self.ensure_writable()?;
        let handle = self.existing_collection_handle(collection)?;
        self.append_log_record(LogRecord::Command(Command::Jdel {
            collection: collection.to_owned(),
            id: id.to_owned(),
            path: path.to_owned(),
        }))?;
        let mut state = P::write(&*handle);
        let json = match state
            .collection
            .objects
            .get_mut(id)
            .map(|object| &mut object.geo)
        {
            Some(GeoType::GeoJson(value)) => value,
            Some(_) => {
                return Err(CoreError::Message(
                    "JDEL requires a GeoJSON object".to_owned(),
                ));
            }
            None => {
                return Err(CoreError::ObjectNotFound {
                    collection: collection.to_owned(),
                    id: id.to_owned(),
                });
            }
        };
        let deleted = delete_json_path(json, path)?;
        state.version = state.version.saturating_add(1);
        Ok(deleted)
    }

    pub fn set(&self, req: SetRequest) -> Result<bool> {
        {
            let _gate = self.read_control();
            if let Some(result) = self.try_set_local(&req)? {
                return Ok(result);
            }
        }
        let _gate = self.write_control();
        self.set_exclusive(req)
    }

    pub fn get(&self, collection: &str, id: &str, options: GetOptions) -> Result<Option<Object>> {
        let _gate = self.read_control();
        let Some(handle) = self.collection_handle(collection) else {
            return Ok(None);
        };
        let collection = P::read(&*handle);
        let Some(object) = collection.collection.objects.get(id) else {
            return Ok(None);
        };
        if is_expired(object) {
            return Ok(None);
        }
        Ok(Some(project_object_ref(
            object,
            options.output,
            options.with_fields,
        )?))
    }

    pub fn del(&self, collection: &str, id: &str) -> Result<bool> {
        {
            let _gate = self.read_control();
            if let Some(result) = self.try_del_local(collection, id)? {
                return Ok(result);
            }
        }
        let _gate = self.write_control();
        self.del_exclusive(collection, id)
    }

    pub(crate) fn del_unguarded(&self, collection: &str, id: &str) -> Result<bool> {
        self.del_exclusive(collection, id)
    }

    pub(crate) fn del_exclusive(&self, collection: &str, id: &str) -> Result<bool> {
        self.ensure_writable()?;
        let before = self.collection_handle(collection).and_then(|handle| {
            let guard = P::read(&*handle);
            guard.collection.objects.get(id).cloned()
        });
        if before.is_none() {
            return Ok(false);
        }
        let event = MutationEvent {
            command: MutationCommand::Del,
            collection: collection.to_owned(),
            id: id.to_owned(),
            before,
            after: None,
            timestamp_ns: now_nanos(),
        };
        let planned = self.plan_mutation(&event)?;
        let webhook_records =
            self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?;
        self.persist_command_batch(
            Command::Del {
                collection: collection.to_owned(),
                id: id.to_owned(),
            },
            &webhook_records,
        )?;
        let Some(handle) = self.collection_handle(collection) else {
            return Ok(false);
        };
        let removed = {
            let mut state = P::write(&*handle);
            let removed = state.collection.remove(id).is_some();
            if removed {
                state.version = state.version.saturating_add(1);
            }
            removed
        };
        P::write(&self.geofences).apply_prepared_mutation(planned);
        Ok(removed)
    }

    pub fn pdel(&self, collection: &str, pattern: &str) -> Result<u64> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        let ids = self
            .collection_handle(collection)
            .map(|handle| {
                let state = P::read(&*handle);
                state
                    .collection
                    .objects
                    .keys()
                    .filter(|id| glob_match(pattern, id))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut removed = 0;
        for id in ids {
            removed += u64::from(self.del_unguarded(collection, &id)?);
        }
        Ok(removed)
    }

    pub fn create_collection(&self, collection: &str) -> Result<bool> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        if self.collection_handle(collection).is_some() {
            return Ok(false);
        }
        self.append_log_record(LogRecord::Command(Command::CreateCollection {
            collection: collection.to_owned(),
        }))?;
        self.ensure_collection_cell(collection);
        Ok(true)
    }

    pub fn drop_collection(&self, collection: &str) -> Result<bool> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        let Some(handle) = self.collection_handle(collection) else {
            return Ok(false);
        };
        let previous = {
            let state = P::read(&*handle);
            state.collection.clone()
        };
        let events = previous
            .objects
            .values()
            .map(|object| MutationEvent {
                command: MutationCommand::Drop,
                collection: collection.to_owned(),
                id: object.id.clone(),
                before: Some(object.clone()),
                after: None,
                timestamp_ns: now_nanos(),
            })
            .collect::<Vec<_>>();
        let mut prepared = Vec::with_capacity(events.len());
        let mut webhook_records = Vec::new();
        for event in &events {
            let planned = self.plan_mutation(event)?;
            webhook_records.extend(
                self.webhook_records_for_prepared(&planned, self.current_webhook_retry_count())?,
            );
            prepared.push(planned);
        }
        self.persist_command_batch(
            Command::DropCollection {
                collection: collection.to_owned(),
            },
            &webhook_records,
        )?;
        P::write(&self.collections).remove(collection);
        let mut geofences = P::write(&self.geofences);
        for planned in prepared {
            geofences.apply_prepared_mutation(planned);
        }
        Ok(true)
    }

    pub fn rename(&self, collection: &str, new_name: &str) -> Result<()> {
        self.rename_inner(collection, new_name, false).map(|_| ())
    }

    pub fn renamenx(&self, collection: &str, new_name: &str) -> Result<bool> {
        self.rename_inner(collection, new_name, true)
    }

    pub(crate) fn rename_inner(
        &self,
        collection: &str,
        new_name: &str,
        nx_only: bool,
    ) -> Result<bool> {
        let _gate = self.write_control();
        self.ensure_writable()?;
        let mut collections = P::write(&self.collections);
        if nx_only && collections.contains_key(new_name) {
            return Ok(false);
        }
        let Some(handle) = collections.get(collection).cloned() else {
            return Err(CoreError::CollectionNotFound(collection.to_owned()));
        };
        self.append_log_record(LogRecord::Command(Command::Rename {
            collection: collection.to_owned(),
            new_name: new_name.to_owned(),
        }))?;
        collections.remove(collection);
        {
            let mut state = P::write(&*handle);
            state.collection.name = new_name.to_owned();
            state.version = state.version.saturating_add(1);
        }
        collections.insert(new_name.to_owned(), handle);
        Ok(true)
    }

    pub fn fset(
        &self,
        collection: &str,
        id: &str,
        fields: &[FieldEntry],
        xx: bool,
    ) -> Result<bool> {
        {
            let _gate = self.read_control();
            if let Some(result) = self.try_fset_local(collection, id, fields, xx)? {
                return Ok(result);
            }
        }
        let _gate = self.write_control();
        self.fset_exclusive(collection, id, fields, xx)
    }

    pub fn fget(&self, collection: &str, id: &str, field: &str) -> Result<Option<FieldValue>> {
        let _gate = self.read_control();
        Ok(self
            .get_live_object(collection, id)?
            .and_then(|object| object.fields.get(field).cloned()))
    }

    pub fn expire(&self, collection: &str, id: &str, seconds: u32) -> Result<()> {
        {
            let _gate = self.read_control();
            if self.try_expire_local(collection, id, seconds)? {
                return Ok(());
            }
        }
        let _gate = self.write_control();
        self.expire_exclusive(collection, id, seconds)
    }

    pub fn persist(&self, collection: &str, id: &str) -> Result<()> {
        {
            let _gate = self.read_control();
            if self.try_persist_local(collection, id)? {
                return Ok(());
            }
        }
        let _gate = self.write_control();
        self.persist_exclusive(collection, id)
    }

    pub fn ttl(&self, collection: &str, id: &str) -> Result<Option<i32>> {
        let _gate = self.read_control();
        let Some(object) = self.get_live_object(collection, id)? else {
            return Ok(None);
        };
        Ok(object.expires_at.map(|deadline| {
            let remaining = deadline.saturating_sub(now_millis()) / 1_000;
            i32::try_from(remaining).unwrap_or(i32::MAX)
        }))
    }

    pub fn exists(&self, collection: &str, id: &str) -> Result<bool> {
        let _gate = self.read_control();
        Ok(self.get_live_object(collection, id)?.is_some())
    }

    pub fn fexists(&self, collection: &str, id: &str, field: &str) -> Result<bool> {
        let _gate = self.read_control();
        Ok(self
            .get_live_object(collection, id)?
            .is_some_and(|object| object.fields.contains_key(field)))
    }

    pub fn bounds(&self, collection: &str) -> Result<Option<BoundingBox>> {
        let _gate = self.read_control();
        let Some(handle) = self.collection_handle(collection) else {
            return Ok(None);
        };
        P::read(&*handle).collection.bounds()
    }

    pub fn collections(&self, pattern: &str) -> Result<Vec<String>> {
        let _gate = self.read_control();
        let collections = P::read(&self.collections);
        let mut items = collections
            .keys()
            .filter(|name| glob_match(pattern, name))
            .cloned()
            .collect::<Vec<_>>();
        items.sort();
        Ok(items)
    }

    pub fn stats(&self, collections: &[&str]) -> Result<Vec<CollectionStats>> {
        let _gate = self.read_control();
        let mut items = Vec::new();
        for name in collections {
            if let Some(handle) = self.collection_handle(name) {
                let collection = P::read(&*handle);
                let mut stats = CollectionStats {
                    name: collection.collection.name.clone(),
                    object_count: 0,
                    point_count: 0,
                    string_count: 0,
                    expires_count: 0,
                };
                for object in collection
                    .collection
                    .objects
                    .values()
                    .filter(|object| !is_expired(object))
                {
                    stats.object_count += 1;
                    if matches!(object.geo, GeoType::Point { .. }) {
                        stats.point_count += 1;
                    }
                    if matches!(object.geo, GeoType::String(_)) {
                        stats.string_count += 1;
                    }
                    if object.expires_at.is_some() {
                        stats.expires_count += 1;
                    }
                }
                items.push(stats);
            }
        }
        Ok(items)
    }

    pub fn jset(
        &self,
        collection: &str,
        id: &str,
        path: &str,
        value: &str,
        raw: bool,
    ) -> Result<()> {
        {
            let _gate = self.read_control();
            if self.try_jset_local(collection, id, path, value, raw)? {
                return Ok(());
            }
        }
        let _gate = self.write_control();
        self.jset_exclusive(collection, id, path, value, raw)
    }

    pub fn jget(&self, collection: &str, id: &str, path: &str) -> Result<Option<String>> {
        let _gate = self.read_control();
        let Some(object) = self.get_live_object(collection, id)? else {
            return Ok(None);
        };
        let Some(json) = object.geo.json_value() else {
            return Ok(None);
        };
        Ok(get_json_path(json, path).map(|value| value.to_string()))
    }

    pub fn jdel(&self, collection: &str, id: &str, path: &str) -> Result<bool> {
        {
            let _gate = self.read_control();
            if let Some(result) = self.try_jdel_local(collection, id, path)? {
                return Ok(result);
            }
        }
        let _gate = self.write_control();
        self.jdel_exclusive(collection, id, path)
    }
}
