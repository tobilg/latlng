use super::*;

pub(crate) fn apply_persisted_entry_to_state<P: Platform>(
    collections: &P::RwLock<HashMap<String, CollectionHandle<P>>>,
    geofences: &P::RwLock<GeofenceRegistry<P>>,
    entry: StorageEntry,
) -> Result<()> {
    match decode_log_record(&entry.command)? {
        LogRecord::Command(command) => {
            apply_persisted_command_to_state::<P>(collections, geofences, command)
        }
        LogRecord::WebhookEnqueue(_)
        | LogRecord::WebhookAck(_)
        | LogRecord::WebhookRetryScheduled(_)
        | LogRecord::WebhookDeadLetter(_) => Ok(()),
    }
}

pub(crate) fn apply_persisted_command_to_state<P: Platform>(
    collections: &P::RwLock<HashMap<String, CollectionHandle<P>>>,
    geofences: &P::RwLock<GeofenceRegistry<P>>,
    command: Command,
) -> Result<()> {
    match command {
        Command::Set(request) => {
            let collection_name = request.collection.clone();
            let expires_at = request
                .expire_seconds
                .map(|seconds| now_millis().saturating_add(u64::from(seconds) * 1_000));
            let object = Object {
                id: request.id.clone(),
                geo: request.object,
                fields: field_entries_to_map(&request.fields),
                expires_at,
            };
            let handle = ensure_collection_cell_in_catalog::<P>(collections, &collection_name);
            let mut collection = P::write(&*handle);
            collection.collection.upsert(object)?;
            collection.version = collection.version.saturating_add(1);
        }
        Command::Del { collection, id } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                collection_state.collection.remove(&id);
                collection_state.version = collection_state.version.saturating_add(1);
            }
        }
        Command::DropCollection { collection } => {
            P::write(collections).remove(&collection);
        }
        Command::Rename {
            collection,
            new_name,
        } => {
            let mut all = P::write(collections);
            if let Some(handle) = all.remove(&collection) {
                {
                    let mut state = P::write(&*handle);
                    state.collection.name = new_name.clone();
                    state.version = state.version.saturating_add(1);
                }
                all.insert(new_name, handle);
            }
        }
        Command::Fset {
            collection,
            id,
            fields,
            ..
        } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                if let Some(object) = collection_state.collection.objects.get_mut(&id) {
                    for field in fields {
                        object.fields.insert(field.name, field.value);
                    }
                    collection_state.version = collection_state.version.saturating_add(1);
                }
            }
        }
        Command::Expire {
            collection,
            id,
            seconds,
        } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                if let Some(object) = collection_state.collection.objects.get_mut(&id) {
                    object.expires_at =
                        Some(now_millis().saturating_add(u64::from(seconds) * 1_000));
                    collection_state.version = collection_state.version.saturating_add(1);
                }
            }
        }
        Command::SetPersisted(record) => {
            let object = Object {
                id: record.id.clone(),
                geo: record.object,
                fields: field_entries_to_map(&record.fields),
                expires_at: record.expires_at_ms,
            };
            let handle = ensure_collection_cell_in_catalog::<P>(collections, &record.collection);
            let mut collection = P::write(&*handle);
            collection.collection.upsert(object)?;
            collection.version = collection.version.saturating_add(1);
        }
        Command::ExpireAt {
            collection,
            id,
            expires_at_ms,
        } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                if let Some(object) = collection_state.collection.objects.get_mut(&id) {
                    object.expires_at = Some(expires_at_ms);
                    collection_state.version = collection_state.version.saturating_add(1);
                }
            }
        }
        Command::CreateCollection { collection } => {
            ensure_collection_cell_in_catalog::<P>(collections, &collection);
        }
        Command::Persist { collection, id } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                if let Some(object) = collection_state.collection.objects.get_mut(&id) {
                    object.expires_at = None;
                    collection_state.version = collection_state.version.saturating_add(1);
                }
            }
        }
        Command::Jset {
            collection,
            id,
            path,
            value,
            raw,
        } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                if let Some(GeoType::GeoJson(json)) = collection_state
                    .collection
                    .objects
                    .get_mut(&id)
                    .map(|object| &mut object.geo)
                {
                    let payload = if raw {
                        serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value))
                    } else {
                        serde_json::Value::String(value)
                    };
                    let _ = set_json_path(json, &path, payload);
                    collection_state.version = collection_state.version.saturating_add(1);
                }
            }
        }
        Command::Jdel {
            collection,
            id,
            path,
        } => {
            if let Some(handle) = collection_handle_from_catalog::<P>(collections, &collection) {
                let mut collection_state = P::write(&*handle);
                if let Some(GeoType::GeoJson(json)) = collection_state
                    .collection
                    .objects
                    .get_mut(&id)
                    .map(|object| &mut object.geo)
                {
                    let _ = delete_json_path(json, &path);
                    collection_state.version = collection_state.version.saturating_add(1);
                }
            }
        }
        Command::SetHook {
            name,
            endpoint,
            def,
        } => {
            P::write(geofences).set_hook(name, endpoint, def);
        }
        Command::DelHook { name } => {
            P::write(geofences).del_hook(&name);
        }
        Command::PDelHook { pattern } => {
            P::write(geofences).pdel_hook(&pattern);
        }
        Command::SetChannel { name, def } => {
            P::write(geofences).set_channel(name, def);
        }
        Command::DelChannel { name } => {
            P::write(geofences).del_channel(&name);
        }
        Command::PDelChannel { pattern } => {
            P::write(geofences).pdel_channel(&pattern);
        }
        Command::FlushDb => {
            P::write(collections).clear();
            P::write(geofences).clear_all();
        }
    }
    Ok(())
}
