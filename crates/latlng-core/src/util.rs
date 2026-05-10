use super::*;

pub(crate) fn field_entries_to_map(entries: &[FieldEntry]) -> FieldMap {
    let mut map = FieldMap::new();
    for entry in entries {
        map.insert(entry.name.clone(), entry.value.clone());
    }
    map
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
        let expired = collection
            .collection
            .objects
            .iter()
            .filter(|(_, object)| object.expires_at.is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
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
                .ok_or_else(|| CoreError::Message("object is not spatial".to_owned()))?,
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
        .ok_or_else(|| CoreError::Message("object is not spatial".to_owned()))?;
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
