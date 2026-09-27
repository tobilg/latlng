use super::*;

pub(crate) fn field_entries_to_map(entries: &[FieldEntry]) -> FieldMap {
    let mut map = FieldMap::new();
    for entry in entries {
        map.insert(entry.name.clone(), entry.value.clone());
    }
    map
}

/// A numeric `0` means "no value": setting a field to `0` deletes
/// it, and a missing field reads as `0` in queries.
pub(crate) fn is_zero_field_value(value: &FieldValue) -> bool {
    matches!(value, FieldValue::Number(number) if *number == 0.0)
}

/// Applies field updates: fields not mentioned are kept, given
/// fields are added or overwritten, and fields set to `0` are removed.
pub(crate) fn apply_field_updates(fields: &mut FieldMap, updates: &[FieldEntry]) {
    for entry in updates {
        if is_zero_field_value(&entry.value) {
            fields.remove(&entry.name);
        } else {
            fields.insert(entry.name.clone(), entry.value.clone());
        }
    }
}

/// The fields an object has after a SET: the existing object's fields with
/// the request's fields applied on top.
pub(crate) fn merged_set_fields(existing: Option<&Object>, updates: &[FieldEntry]) -> FieldMap {
    let mut fields = existing
        .map(|object| object.fields.clone())
        .unwrap_or_default();
    apply_field_updates(&mut fields, updates);
    fields
}

pub(crate) fn field_map_to_entries(fields: &FieldMap) -> Vec<FieldEntry> {
    fields
        .iter()
        .map(|(name, value)| FieldEntry {
            name: name.to_owned(),
            value: value.clone(),
        })
        .collect()
}

pub(crate) fn normalize_command_key(command: &str) -> String {
    command.trim().to_ascii_lowercase()
}

pub(crate) fn gc_collections_locked<P: Platform>(
    collections: &P::RwLock<HashMap<String, CollectionHandle<P>>>,
) {
    let now = now_millis();
    let handles = {
        let collections = P::read(collections);
        collections
            .iter()
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect::<Vec<_>>()
    };
    for (_, handle) in handles {
        let mut collection = P::write(&*handle);
        let expired = collection.collection.due_expirations(now, usize::MAX);
        if !expired.is_empty() {
            for id in expired {
                collection.collection.remove(&id);
            }
            collection.version = collection.version.saturating_add(1);
        }
    }
}

pub(crate) fn project_object_ref(
    object: &Object,
    output: OutputFormat,
    with_fields: bool,
) -> Result<Object> {
    Ok(Object {
        id: object.id.clone(),
        geo: project_geo(&object.id, &object.geo, output)?,
        fields: if with_fields {
            object.fields.clone()
        } else {
            FieldMap::new()
        },
        expires_at: object.expires_at,
    })
}

fn project_geo(id: &str, geo: &GeoType, output: OutputFormat) -> Result<GeoType> {
    Ok(match output {
        OutputFormat::Objects => geo.clone(),
        OutputFormat::Points => {
            let (lat, lon) = projected_point_coordinates(geo)?;
            GeoType::Point { lat, lon, z: None }
        }
        OutputFormat::Bounds => GeoType::Bounds(
            geo.envelope()?
                .ok_or_else(|| CoreError::Invalid("object is not spatial".to_owned()))?,
        ),
        OutputFormat::Hashes { precision } => {
            let (lat, lon) = projected_point_coordinates(geo)?;
            GeoType::Hash(latlng_geo::encode_geohash(lat, lon, precision as usize))
        }
        OutputFormat::Ids | OutputFormat::Count => GeoType::String(id.to_owned()),
    })
}

fn projected_point_coordinates(geo: &GeoType) -> Result<(f64, f64)> {
    if let Some((lat, lon)) = geo.point_coordinates() {
        return Ok((lat, lon));
    }
    let bounds = geo
        .envelope()?
        .ok_or_else(|| CoreError::Invalid("object is not spatial".to_owned()))?;
    Ok(bounds.center())
}

pub(crate) fn is_expired(object: &Object) -> bool {
    object
        .expires_at
        .is_some_and(|deadline| deadline <= now_millis())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn now_millis() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn now_nanos() -> u64 {
    (js_sys::Date::now() * 1_000_000.0) as u64
}

/// Rejects geofence definitions that could never be evaluated. Reference areas
/// are only resolved by the search paths, so a fence cannot use one. This is
/// the only check applied when replaying the log.
pub(crate) fn ensure_geofence_evaluable(def: &GeofenceDef) -> Result<()> {
    match &def.query {
        GeofenceQuery::Within { area, .. } | GeofenceQuery::Intersects { area, .. } => {
            if matches!(area, Area::Reference { .. }) {
                return Err(CoreError::Invalid(
                    "reference areas are not supported in hooks or channels; use a GeoJSON, bounds, or circle area instead".to_owned(),
                ));
            }
        }
        GeofenceQuery::Nearby { .. } | GeofenceQuery::Roam { .. } => {}
    }
    Ok(())
}

/// Full validation for newly registered hooks and channels: evaluability,
/// search options, coordinates, radii and bearings.
pub(crate) fn validate_geofence_def(def: &GeofenceDef) -> Result<()> {
    ensure_geofence_evaluable(def)?;
    match &def.query {
        GeofenceQuery::Nearby {
            lat,
            lon,
            meters,
            options,
        } => {
            validate_lat_lon(*lat, *lon)?;
            validate_meters(*meters)?;
            options.validate()?;
        }
        GeofenceQuery::Within { area, options } | GeofenceQuery::Intersects { area, options } => {
            area.validate()?;
            options.validate()?;
        }
        GeofenceQuery::Roam {
            meters, options, ..
        } => {
            validate_meters(*meters)?;
            options.validate()?;
        }
    }
    Ok(())
}
