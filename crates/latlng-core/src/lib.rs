#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use glob_match::glob_match;
use hashbrown::HashMap;
use latlng_geo::{
    Area, BoundingBox, FieldMap, FieldValue, GeoError, GeoType, Object, delete_json_path,
    get_json_path, set_json_path,
};
use latlng_geofence::{
    ChannelDef, DEFAULT_SUBSCRIBER_QUEUE_CAPACITY, GeofenceDef, GeofenceEvent,
    GeofenceEventReceiver, GeofenceRegistry, HookDef, HookInfo, MutationCommand, MutationEvent,
};
#[cfg(feature = "parallel")]
use latlng_index::{
    AreaPredicate, area_candidates_owned, nearby_candidates_owned, snapshot_candidates_owned,
    string_snapshot_candidates_owned,
};
use latlng_index::{
    IndexError, OutputFormat, SearchItem, SearchOptions, SearchResults, SpatialIndex,
    WhereComparison, apply_search_options, matches_candidate,
};
#[cfg(not(feature = "parallel"))]
use latlng_index::{SearchCandidate, nearby_candidates};
use latlng_platform::{NativePlatform, Platform, WasmPlatform};
use latlng_storage::{CompactionResult, StorageBackend, StorageEntry, StorageError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use latlng_geo as geo;
pub use latlng_geofence as geofence;
pub use latlng_index as index;
pub use latlng_platform as platform;
pub use latlng_storage as storage;

pub type Result<T> = std::result::Result<T, CoreError>;

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const API_VERSION: &str = "v1";
pub const PROTOCOL_VERSION: &str = "capnp-v1";
pub const STORAGE_FORMAT_VERSION: &str = "storage-v1";

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Geo(#[from] GeoError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("collection not found: {0}")]
    CollectionNotFound(String),
    #[error("object not found: {collection}/{id}")]
    ObjectNotFound { collection: String, id: String },
    #[error("read-only mode is enabled")]
    ReadOnly,
    #[error("builder is missing a storage backend")]
    MissingStorage,
    #[error("invalid command payload: {0}")]
    Codec(String),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub read_only: bool,
    #[serde(default)]
    pub command_timeouts: HashMap<String, f64>,
    #[serde(default = "default_subscriber_queue_capacity")]
    pub subscriber_queue_capacity: usize,
    #[serde(default = "default_webhook_retry_count")]
    pub webhook_retry_count: u32,
    #[serde(skip)]
    pub config_file: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            read_only: false,
            command_timeouts: HashMap::new(),
            subscriber_queue_capacity: default_subscriber_queue_capacity(),
            webhook_retry_count: default_webhook_retry_count(),
            config_file: None,
        }
    }
}

impl Config {
    pub fn timeout_for(&self, command: &str) -> Option<f64> {
        self.command_timeouts
            .get(&normalize_command_key(command))
            .copied()
    }

    pub fn set_timeout(&mut self, command: &str, seconds: f64) {
        self.command_timeouts
            .insert(normalize_command_key(command), seconds.max(0.0));
    }

    pub fn clear_timeout(&mut self, command: &str) {
        self.command_timeouts
            .remove(&normalize_command_key(command));
    }
}

pub fn default_subscriber_queue_capacity() -> usize {
    DEFAULT_SUBSCRIBER_QUEUE_CAPACITY
}

pub fn default_webhook_retry_count() -> u32 {
    8
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldEntry {
    pub name: String,
    pub value: FieldValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetCondition {
    Always,
    Nx,
    Xx,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetRequest {
    pub collection: String,
    pub id: String,
    pub object: GeoType,
    pub fields: Vec<FieldEntry>,
    pub expire_seconds: Option<u32>,
    pub condition: SetCondition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedSetRecord {
    pub collection: String,
    pub id: String,
    pub object: GeoType,
    pub fields: Vec<FieldEntry>,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetOptions {
    pub with_fields: bool,
    pub output: OutputFormat,
}

impl Default for GetOptions {
    fn default() -> Self {
        Self {
            with_fields: false,
            output: OutputFormat::Objects,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NearbyQuery {
    pub lat: f64,
    pub lon: f64,
    pub meters: f64,
    #[serde(default)]
    pub options: SearchOptions,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CollectionStats {
    pub name: String,
    pub object_count: u64,
    pub point_count: u64,
    pub string_count: u64,
    pub expires_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub protocol_version: String,
    #[serde(default)]
    pub storage_format_version: String,
    pub num_collections: u32,
    pub num_objects: u64,
    pub num_points: u64,
    pub heap_bytes: u64,
    pub read_only: bool,
    pub leader: bool,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub following: Option<String>,
    #[serde(default)]
    pub caught_up: bool,
    #[serde(default)]
    pub caught_up_once: bool,
    #[serde(default)]
    pub last_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookEnqueueRecord {
    pub job_id: String,
    pub event: GeofenceEvent,
    pub endpoint: String,
    pub attempts_used: u32,
    pub max_attempts: u32,
    pub next_attempt_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookAckRecord {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookRetryScheduledRecord {
    pub job_id: String,
    pub attempts_used: u32,
    pub next_attempt_at_ms: u64,
    pub last_error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookDeadLetterRecord {
    pub job_id: String,
    pub attempts_used: u32,
    pub last_error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LogRecord {
    Command(Command),
    WebhookEnqueue(WebhookEnqueueRecord),
    WebhookAck(WebhookAckRecord),
    WebhookRetryScheduled(WebhookRetryScheduledRecord),
    WebhookDeadLetter(WebhookDeadLetterRecord),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Set(SetRequest),
    Del {
        collection: String,
        id: String,
    },
    DropCollection {
        collection: String,
    },
    Rename {
        collection: String,
        new_name: String,
    },
    Fset {
        collection: String,
        id: String,
        fields: Vec<FieldEntry>,
        xx: bool,
    },
    Expire {
        collection: String,
        id: String,
        seconds: u32,
    },
    Persist {
        collection: String,
        id: String,
    },
    Jset {
        collection: String,
        id: String,
        path: String,
        value: String,
        raw: bool,
    },
    Jdel {
        collection: String,
        id: String,
        path: String,
    },
    SetHook {
        name: String,
        endpoint: String,
        def: GeofenceDef,
    },
    DelHook {
        name: String,
    },
    PDelHook {
        pattern: String,
    },
    SetChannel {
        name: String,
        def: GeofenceDef,
    },
    DelChannel {
        name: String,
    },
    PDelChannel {
        pattern: String,
    },
    FlushDb,
    SetPersisted(PersistedSetRecord),
    ExpireAt {
        collection: String,
        id: String,
        expires_at_ms: u64,
    },
    CreateCollection {
        collection: String,
    },
}

#[derive(Debug, Clone)]
pub struct Collection {
    pub name: String,
    objects: HashMap<String, Object>,
    spatial_index: SpatialIndex,
    field_indexes: FieldIndexes,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            objects: HashMap::new(),
            spatial_index: SpatialIndex::new(),
            field_indexes: FieldIndexes::default(),
        }
    }

    fn upsert(&mut self, object: Object) -> Result<()> {
        if let Some(previous) = self.objects.get(&object.id) {
            self.field_indexes.remove_object(previous);
        }
        self.spatial_index.remove(&object.id);
        self.spatial_index.insert(object.id.clone(), &object.geo)?;
        self.field_indexes.insert_object(&object);
        self.objects.insert(object.id.clone(), object);
        Ok(())
    }

    fn remove(&mut self, id: &str) -> Option<Object> {
        self.spatial_index.remove(id);
        let removed = self.objects.remove(id);
        if let Some(object) = removed.as_ref() {
            self.field_indexes.remove_object(object);
        }
        removed
    }

    fn insert_fields(&mut self, id: &str, fields: &[FieldEntry]) -> bool {
        let Some(object) = self.objects.get_mut(id) else {
            return false;
        };
        self.field_indexes.remove_object(object);
        for field in fields {
            object
                .fields
                .insert(field.name.clone(), field.value.clone());
        }
        self.field_indexes.insert_object(object);
        true
    }

    fn indexed_candidate_ids(&self, options: &SearchOptions) -> Option<Vec<String>> {
        self.field_indexes.candidate_ids(options)
    }

    fn fast_indexed_results(
        &self,
        options: &SearchOptions,
        string_objects_only: bool,
    ) -> Result<Option<SearchResults>> {
        if !options.fast_limited_ids() {
            return Ok(None);
        }

        let limit = options.limit.max(1) as usize;
        let mut results = Vec::with_capacity(limit);
        let mut error = None;
        let used_index = self.field_indexes.visit_candidate_ids(options, |id| {
            let Some(object) = self.objects.get(id) else {
                return true;
            };
            if is_expired(object)
                || (string_objects_only && !matches!(object.geo, GeoType::String(_)))
            {
                return true;
            }
            let matched = match matches_candidate(object, options) {
                Ok(matched) => matched,
                Err(err) => {
                    error = Some(err);
                    return false;
                }
            };
            if !matched {
                return true;
            }
            results.push(SearchItem {
                id: object.id.clone(),
                object: None,
                fields: None,
                distance_meters: None,
            });
            results.len() < limit
        });

        if let Some(error) = error {
            return Err(error.into());
        }
        Ok(used_index.then_some(SearchResults {
            count: results.len() as u32,
            cursor: 0,
            results,
        }))
    }

    fn bounds(&self) -> Result<Option<BoundingBox>> {
        let mut bounds: Option<BoundingBox> = None;
        for object in self.objects.values() {
            if let Some(object_bounds) = object.envelope()? {
                bounds = Some(match bounds {
                    Some(existing) => existing.union(object_bounds),
                    None => object_bounds,
                });
            }
        }
        Ok(bounds)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OrderedF64(f64);

impl Eq for OrderedF64 {}

impl PartialOrd for OrderedF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Debug, Clone, Default)]
struct FieldIndexes {
    numeric: HashMap<String, BTreeMap<OrderedF64, BTreeSet<String>>>,
    text: HashMap<String, BTreeMap<String, BTreeSet<String>>>,
}

impl FieldIndexes {
    fn insert_object(&mut self, object: &Object) {
        for (field, value) in object.fields.iter() {
            match value {
                FieldValue::Number(number) => {
                    self.numeric
                        .entry(field.to_owned())
                        .or_default()
                        .entry(OrderedF64(*number))
                        .or_default()
                        .insert(object.id.clone());
                }
                FieldValue::Text(text) | FieldValue::Json(text) => {
                    self.text
                        .entry(field.to_owned())
                        .or_default()
                        .entry(text.clone())
                        .or_default()
                        .insert(object.id.clone());
                }
            }
        }
    }

    fn remove_object(&mut self, object: &Object) {
        for (field, value) in object.fields.iter() {
            match value {
                FieldValue::Number(number) => {
                    remove_indexed_id(&mut self.numeric, field, &OrderedF64(*number), &object.id);
                }
                FieldValue::Text(text) | FieldValue::Json(text) => {
                    remove_indexed_id(&mut self.text, field, text, &object.id);
                }
            }
        }
    }

    fn candidate_ids(&self, options: &SearchOptions) -> Option<Vec<String>> {
        let mut selected: Option<BTreeSet<String>> = None;
        let mut used_index = false;

        for filter in &options.where_filters {
            let Some(ids) = self.ids_for_where_filter(filter) else {
                continue;
            };
            used_index = true;
            intersect_selected(&mut selected, ids);
        }

        for filter in &options.where_in_filters {
            let Some(ids) = self.ids_for_where_in_filter(filter) else {
                continue;
            };
            used_index = true;
            intersect_selected(&mut selected, ids);
        }

        used_index.then(|| selected.unwrap_or_default().into_iter().collect())
    }

    fn visit_candidate_ids(
        &self,
        options: &SearchOptions,
        mut visit: impl FnMut(&str) -> bool,
    ) -> bool {
        for filter in &options.where_filters {
            if self.visit_where_filter_ids(filter, &mut visit) {
                return true;
            }
        }

        for filter in &options.where_in_filters {
            if self.visit_where_in_filter_ids(filter, &mut visit) {
                return true;
            }
        }

        false
    }

    fn visit_where_filter_ids(
        &self,
        filter: &latlng_index::WhereFilter,
        visit: &mut impl FnMut(&str) -> bool,
    ) -> bool {
        if !is_top_level_field(&filter.field) {
            return false;
        }
        match &filter.comparison {
            WhereComparison::Range { min, max } => {
                let Some(range) = self.numeric.get(&filter.field) else {
                    return true;
                };
                let min = OrderedF64(*min);
                let max = OrderedF64(*max);
                for (_, ids) in range.range(min..=max) {
                    for id in ids {
                        if !visit(id) {
                            return true;
                        }
                    }
                }
                true
            }
            WhereComparison::EqualsText(expected) => {
                if let Some(ids) = self
                    .text
                    .get(&filter.field)
                    .and_then(|values| values.get(expected))
                {
                    for id in ids {
                        if !visit(id) {
                            return true;
                        }
                    }
                }
                true
            }
            WhereComparison::Regex(_) => false,
        }
    }

    fn visit_where_in_filter_ids(
        &self,
        filter: &latlng_index::WhereInFilter,
        visit: &mut impl FnMut(&str) -> bool,
    ) -> bool {
        let Some(ids) = self.ids_for_where_in_filter(filter) else {
            return false;
        };
        for id in &ids {
            if !visit(id) {
                break;
            }
        }
        true
    }

    fn ids_for_where_filter(&self, filter: &latlng_index::WhereFilter) -> Option<BTreeSet<String>> {
        if !is_top_level_field(&filter.field) {
            return None;
        }
        match &filter.comparison {
            WhereComparison::Range { min, max } => {
                let range = self.numeric.get(&filter.field)?;
                let min = OrderedF64(*min);
                let max = OrderedF64(*max);
                let mut out = BTreeSet::new();
                for (_, ids) in range.range(min..=max) {
                    out.extend(ids.iter().cloned());
                }
                Some(out)
            }
            WhereComparison::EqualsText(expected) => Some(
                self.text
                    .get(&filter.field)
                    .and_then(|values| values.get(expected))
                    .cloned()
                    .unwrap_or_default(),
            ),
            WhereComparison::Regex(_) => None,
        }
    }

    fn ids_for_where_in_filter(
        &self,
        filter: &latlng_index::WhereInFilter,
    ) -> Option<BTreeSet<String>> {
        if !is_top_level_field(&filter.field) {
            return None;
        }
        let mut out = BTreeSet::new();
        if let Some(text_values) = self.text.get(&filter.field) {
            for value in &filter.values {
                if let Some(ids) = text_values.get(value) {
                    out.extend(ids.iter().cloned());
                }
            }
        }
        if let Some(numeric_values) = self.numeric.get(&filter.field) {
            for value in &filter.values {
                if let Ok(number) = value.parse::<f64>()
                    && let Some(ids) = numeric_values.get(&OrderedF64(number))
                {
                    out.extend(ids.iter().cloned());
                }
            }
        }
        Some(out)
    }
}

fn is_top_level_field(field: &str) -> bool {
    field != "z" && !field.contains('.')
}

fn intersect_selected(selected: &mut Option<BTreeSet<String>>, ids: BTreeSet<String>) {
    match selected {
        Some(existing) => {
            *existing = existing.intersection(&ids).cloned().collect();
        }
        None => *selected = Some(ids),
    }
}

fn remove_indexed_id<K>(
    indexes: &mut HashMap<String, BTreeMap<K, BTreeSet<String>>>,
    field: &str,
    value: &K,
    id: &str,
) where
    K: Ord + Clone,
{
    let Some(values) = indexes.get_mut(field) else {
        return;
    };
    let should_remove_value = if let Some(ids) = values.get_mut(value) {
        ids.remove(id);
        ids.is_empty()
    } else {
        false
    };
    if should_remove_value {
        values.remove(value);
    }
    if values.is_empty() {
        indexes.remove(field);
    }
}

#[derive(Debug, Clone)]
struct VersionedCollection {
    version: u64,
    collection: Collection,
}

impl VersionedCollection {
    fn new(name: impl Into<String>) -> Self {
        Self {
            version: 0,
            collection: Collection::new(name),
        }
    }
}

pub struct LatLngBuilder<P: Platform, S: StorageBackend> {
    storage: Option<S>,
    config: Config,
    _platform: PhantomData<P>,
}

impl<P: Platform, S: StorageBackend> Default for LatLngBuilder<P, S> {
    fn default() -> Self {
        Self {
            storage: None,
            config: Config::default(),
            _platform: PhantomData,
        }
    }
}

impl<P: Platform, S: StorageBackend> LatLngBuilder<P, S> {
    pub fn storage(mut self, storage: S) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    pub fn build(self) -> Result<LatLng<P, S>> {
        let storage = self.storage.ok_or(CoreError::MissingStorage)?;
        let collections = P::new_rwlock(HashMap::new());
        let geofences = P::new_rwlock(GeofenceRegistry::with_subscriber_queue_capacity(
            self.config.subscriber_queue_capacity,
        ));
        let mut last_sequence = 0;
        storage.replay(0, &mut |entry| {
            last_sequence = entry.sequence;
            apply_persisted_entry_to_state::<P>(&collections, &geofences, entry)
                .map_err(|error| StorageError::Message(error.to_string()))
        })?;

        let db = LatLng {
            control: P::new_rwlock(()),
            collections,
            geofences,
            storage,
            config: P::new_rwlock(self.config),
            next_sequence: P::new_rwlock(0),
            _platform: PhantomData,
        };
        db.gc();
        *P::write(&db.next_sequence) = last_sequence;
        Ok(db)
    }
}

pub struct LatLng<P: Platform, S: StorageBackend> {
    control: P::RwLock<()>,
    collections: P::RwLock<HashMap<String, CollectionHandle<P>>>,
    geofences: P::RwLock<GeofenceRegistry<P>>,
    storage: S,
    config: P::RwLock<Config>,
    next_sequence: P::RwLock<u64>,
    _platform: PhantomData<P>,
}

pub type LatLngNative<S> = LatLng<NativePlatform, S>;
pub type LatLngWasm<S> = LatLng<WasmPlatform, S>;
type CollectionHandle<P> = <P as Platform>::Shared<<P as Platform>::RwLock<VersionedCollection>>;

mod admin;
mod catalog;
mod commands;
mod engine;
mod geofences;
mod internals;
mod log_codec;
mod persistence;
mod queries;
mod restore;
mod util;
mod webhook_ids;

pub(crate) use catalog::*;
pub(crate) use log_codec::*;
pub(crate) use restore::*;
pub(crate) use util::*;
pub(crate) use webhook_ids::*;

#[cfg(all(
    target_arch = "wasm32",
    any(feature = "wasm-bindings", feature = "wasm-browser-bindings")
))]
mod wasm;

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread::{self, sleep};
    use std::time::Duration;

    use bytes::Bytes;
    use latlng_geo::{Area, BoundingBox, FieldMap, FieldValue, GeoType};
    use latlng_geofence::{DetectType, GeofenceQuery};
    use latlng_index::{OutputFormat, SearchOptions, WhereComparison, WhereFilter};
    use latlng_platform::{NativePlatform, Platform};
    use latlng_storage::{
        CompactionResult, StorageBackend, StorageEntry, StorageError, StorageResult,
    };
    use latlng_storage_aof::AofBackend;
    use latlng_storage_memory::MemoryBackend;
    use tempfile::tempdir;

    use super::{
        Command, Config, FieldEntry, GeofenceDef, GetOptions, LatLng, LogRecord, MutationCommand,
        PersistedSetRecord, SetCondition, SetRequest, WebhookAckRecord, WebhookEnqueueRecord,
        decode_log_record, encode_log_record, now_millis, now_nanos,
    };

    fn db() -> LatLng<NativePlatform, MemoryBackend> {
        LatLng::builder()
            .storage(MemoryBackend::recording())
            .build()
            .unwrap()
    }

    fn db_with_config(config: Config) -> LatLng<NativePlatform, MemoryBackend> {
        LatLng::builder()
            .storage(MemoryBackend::recording())
            .config(config)
            .build()
            .unwrap()
    }

    fn db_with_storage<S: StorageBackend>(storage: S) -> LatLng<NativePlatform, S> {
        LatLng::builder().storage(storage).build().unwrap()
    }

    fn db_from_records(records: &[LogRecord]) -> LatLng<NativePlatform, MemoryBackend> {
        let backend = MemoryBackend::recording();
        for (index, record) in records.iter().enumerate() {
            backend
                .append(&StorageEntry {
                    sequence: index as u64 + 1,
                    timestamp_ns: now_nanos(),
                    command: Bytes::from(encode_log_record(record).unwrap()),
                })
                .unwrap();
        }
        LatLng::builder().storage(backend).build().unwrap()
    }

    fn stored_deadline(
        db: &LatLng<NativePlatform, MemoryBackend>,
        collection: &str,
        id: &str,
    ) -> Option<u64> {
        let collections = match db.collections.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        collections
            .get(collection)
            .and_then(|collection_state| match collection_state.read() {
                Ok(guard) => guard.collection.objects.get(id).cloned(),
                Err(poisoned) => poisoned.into_inner().collection.objects.get(id).cloned(),
            })
            .and_then(|object| object.expires_at)
    }

    fn persisted_log_records(db: &LatLng<NativePlatform, MemoryBackend>) -> Vec<LogRecord> {
        db.storage
            .entries()
            .into_iter()
            .map(|entry| decode_log_record(&entry.command).unwrap())
            .collect()
    }

    #[derive(Debug)]
    struct FailingAppendBackend {
        inner: MemoryBackend,
        fail_appends: Arc<AtomicBool>,
    }

    impl FailingAppendBackend {
        fn with_records(records: &[LogRecord], fail_appends: bool) -> Self {
            let inner = MemoryBackend::recording();
            for (index, record) in records.iter().enumerate() {
                inner
                    .append(&StorageEntry {
                        sequence: index as u64 + 1,
                        timestamp_ns: now_nanos(),
                        command: Bytes::from(encode_log_record(record).unwrap()),
                    })
                    .unwrap();
            }
            Self {
                inner,
                fail_appends: Arc::new(AtomicBool::new(fail_appends)),
            }
        }
    }

    impl StorageBackend for FailingAppendBackend {
        fn stores_command_log(&self) -> bool {
            true
        }

        fn append(&self, entry: &StorageEntry) -> StorageResult<()> {
            if self.fail_appends.load(Ordering::Relaxed) {
                return Err(StorageError::Message("injected append failure".to_owned()));
            }
            self.inner.append(entry)
        }

        fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()> {
            if self.fail_appends.load(Ordering::Relaxed) {
                return Err(StorageError::Message("injected append failure".to_owned()));
            }
            self.inner.append_batch(entries)
        }

        fn replay(
            &self,
            after_seq: u64,
            callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
        ) -> StorageResult<u64> {
            self.inner.replay(after_seq, callback)
        }

        fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()> {
            self.inner.snapshot(entries)
        }

        fn compact(&self) -> StorageResult<CompactionResult> {
            self.inner.compact()
        }

        fn last_sequence(&self) -> StorageResult<u64> {
            self.inner.last_sequence()
        }

        fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]> {
            self.inner.checksum(from, to)
        }

        fn close(&self) -> StorageResult<()> {
            self.inner.close()
        }
    }

    #[test]
    fn volatile_memory_backend_skips_command_log_storage() {
        let db = db_with_storage(MemoryBackend::new());

        assert!(
            db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: "truck-1".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap()
        );

        assert_eq!(db.last_sequence(), 1);
        assert!(db.storage.entries().is_empty());
    }

    #[test]
    fn volatile_memory_aofshrink_does_not_materialize_snapshot_log() {
        let db = db_with_storage(MemoryBackend::new());
        for index in 0..3 {
            db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: format!("truck-{index}"),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap();
        }

        let result = db.aofshrink().unwrap();

        assert_eq!(db.last_sequence(), 3);
        assert_eq!(result.after_entries, 0);
        assert!(db.storage.entries().is_empty());
    }

    #[test]
    fn volatile_existing_point_set_updates_indexes_without_log_entries() {
        let db = db_with_storage(MemoryBackend::new());
        assert!(db.create_collection("fleet").unwrap());

        assert!(
            db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: "truck-1".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: vec![FieldEntry {
                    name: "speed".to_owned(),
                    value: FieldValue::Number(80.0),
                }],
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap()
        );

        assert_eq!(db.last_sequence(), 2);
        assert!(db.storage.entries().is_empty());

        let object = db
            .get(
                "fleet",
                "truck-1",
                GetOptions {
                    with_fields: true,
                    output: OutputFormat::Objects,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(object.fields.get("speed"), Some(&FieldValue::Number(80.0)));

        let nearby = db
            .nearby(
                "fleet",
                super::NearbyQuery {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
            )
            .unwrap();
        assert_eq!(nearby.count, 1);

        let scanned = db
            .scan(
                "fleet",
                SearchOptions {
                    where_filters: vec![WhereFilter {
                        field: "speed".to_owned(),
                        comparison: WhereComparison::Range {
                            min: 70.0,
                            max: 90.0,
                        },
                    }],
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(scanned.count, 1);
        assert_eq!(scanned.results[0].id, "truck-1");
    }

    #[test]
    fn volatile_point_set_keeps_geofence_side_effects() {
        let db = db_with_storage(MemoryBackend::new());
        assert!(db.create_collection("fleet").unwrap());
        db.setchan(
            "fleet-events",
            GeofenceDef {
                collection: "fleet".to_owned(),
                query: GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![DetectType::Enter],
                commands: vec![MutationCommand::Set],
            },
        )
        .unwrap();
        let mut receiver = db.subscribe(&["fleet-events"]);

        assert!(
            db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: "truck-1".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap()
        );

        let event = receiver.try_recv().expect("expected geofence event");
        assert_eq!(event.id, "truck-1");
        assert_eq!(event.detect, DetectType::Enter);
        assert!(db.storage.entries().is_empty());
    }

    #[test]
    fn volatile_set_falls_back_for_conditions_and_expiry() {
        let db = db_with_storage(MemoryBackend::new());
        assert!(db.create_collection("fleet").unwrap());

        assert!(
            !db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: "missing".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expire_seconds: None,
                condition: SetCondition::Xx,
            })
            .unwrap()
        );
        assert!(!db.exists("fleet", "missing").unwrap());

        assert!(
            db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: "truck-ttl".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expire_seconds: Some(30),
                condition: SetCondition::Always,
            })
            .unwrap()
        );
        assert!(stored_deadline(&db, "fleet", "truck-ttl").is_some());
        assert!(db.storage.entries().is_empty());
    }

    #[test]
    fn get_omits_fields_by_default_and_preserves_them_when_requested() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: vec![FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(80.0),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let without_fields = db
            .get("fleet", "truck-1", GetOptions::default())
            .unwrap()
            .unwrap();
        assert!(without_fields.fields.iter().next().is_none());

        let with_fields = db
            .get(
                "fleet",
                "truck-1",
                GetOptions {
                    with_fields: true,
                    output: OutputFormat::Points,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            with_fields.fields.get("speed"),
            Some(&FieldValue::Number(80.0))
        );
        assert!(matches!(with_fields.geo, GeoType::Point { .. }));
    }

    #[test]
    fn set_get_and_nearby_roundtrip() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: vec![FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(80.0),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let object = db
            .get(
                "fleet",
                "truck-1",
                GetOptions {
                    with_fields: true,
                    output: OutputFormat::Objects,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(object.id, "truck-1");

        let results = db
            .nearby(
                "fleet",
                super::NearbyQuery {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 500.0,
                    options: SearchOptions::default(),
                },
            )
            .unwrap();
        assert_eq!(results.count, 1);
    }

    #[test]
    fn collections_can_be_created_explicitly_and_replayed_empty() {
        let db = db();

        assert!(db.create_collection("fleet").unwrap());
        assert!(!db.create_collection("fleet").unwrap());
        assert_eq!(db.collections("*").unwrap(), vec!["fleet".to_owned()]);

        let stats = db.stats(&["fleet"]).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].object_count, 0);

        let replayed = db_from_records(&persisted_log_records(&db));
        assert_eq!(replayed.collections("*").unwrap(), vec!["fleet".to_owned()]);
        let replayed_stats = replayed.stats(&["fleet"]).unwrap();
        assert_eq!(replayed_stats.len(), 1);
        assert_eq!(replayed_stats[0].object_count, 0);
    }

    #[test]
    fn deleting_last_object_keeps_collection_until_drop() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        assert!(db.del("fleet", "truck-1").unwrap());
        assert_eq!(db.collections("*").unwrap(), vec!["fleet".to_owned()]);
        let stats = db.stats(&["fleet"]).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].object_count, 0);

        assert!(db.drop_collection("fleet").unwrap());
        assert!(db.collections("*").unwrap().is_empty());
    }

    #[test]
    fn previewed_set_batch_reports_storage_entries() {
        let source = db();
        let (stored, entries, webhook_jobs) = source
            .preview_set_command_batch(&SetRequest {
                collection: "fleet".to_owned(),
                id: "truck-preview".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: vec![FieldEntry {
                    name: "speed".to_owned(),
                    value: FieldValue::Number(45.0),
                }],
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap();
        assert!(stored);
        assert!(webhook_jobs.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sequence, 1);
        assert!(!entries[0].command.is_empty());
    }

    #[test]
    fn webhook_ids_are_stable_and_opaque() {
        fn preview_webhook() -> (String, String) {
            let source = db();
            source
                .sethook(
                    "fleet-hook",
                    "https://hooks.example.test/ingest",
                    crate::geofence::GeofenceDef {
                        collection: "fleet".to_owned(),
                        query: crate::geofence::GeofenceQuery::Nearby {
                            lat: 52.52,
                            lon: 13.405,
                            meters: 100.0,
                            options: SearchOptions::default(),
                        },
                        detect: vec![crate::geofence::DetectType::Enter],
                        commands: vec![crate::geofence::MutationCommand::Set],
                    },
                )
                .unwrap();
            let (_, entries, webhook_jobs) = source
                .preview_set_command_batch(&SetRequest {
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: Vec::new(),
                    expire_seconds: None,
                    condition: SetCondition::Always,
                })
                .unwrap();

            assert_eq!(webhook_jobs.len(), 1);
            assert_eq!(entries.len(), 2);
            let LogRecord::WebhookEnqueue(record) = decode_log_record(&entries[1].command).unwrap()
            else {
                panic!("expected webhook enqueue log record");
            };
            (record.event.event_id.unwrap(), record.event.job_id.unwrap())
        }

        let first = preview_webhook();
        let second = preview_webhook();
        assert_eq!(first, second);
        assert!(first.0.starts_with("evt_"));
        assert!(first.1.starts_with("job_"));
        assert!(!first.0.starts_with("whevt-"));
        assert!(!first.1.starts_with("whjob-"));
        assert!(!first.0.contains("truck-1"));
    }

    #[test]
    fn previewed_hook_log_entry_can_be_replayed() {
        let source = db();
        let entry = source
            .preview_log_record_entry(LogRecord::Command(Command::SetHook {
                name: "fleet-hook".to_owned(),
                endpoint: "https://hooks.example.test/ingest".to_owned(),
                def: crate::geofence::GeofenceDef {
                    collection: "fleet".to_owned(),
                    query: crate::geofence::GeofenceQuery::Nearby {
                        lat: 52.52,
                        lon: 13.405,
                        meters: 100.0,
                        options: SearchOptions::default(),
                    },
                    detect: vec![crate::geofence::DetectType::Enter],
                    commands: vec![crate::geofence::MutationCommand::Set],
                },
            }))
            .unwrap();

        let replayed = db();
        replayed.apply_replicated_entries(&[entry]).unwrap();

        let hooks = replayed.hooks("*").unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "fleet-hook");
        assert_eq!(hooks[0].collection, "fleet");
    }

    #[test]
    fn previewed_geojson_hook_log_entry_can_be_replayed() {
        let source = db();
        let entry = source
            .preview_log_record_entry(LogRecord::Command(Command::SetHook {
                name: "fleet-hook".to_owned(),
                endpoint: "https://hooks.example.test/ingest".to_owned(),
                def: crate::geofence::GeofenceDef {
                    collection: "fleet".to_owned(),
                    query: crate::geofence::GeofenceQuery::Within {
                        area: Area::GeoJson(serde_json::json!({
                            "type": "Polygon",
                            "coordinates": [[
                                [13.39, 52.51],
                                [13.42, 52.51],
                                [13.42, 52.53],
                                [13.39, 52.53],
                                [13.39, 52.51]
                            ]]
                        })),
                        options: SearchOptions::default(),
                    },
                    detect: vec![crate::geofence::DetectType::Enter],
                    commands: vec![crate::geofence::MutationCommand::Set],
                },
            }))
            .unwrap();

        let replayed = db();
        replayed.apply_replicated_entries(&[entry]).unwrap();

        let hooks = replayed.hooks("*").unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "fleet-hook");
        assert_eq!(hooks[0].collection, "fleet");
    }

    #[test]
    fn previewed_geojson_object_log_entry_can_be_replayed() {
        let source = db();
        let (stored, entries, webhook_jobs) = source
            .preview_set_command_batch(&SetRequest {
                collection: "zones".to_owned(),
                id: "poly-1".to_owned(),
                object: GeoType::GeoJson(serde_json::json!({
                    "type": "Polygon",
                    "coordinates": [[
                        [0.0, 0.0],
                        [2.0, 0.0],
                        [2.0, 2.0],
                        [0.0, 2.0],
                        [0.0, 0.0]
                    ]]
                })),
                fields: Vec::new(),
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap();
        assert!(stored);
        assert!(webhook_jobs.is_empty());

        let replayed = db();
        replayed.apply_replicated_entries(&entries).unwrap();

        assert!(replayed.exists("zones", "poly-1").unwrap());
    }

    #[test]
    fn legacy_bincode_log_record_can_still_be_decoded() {
        let record = LogRecord::Command(Command::CreateCollection {
            collection: "fleet".to_owned(),
        });
        let payload = bincode::serialize(&record).unwrap();

        assert_eq!(decode_log_record(&payload).unwrap(), record);
    }

    #[test]
    fn legacy_bincode_command_can_still_be_decoded() {
        let command = Command::CreateCollection {
            collection: "fleet".to_owned(),
        };
        let payload = bincode::serialize(&command).unwrap();

        assert_eq!(
            decode_log_record(&payload).unwrap(),
            LogRecord::Command(command)
        );
    }

    #[test]
    fn where_filter_applies_in_scan() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "car-1".to_owned(),
            object: GeoType::point(1.0, 1.0),
            fields: vec![FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(35.0),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let results = db
            .scan(
                "fleet",
                SearchOptions {
                    where_filters: vec![WhereFilter {
                        field: "speed".to_owned(),
                        comparison: WhereComparison::Range {
                            min: 30.0,
                            max: 40.0,
                        },
                    }],
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(results.count, 1);
        assert_eq!(results.results[0].id, "car-1");
    }

    #[test]
    fn scan_can_skip_count_for_limited_ids() {
        let db = db();
        for index in 0..5 {
            db.set(SetRequest {
                collection: "fleet".to_owned(),
                id: format!("car-{index}"),
                object: GeoType::point(1.0, 1.0),
                fields: vec![FieldEntry {
                    name: "speed".to_owned(),
                    value: FieldValue::Number(35.0),
                }],
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .unwrap();
        }

        let results = db
            .scan(
                "fleet",
                SearchOptions {
                    limit: 2,
                    nofields: true,
                    include_count: false,
                    where_filters: vec![WhereFilter {
                        field: "speed".to_owned(),
                        comparison: WhereComparison::Range {
                            min: 30.0,
                            max: 40.0,
                        },
                    }],
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(results.count, 2);
        assert_eq!(results.results.len(), 2);
        assert!(results.results.iter().all(|item| item.fields.is_none()));
    }

    #[test]
    fn fset_updates_field_index() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "car-1".to_owned(),
            object: GeoType::point(1.0, 1.0),
            fields: vec![FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(10.0),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.fset(
            "fleet",
            "car-1",
            &[FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(55.0),
            }],
            false,
        )
        .unwrap();

        let old = db
            .scan(
                "fleet",
                SearchOptions {
                    where_filters: vec![WhereFilter {
                        field: "speed".to_owned(),
                        comparison: WhereComparison::Range {
                            min: 5.0,
                            max: 15.0,
                        },
                    }],
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        let new = db
            .scan(
                "fleet",
                SearchOptions {
                    where_filters: vec![WhereFilter {
                        field: "speed".to_owned(),
                        comparison: WhereComparison::Range {
                            min: 50.0,
                            max: 60.0,
                        },
                    }],
                    output: OutputFormat::Ids,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(old.count, 0);
        assert_eq!(new.count, 1);
        assert_eq!(new.results[0].id, "car-1");
    }

    #[test]
    fn subscriber_queue_capacity_config_is_applied() {
        let db = db_with_config(Config {
            subscriber_queue_capacity: 1,
            ..Config::default()
        });
        db.setchan(
            "fleet",
            crate::geofence::GeofenceDef {
                collection: "fleet".to_owned(),
                query: crate::geofence::GeofenceQuery::Nearby {
                    lat: 52.52,
                    lon: 13.405,
                    meters: 100.0,
                    options: SearchOptions::default(),
                },
                detect: vec![
                    crate::geofence::DetectType::Enter,
                    crate::geofence::DetectType::Inside,
                ],
                commands: vec![crate::geofence::MutationCommand::Set],
            },
        )
        .unwrap();
        let mut receiver = db.subscribe(&["fleet"]);

        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.5202, 13.4052),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        assert_eq!(
            receiver.try_recv().unwrap().detect,
            crate::geofence::DetectType::Inside
        );
        assert!(receiver.try_recv().is_none());
    }

    #[test]
    fn control_gate_allows_parallel_reads() {
        let db = Arc::new(db());
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let guard = db.read_control();
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(guard);
            })
        };

        started_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let reader = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let object = db
                    .get("fleet", "truck-1", GetOptions::default())
                    .unwrap()
                    .is_some();
                done_tx.send(object).unwrap();
            })
        };

        assert_eq!(done_rx.recv_timeout(Duration::from_millis(200)), Ok(true));
        release_tx.send(()).unwrap();

        holder.join().unwrap();
        reader.join().unwrap();
    }

    #[test]
    fn control_gate_blocks_reads_until_writer_releases() {
        let db = Arc::new(db());
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let guard = db.write_control();
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(guard);
            })
        };

        started_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let reader = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let object = db
                    .get("fleet", "truck-1", GetOptions::default())
                    .unwrap()
                    .is_some();
                done_tx.send(object).unwrap();
            })
        };

        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        release_tx.send(()).unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(1)), Ok(true));

        holder.join().unwrap();
        reader.join().unwrap();
    }

    #[test]
    fn control_gate_serializes_writes() {
        let db = Arc::new(db());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let guard = db.write_control();
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(guard);
            })
        };

        started_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let writer = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                db.set_timeout("set", 1.25);
                done_tx.send(()).unwrap();
            })
        };

        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(db.timeout("set"), Some(1.25));

        holder.join().unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn collection_read_on_other_collection_does_not_wait_on_unrelated_write_lock() {
        let db = Arc::new(db());
        db.set(SetRequest {
            collection: "fleet-a".to_owned(),
            id: "truck-a".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.set(SetRequest {
            collection: "fleet-b".to_owned(),
            id: "truck-b".to_owned(),
            object: GeoType::point(48.13, 11.58),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let handle = db.collection_handle("fleet-a").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            let guard = NativePlatform::write(&*handle);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });

        started_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let reader = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let object = db
                    .get("fleet-b", "truck-b", GetOptions::default())
                    .unwrap()
                    .is_some();
                done_tx.send(object).unwrap();
            })
        };

        assert_eq!(done_rx.recv_timeout(Duration::from_millis(200)), Ok(true));
        release_tx.send(()).unwrap();

        holder.join().unwrap();
        reader.join().unwrap();
    }

    #[test]
    fn collection_write_on_other_collection_does_not_wait_on_unrelated_write_lock() {
        let db = Arc::new(db());
        db.set(SetRequest {
            collection: "fleet-a".to_owned(),
            id: "truck-a".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.set(SetRequest {
            collection: "fleet-b".to_owned(),
            id: "truck-b".to_owned(),
            object: GeoType::point(48.13, 11.58),
            fields: vec![FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(20.0),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let handle = db.collection_handle("fleet-a").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            let guard = NativePlatform::write(&*handle);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });

        started_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let writer = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let updated = db
                    .fset(
                        "fleet-b",
                        "truck-b",
                        &[FieldEntry {
                            name: "speed".to_owned(),
                            value: FieldValue::Number(42.0),
                        }],
                        false,
                    )
                    .unwrap();
                done_tx.send(updated).unwrap();
            })
        };

        assert_eq!(done_rx.recv_timeout(Duration::from_millis(200)), Ok(true));
        release_tx.send(()).unwrap();

        assert_eq!(
            db.fget("fleet-b", "truck-b", "speed").unwrap(),
            Some(FieldValue::Number(42.0))
        );

        holder.join().unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn same_collection_write_waits_for_collection_lock() {
        let db = Arc::new(db());
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: vec![FieldEntry {
                name: "speed".to_owned(),
                value: FieldValue::Number(20.0),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let handle = db.collection_handle("fleet").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            let guard = NativePlatform::write(&*handle);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });

        started_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let writer = {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                let updated = db
                    .fset(
                        "fleet",
                        "truck-1",
                        &[FieldEntry {
                            name: "speed".to_owned(),
                            value: FieldValue::Number(42.0),
                        }],
                        false,
                    )
                    .unwrap();
                done_tx.send(updated).unwrap();
            })
        };

        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        release_tx.send(()).unwrap();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(1)), Ok(true));

        holder.join().unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn flushdb_clears_geofences_and_drops_stale_subscriber_events() {
        let db = db();
        let def = crate::geofence::GeofenceDef {
            collection: "fleet".to_owned(),
            query: crate::geofence::GeofenceQuery::Nearby {
                lat: 52.52,
                lon: 13.405,
                meters: 100.0,
                options: SearchOptions::default(),
            },
            detect: vec![crate::geofence::DetectType::Enter],
            commands: vec![crate::geofence::MutationCommand::Set],
        };
        db.setchan("fleet", def.clone()).unwrap();
        db.sethook("fleet-hook", "http://127.0.0.1:1/hook", def.clone())
            .unwrap();
        let mut receiver = db.subscribe(&["fleet"]);

        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: vec![FieldEntry {
                name: "status".to_owned(),
                value: FieldValue::Text("before-flush".to_owned()),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        db.flushdb().unwrap();

        assert!(db.chans("*").unwrap().is_empty());
        assert!(db.hooks("*").unwrap().is_empty());

        db.setchan("fleet", def).unwrap();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: vec![FieldEntry {
                name: "status".to_owned(),
                value: FieldValue::Text("after-flush".to_owned()),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let event = receiver.try_recv().unwrap();
        assert_eq!(event.detect, crate::geofence::DetectType::Enter);
        assert_eq!(
            event.fields.get("status"),
            Some(&FieldValue::Text("after-flush".to_owned()))
        );
        assert!(receiver.try_recv().is_none());
    }

    #[test]
    fn set_persists_absolute_deadline_in_log() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-ttl".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: Some(2),
            condition: SetCondition::Always,
        })
        .unwrap();

        let records = persisted_log_records(&db);
        let deadline = match &records[0] {
            LogRecord::Command(Command::SetPersisted(record)) => record.expires_at_ms,
            other => panic!("unexpected persisted record: {other:?}"),
        };
        let expected_min = now_millis().saturating_add(1_500);
        let expected_max = now_millis().saturating_add(2_100);
        let deadline = deadline.expect("deadline should be present");
        assert!(deadline >= expected_min);
        assert!(deadline <= expected_max);
    }

    #[test]
    fn expire_persists_absolute_deadline_in_log() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-expire".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.expire("fleet", "truck-expire", 2).unwrap();

        let records = persisted_log_records(&db);
        let deadline = match &records[1] {
            LogRecord::Command(Command::ExpireAt { expires_at_ms, .. }) => *expires_at_ms,
            other => panic!("unexpected persisted record: {other:?}"),
        };
        let expected_min = now_millis().saturating_add(1_500);
        let expected_max = now_millis().saturating_add(2_100);
        assert!(deadline >= expected_min);
        assert!(deadline <= expected_max);
    }

    #[test]
    fn replay_preserves_absolute_deadline_without_extending_it() {
        let deadline = now_millis().saturating_add(400);
        sleep(Duration::from_millis(120));
        let db = db_from_records(&[LogRecord::Command(Command::SetPersisted(
            PersistedSetRecord {
                collection: "fleet".to_owned(),
                id: "truck-replay".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expires_at_ms: Some(deadline),
            },
        ))]);

        assert_eq!(
            stored_deadline(&db, "fleet", "truck-replay"),
            Some(deadline)
        );
    }

    #[test]
    fn startup_replay_resumes_next_sequence_from_last_log_entry() {
        let db = db_from_records(&[
            LogRecord::Command(Command::SetPersisted(PersistedSetRecord {
                collection: "fleet".to_owned(),
                id: "truck-1".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expires_at_ms: None,
            })),
            LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                job_id: "whjob-2".to_owned(),
                event: crate::geofence::GeofenceEvent {
                    command: crate::geofence::MutationCommand::Set,
                    detect: crate::geofence::DetectType::Enter,
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: FieldMap::new(),
                    timestamp_ns: now_nanos(),
                    event_id: Some("evt-2".to_owned()),
                    job_id: Some("whjob-2".to_owned()),
                    hook: Some("fleet-hook".to_owned()),
                    group: Some("fleet-hook".to_owned()),
                    nearby: None,
                    generation: 0,
                },
                endpoint: "http://127.0.0.1:1/hook".to_owned(),
                attempts_used: 0,
                max_attempts: 9,
                next_attempt_at_ms: now_millis(),
            }),
        ]);

        let next = db
            .append_log_record(LogRecord::Command(Command::Persist {
                collection: "fleet".to_owned(),
                id: "truck-1".to_owned(),
            }))
            .unwrap();
        assert_eq!(next, 3);
    }

    #[test]
    fn expired_objects_are_removed_after_replay() {
        let deadline = now_millis().saturating_add(60);
        sleep(Duration::from_millis(80));
        let db = db_from_records(&[LogRecord::Command(Command::SetPersisted(
            PersistedSetRecord {
                collection: "fleet".to_owned(),
                id: "truck-expired".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expires_at_ms: Some(deadline),
            },
        ))]);

        assert!(
            db.get("fleet", "truck-expired", GetOptions::default())
                .unwrap()
                .is_none()
        );
        assert!(!db.exists("fleet", "truck-expired").unwrap());
    }

    #[test]
    fn persist_after_expiring_set_survives_replay() {
        let deadline = now_millis().saturating_add(60);
        sleep(Duration::from_millis(80));
        let db = db_from_records(&[
            LogRecord::Command(Command::SetPersisted(PersistedSetRecord {
                collection: "fleet".to_owned(),
                id: "truck-persist".to_owned(),
                object: GeoType::point(52.52, 13.405),
                fields: Vec::new(),
                expires_at_ms: Some(deadline),
            })),
            LogRecord::Command(Command::Persist {
                collection: "fleet".to_owned(),
                id: "truck-persist".to_owned(),
            }),
        ]);

        assert!(
            db.get("fleet", "truck-persist", GetOptions::default())
                .unwrap()
                .is_some()
        );
        assert_eq!(stored_deadline(&db, "fleet", "truck-persist"), None);
    }

    #[test]
    fn webhook_log_records_are_ignored_during_core_rebuild() {
        let db = db_from_records(&[
            LogRecord::WebhookEnqueue(WebhookEnqueueRecord {
                job_id: "whjob-1".to_owned(),
                event: crate::geofence::GeofenceEvent {
                    command: crate::geofence::MutationCommand::Set,
                    detect: crate::geofence::DetectType::Enter,
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: FieldMap::new(),
                    timestamp_ns: now_nanos(),
                    event_id: Some("evt-1".to_owned()),
                    job_id: Some("whjob-1".to_owned()),
                    hook: Some("fleet-hook".to_owned()),
                    group: Some("fleet-hook".to_owned()),
                    nearby: None,
                    generation: 0,
                },
                endpoint: "http://127.0.0.1:1/hook".to_owned(),
                attempts_used: 0,
                max_attempts: 9,
                next_attempt_at_ms: now_millis(),
            }),
            LogRecord::WebhookAck(WebhookAckRecord {
                job_id: "whjob-1".to_owned(),
            }),
        ]);

        assert!(db.collections("*").unwrap().is_empty());
        assert!(db.hooks("*").unwrap().is_empty());
        assert!(db.chans("*").unwrap().is_empty());
    }

    #[test]
    fn aofshrink_preserves_original_deadline() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-snapshot".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: Some(2),
            condition: SetCondition::Always,
        })
        .unwrap();

        let original_deadline = stored_deadline(&db, "fleet", "truck-snapshot").unwrap();
        sleep(Duration::from_millis(120));
        db.aofshrink().unwrap();
        let records = persisted_log_records(&db);
        let snapshot_deadline = match records
            .iter()
            .find(|record| matches!(record, LogRecord::Command(Command::SetPersisted(_))))
            .expect("expected compacted snapshot to include persisted object record")
        {
            LogRecord::Command(Command::SetPersisted(record)) => record.expires_at_ms,
            other => panic!("unexpected snapshot record: {other:?}"),
        };

        assert_eq!(snapshot_deadline, Some(original_deadline));
    }

    #[test]
    fn aofshrink_preserves_empty_explicit_collections() {
        let db = db();
        assert!(db.create_collection("fleet").unwrap());

        db.aofshrink().unwrap();

        let replayed = db_from_records(&persisted_log_records(&db));
        assert_eq!(replayed.collections("*").unwrap(), vec!["fleet".to_owned()]);
        let stats = replayed.stats(&["fleet"]).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].object_count, 0);
    }

    #[test]
    fn rename_append_failure_leaves_source_collection_present() {
        let backend = FailingAppendBackend::with_records(
            &[LogRecord::Command(Command::SetPersisted(
                PersistedSetRecord {
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: Vec::new(),
                    expires_at_ms: None,
                },
            ))],
            true,
        );
        let db = db_with_storage(backend);

        let error = db.rename("fleet", "renamed").unwrap_err();
        assert!(matches!(error, super::CoreError::Storage(_)));
        assert!(db.exists("fleet", "truck-1").unwrap());
        assert!(!db.exists("renamed", "truck-1").unwrap());
    }

    #[test]
    fn renamenx_append_failure_leaves_source_collection_present() {
        let backend = FailingAppendBackend::with_records(
            &[LogRecord::Command(Command::SetPersisted(
                PersistedSetRecord {
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: Vec::new(),
                    expires_at_ms: None,
                },
            ))],
            true,
        );
        let db = db_with_storage(backend);

        let error = db.renamenx("fleet", "renamed").unwrap_err();
        assert!(matches!(error, super::CoreError::Storage(_)));
        assert!(db.exists("fleet", "truck-1").unwrap());
        assert!(!db.exists("renamed", "truck-1").unwrap());
    }

    #[test]
    fn renamenx_existing_target_returns_false_without_appending() {
        let backend = FailingAppendBackend::with_records(
            &[
                LogRecord::Command(Command::SetPersisted(PersistedSetRecord {
                    collection: "fleet".to_owned(),
                    id: "truck-1".to_owned(),
                    object: GeoType::point(52.52, 13.405),
                    fields: Vec::new(),
                    expires_at_ms: None,
                })),
                LogRecord::Command(Command::SetPersisted(PersistedSetRecord {
                    collection: "renamed".to_owned(),
                    id: "truck-2".to_owned(),
                    object: GeoType::point(52.53, 13.406),
                    fields: Vec::new(),
                    expires_at_ms: None,
                })),
            ],
            true,
        );
        let db = db_with_storage(backend);

        assert!(!db.renamenx("fleet", "renamed").unwrap());
        assert!(db.exists("fleet", "truck-1").unwrap());
        assert!(db.exists("renamed", "truck-2").unwrap());
    }

    #[test]
    fn successful_rename_replays_from_persisted_log() {
        let db = db();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.rename("fleet", "renamed").unwrap();

        let replayed = db_from_records(&persisted_log_records(&db));
        assert!(!replayed.exists("fleet", "truck-1").unwrap());
        assert!(replayed.exists("renamed", "truck-1").unwrap());
    }

    #[test]
    fn intersects_with_clip_returns_clipped_geometry() {
        let db = db();
        db.set(SetRequest {
            collection: "zones".to_owned(),
            id: "poly-1".to_owned(),
            object: GeoType::GeoJson(serde_json::json!({
                "type": "Polygon",
                "coordinates": [[
                    [0.0, 0.0],
                    [2.0, 0.0],
                    [2.0, 2.0],
                    [0.0, 2.0],
                    [0.0, 0.0]
                ]]
            })),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();

        let results = db
            .intersects(
                "zones",
                Area::Bounds(BoundingBox::new(1.0, 1.0, 3.0, 3.0)),
                SearchOptions {
                    clip: true,
                    output: OutputFormat::Objects,
                    ..SearchOptions::default()
                },
            )
            .unwrap();

        assert_eq!(results.count, 1);
        let clipped = results.results[0].object.clone().unwrap();
        assert_eq!(
            clipped.envelope().unwrap().unwrap(),
            BoundingBox::new(1.0, 1.0, 2.0, 2.0)
        );
    }

    #[test]
    fn torn_aof_tail_drops_command_and_webhook_batch_together() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let db = db_with_storage(AofBackend::open(&path).unwrap());
        let def = crate::geofence::GeofenceDef {
            collection: "fleet".to_owned(),
            query: crate::geofence::GeofenceQuery::Nearby {
                lat: 52.52,
                lon: 13.405,
                meters: 100.0,
                options: SearchOptions::default(),
            },
            detect: vec![crate::geofence::DetectType::Enter],
            commands: vec![crate::geofence::MutationCommand::Set],
        };
        db.sethook("fleet-hook", "http://127.0.0.1:1/hook", def)
            .unwrap();
        db.set(SetRequest {
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })
        .unwrap();
        db.storage.close().unwrap();

        let file = OpenOptions::new().write(true).open(&path).unwrap();
        let full_len = file.metadata().unwrap().len();
        assert!(full_len > 16);
        file.set_len(full_len - 16).unwrap();

        let replayed = db_with_storage(AofBackend::open(&path).unwrap());
        assert!(!replayed.exists("fleet", "truck-1").unwrap());
        let hooks = replayed.hooks("*").unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "fleet-hook");
    }
}
