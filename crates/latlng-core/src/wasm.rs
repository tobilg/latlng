use super::{
    FieldEntry, GeoType, GeofenceDef, GetOptions, LatLngWasm, NearbyQuery, OutputFormat,
    SearchOptions, WebhookEnqueueRecord,
};
#[cfg(feature = "wasm-bindings")]
use super::{
    HookInfo, LogRecord, StorageEntry, WebhookAckRecord, WebhookDeadLetterRecord,
    WebhookRetryScheduledRecord,
};
use super::{LatLng, SetCondition, SetRequest};
use crate::geofence::{DetectType, MutationCommand, RoamingInfo};
use crate::{BoundingBox, CollectionStats};
#[cfg(feature = "wasm-bindings")]
use base64::Engine as _;
#[cfg(feature = "wasm-bindings")]
use base64::engine::general_purpose::STANDARD;
#[cfg(feature = "wasm-bindings")]
use bytes::Bytes;
use latlng_geo::FieldMap;
use latlng_storage_memory::MemoryBackend;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[derive(Debug, Deserialize)]
struct WasmSetRequest {
    object: GeoType,
    #[serde(default)]
    fields: Vec<FieldEntry>,
    expire_seconds: Option<u32>,
    condition: Option<SetCondition>,
}

#[cfg(feature = "wasm-bindings")]
#[derive(Debug, Deserialize)]
struct WasmSetHookRequest {
    name: String,
    endpoint: String,
    def: GeofenceDef,
}

#[cfg(feature = "wasm-browser-bindings")]
#[derive(Debug, Deserialize)]
struct WasmHookDefRequest {
    def: GeofenceDef,
}

#[cfg(feature = "wasm-browser-bindings")]
#[derive(Debug, Deserialize)]
struct WasmAreaQuery {
    area: crate::geo::Area,
    #[serde(default)]
    options: SearchOptions,
}

#[cfg(feature = "wasm-browser-bindings")]
#[derive(Debug, Serialize)]
struct WasmMutationResponse<T> {
    ok: bool,
    result: T,
    events: Vec<WasmGeofenceEvent>,
}

#[cfg(feature = "wasm-bindings")]
#[derive(Debug, Serialize)]
struct WasmSetResponse {
    ok: bool,
    stored: bool,
}

#[cfg(feature = "wasm-bindings")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WasmStorageEntry {
    sequence: String,
    timestamp_ns: String,
    command_base64: String,
}

#[cfg(feature = "wasm-bindings")]
#[derive(Debug, Serialize)]
struct WasmPreparedSetEntries {
    ok: bool,
    stored: bool,
    entries: Vec<WasmStorageEntry>,
    webhook_jobs: Vec<WasmWebhookEnqueueRecord>,
}

#[cfg(feature = "wasm-bindings")]
#[derive(Debug, Serialize)]
struct WasmPreparedDeleteEntries {
    ok: bool,
    deleted: bool,
    entries: Vec<WasmStorageEntry>,
    webhook_jobs: Vec<WasmWebhookEnqueueRecord>,
}

#[cfg(feature = "wasm-bindings")]
#[derive(Debug, Serialize)]
struct WasmPreparedCreateCollectionEntry {
    ok: bool,
    created: bool,
    entry: Option<WasmStorageEntry>,
}

#[derive(Debug, Serialize)]
struct WasmCollectionInfo {
    name: String,
    bounds: Option<BoundingBox>,
    stats: CollectionStats,
}

#[derive(Debug, Clone, Serialize)]
struct WasmGeofenceEvent {
    command: MutationCommand,
    detect: DetectType,
    collection: String,
    id: String,
    object: GeoType,
    fields: FieldMap,
    timestamp_ns: String,
    event_id: Option<String>,
    job_id: Option<String>,
    hook: Option<String>,
    group: Option<String>,
    nearby: Option<RoamingInfo>,
}

#[derive(Debug, Clone, Serialize)]
struct WasmWebhookEnqueueRecord {
    job_id: String,
    event: WasmGeofenceEvent,
    endpoint: String,
    attempts_used: u32,
    max_attempts: u32,
    next_attempt_at_ms: u64,
}

#[cfg(feature = "wasm-bindings")]
#[wasm_bindgen]
pub struct WasmLatLng {
    inner: LatLngWasm<MemoryBackend>,
}

#[cfg(feature = "wasm-bindings")]
#[wasm_bindgen]
impl WasmLatLng {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<WasmLatLng, JsValue> {
        let inner = LatLng::<super::WasmPlatform, MemoryBackend>::builder()
            .storage(MemoryBackend::new())
            .build()
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(Self { inner })
    }

    pub fn set_point(&self, collection: &str, id: &str, lat: f64, lon: f64) -> Result<(), JsValue> {
        self.inner
            .set(SetRequest {
                collection: collection.to_owned(),
                id: id.to_owned(),
                object: GeoType::point(lat, lon),
                fields: Vec::new(),
                expire_seconds: None,
                condition: SetCondition::Always,
            })
            .map(|_| ())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn set_object(
        &self,
        collection: &str,
        id: &str,
        request: JsValue,
    ) -> Result<JsValue, JsValue> {
        let request = serde_wasm_bindgen::from_value::<WasmSetRequest>(request)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let stored = self
            .inner
            .set(SetRequest {
                collection: collection.to_owned(),
                id: id.to_owned(),
                object: request.object,
                fields: request.fields,
                expire_seconds: request.expire_seconds,
                condition: request.condition.unwrap_or(SetCondition::Always),
            })
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&WasmSetResponse { ok: true, stored })
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn prepare_set_object_entries(
        &self,
        collection: &str,
        id: &str,
        request: JsValue,
    ) -> Result<JsValue, JsValue> {
        let request = serde_wasm_bindgen::from_value::<WasmSetRequest>(request)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let (stored, entries, webhook_jobs) = self
            .inner
            .preview_set_command_batch(&SetRequest {
                collection: collection.to_owned(),
                id: id.to_owned(),
                object: request.object,
                fields: request.fields,
                expire_seconds: request.expire_seconds,
                condition: request.condition.unwrap_or(SetCondition::Always),
            })
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&WasmPreparedSetEntries {
            ok: true,
            stored,
            entries: entries.into_iter().map(encode_wasm_storage_entry).collect(),
            webhook_jobs: webhook_jobs
                .into_iter()
                .map(encode_wasm_webhook_enqueue)
                .collect(),
        })
        .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn set_hook(&self, request: JsValue) -> Result<(), JsValue> {
        let request = serde_wasm_bindgen::from_value::<WasmSetHookRequest>(request)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.inner
            .sethook(&request.name, &request.endpoint, request.def)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn prepare_set_hook_entry(&self, request: JsValue) -> Result<JsValue, JsValue> {
        let request = serde_wasm_bindgen::from_value::<WasmSetHookRequest>(request)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let entry = self
            .inner
            .preview_log_record_entry(LogRecord::Command(super::Command::SetHook {
                name: request.name,
                endpoint: request.endpoint,
                def: request.def,
            }))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&encode_wasm_storage_entry(entry))
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn delete_hook(&self, name: &str) -> Result<bool, JsValue> {
        self.inner
            .delhook(name)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn prepare_delete_hook_entry(&self, name: &str) -> Result<JsValue, JsValue> {
        let entry = self
            .inner
            .preview_log_record_entry(LogRecord::Command(super::Command::DelHook {
                name: name.to_owned(),
            }))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&encode_wasm_storage_entry(entry))
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn hooks(&self, pattern: Option<String>) -> Result<JsValue, JsValue> {
        let pattern = pattern.unwrap_or_else(|| "*".to_owned());
        let hooks: Vec<HookInfo> = self
            .inner
            .hooks(&pattern)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&hooks).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn collections(&self, pattern: Option<String>) -> Result<JsValue, JsValue> {
        let pattern = pattern.unwrap_or_else(|| "*".to_owned());
        let collections = self
            .inner
            .collections(&pattern)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&collections)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn create_collection(&self, collection: &str) -> Result<bool, JsValue> {
        self.inner
            .create_collection(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn prepare_create_collection_entry(&self, collection: &str) -> Result<JsValue, JsValue> {
        let created = self.inner.collection_handle(collection).is_none();
        let entry = if created {
            Some(
                self.inner
                    .preview_log_record_entry(LogRecord::Command(
                        super::Command::CreateCollection {
                            collection: collection.to_owned(),
                        },
                    ))
                    .map(encode_wasm_storage_entry)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?,
            )
        } else {
            None
        };
        serde_wasm_bindgen::to_value(&WasmPreparedCreateCollectionEntry {
            ok: true,
            created,
            entry,
        })
        .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn collection_info(&self, collection: &str) -> Result<JsValue, JsValue> {
        let stats = self
            .inner
            .stats(&[collection])
            .map_err(|error| JsValue::from_str(&error.to_string()))?
            .into_iter()
            .next();
        let bounds = self
            .inner
            .bounds(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let info = stats.map(|stats| WasmCollectionInfo {
            name: collection.to_owned(),
            bounds,
            stats,
        });
        serde_wasm_bindgen::to_value(&info).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn get(&self, collection: &str, id: &str) -> Result<JsValue, JsValue> {
        let object = self
            .inner
            .get(
                collection,
                id,
                GetOptions {
                    with_fields: true,
                    output: OutputFormat::Objects,
                },
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&object).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn get_object(
        &self,
        collection: &str,
        id: &str,
        with_fields: bool,
    ) -> Result<JsValue, JsValue> {
        let object = self
            .inner
            .get(
                collection,
                id,
                GetOptions {
                    with_fields,
                    output: OutputFormat::Objects,
                },
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&object).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn delete_object(&self, collection: &str, id: &str) -> Result<bool, JsValue> {
        self.inner
            .del(collection, id)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn prepare_delete_object_entries(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<JsValue, JsValue> {
        let (deleted, entries, webhook_jobs) = self
            .inner
            .preview_del_command_batch(collection, id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&WasmPreparedDeleteEntries {
            ok: true,
            deleted,
            entries: entries.into_iter().map(encode_wasm_storage_entry).collect(),
            webhook_jobs: webhook_jobs
                .into_iter()
                .map(encode_wasm_webhook_enqueue)
                .collect(),
        })
        .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn drop_collection(&self, collection: &str) -> Result<bool, JsValue> {
        self.inner
            .drop_collection(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn prepare_drop_collection_entry(&self, collection: &str) -> Result<JsValue, JsValue> {
        let entry = self
            .inner
            .preview_log_record_entry(LogRecord::Command(super::Command::DropCollection {
                collection: collection.to_owned(),
            }))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&encode_wasm_storage_entry(entry))
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn nearby(
        &self,
        collection: &str,
        lat: f64,
        lon: f64,
        meters: f64,
    ) -> Result<JsValue, JsValue> {
        let results = self
            .inner
            .nearby(
                collection,
                NearbyQuery {
                    lat,
                    lon,
                    meters,
                    options: SearchOptions::default(),
                },
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&results)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn nearby_query(&self, collection: &str, query: JsValue) -> Result<JsValue, JsValue> {
        let query = serde_wasm_bindgen::from_value::<NearbyQuery>(query)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let results = self
            .inner
            .nearby(collection, query)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        serde_wasm_bindgen::to_value(&results)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn server_info(&self) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.server_info())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn last_sequence(&self) -> String {
        self.inner.last_sequence().to_string()
    }

    pub fn apply_storage_entries(&self, entries: JsValue) -> Result<String, JsValue> {
        let entries = serde_wasm_bindgen::from_value::<Vec<WasmStorageEntry>>(entries)
            .map_err(|error| JsValue::from_str(&error.to_string()))?
            .into_iter()
            .map(decode_wasm_storage_entry)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.inner
            .apply_replicated_entries(&entries)
            .map(|value| value.to_string())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn decode_storage_entry(&self, entry: JsValue) -> Result<JsValue, JsValue> {
        let entry = serde_wasm_bindgen::from_value::<WasmStorageEntry>(entry)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let entry = decode_wasm_storage_entry(entry).map_err(|error| JsValue::from_str(&error))?;
        let record = super::decode_log_record(&entry.command)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        encode_log_record_js_value(&record)
    }

    pub fn reset_state(&self) -> Result<(), JsValue> {
        self.inner
            .reset_replication_state()
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn encode_webhook_ack_entry(
        &self,
        sequence: &str,
        timestamp_ns: &str,
        job_id: &str,
    ) -> Result<JsValue, JsValue> {
        encode_log_entry_js(
            sequence,
            timestamp_ns,
            LogRecord::WebhookAck(WebhookAckRecord {
                job_id: job_id.to_owned(),
            }),
        )
    }

    pub fn encode_webhook_retry_scheduled_entry(
        &self,
        sequence: &str,
        timestamp_ns: &str,
        job_id: &str,
        attempts_used: u32,
        next_attempt_at_ms: u64,
        last_error: &str,
    ) -> Result<JsValue, JsValue> {
        encode_log_entry_js(
            sequence,
            timestamp_ns,
            LogRecord::WebhookRetryScheduled(WebhookRetryScheduledRecord {
                job_id: job_id.to_owned(),
                attempts_used,
                next_attempt_at_ms,
                last_error: last_error.to_owned(),
            }),
        )
    }

    pub fn encode_webhook_dead_letter_entry(
        &self,
        sequence: &str,
        timestamp_ns: &str,
        job_id: &str,
        attempts_used: u32,
        last_error: &str,
    ) -> Result<JsValue, JsValue> {
        encode_log_entry_js(
            sequence,
            timestamp_ns,
            LogRecord::WebhookDeadLetter(WebhookDeadLetterRecord {
                job_id: job_id.to_owned(),
                attempts_used,
                last_error: last_error.to_owned(),
            }),
        )
    }
}

#[cfg(feature = "wasm-browser-bindings")]
#[wasm_bindgen]
pub struct BrowserLatLng {
    inner: LatLngWasm<MemoryBackend>,
}

#[cfg(feature = "wasm-browser-bindings")]
#[wasm_bindgen]
impl BrowserLatLng {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<BrowserLatLng, JsValue> {
        let inner = LatLng::<super::WasmPlatform, MemoryBackend>::builder()
            .storage(MemoryBackend::new())
            .build()
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(Self { inner })
    }

    pub fn create_collection(&self, collection: &str) -> Result<JsValue, JsValue> {
        let created = self
            .inner
            .create_collection(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&WasmMutationResponse {
            ok: true,
            result: created,
            events: Vec::new(),
        })
    }

    pub fn drop_collection(&self, collection: &str) -> Result<JsValue, JsValue> {
        let (_, _, webhook_jobs) = self
            .inner
            .preview_drop_collection_command_batch(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let dropped = self
            .inner
            .drop_collection(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&WasmMutationResponse {
            ok: true,
            result: dropped,
            events: webhook_jobs
                .into_iter()
                .map(encode_wasm_webhook_enqueue)
                .map(|record| record.event)
                .collect(),
        })
    }

    pub fn collections(&self, pattern: Option<String>) -> Result<JsValue, JsValue> {
        let pattern = pattern.unwrap_or_else(|| "*".to_owned());
        let collections = self
            .inner
            .collections(&pattern)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&collections)
    }

    pub fn collection_info(&self, collection: &str) -> Result<JsValue, JsValue> {
        let stats = self
            .inner
            .stats(&[collection])
            .map_err(|error| JsValue::from_str(&error.to_string()))?
            .into_iter()
            .next();
        let bounds = self
            .inner
            .bounds(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let info = stats.map(|stats| WasmCollectionInfo {
            name: collection.to_owned(),
            bounds,
            stats,
        });
        to_js(&info)
    }

    pub fn set_object(
        &self,
        collection: &str,
        id: &str,
        request: JsValue,
    ) -> Result<JsValue, JsValue> {
        let request = serde_wasm_bindgen::from_value::<WasmSetRequest>(request)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let req = SetRequest {
            collection: collection.to_owned(),
            id: id.to_owned(),
            object: request.object,
            fields: request.fields,
            expire_seconds: request.expire_seconds,
            condition: request.condition.unwrap_or(SetCondition::Always),
        };
        let (_, _, webhook_jobs) = self
            .inner
            .preview_set_command_batch(&req)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let stored = self
            .inner
            .set(req)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&WasmMutationResponse {
            ok: true,
            result: stored,
            events: webhook_jobs
                .into_iter()
                .map(encode_wasm_webhook_enqueue)
                .map(|record| record.event)
                .collect(),
        })
    }

    pub fn get_object(
        &self,
        collection: &str,
        id: &str,
        with_fields: bool,
    ) -> Result<JsValue, JsValue> {
        let object = self
            .inner
            .get(
                collection,
                id,
                GetOptions {
                    with_fields,
                    output: OutputFormat::Objects,
                },
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&object)
    }

    pub fn delete_object(&self, collection: &str, id: &str) -> Result<JsValue, JsValue> {
        let (_, _, webhook_jobs) = self
            .inner
            .preview_del_command_batch(collection, id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let deleted = self
            .inner
            .del(collection, id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&WasmMutationResponse {
            ok: true,
            result: deleted,
            events: webhook_jobs
                .into_iter()
                .map(encode_wasm_webhook_enqueue)
                .map(|record| record.event)
                .collect(),
        })
    }

    pub fn expire(&self, collection: &str, id: &str, seconds: u32) -> Result<JsValue, JsValue> {
        self.inner
            .expire(collection, id, seconds)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&WasmMutationResponse {
            ok: true,
            result: true,
            events: Vec::new(),
        })
    }

    pub fn persist(&self, collection: &str, id: &str) -> Result<JsValue, JsValue> {
        self.inner
            .persist(collection, id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&WasmMutationResponse {
            ok: true,
            result: true,
            events: Vec::new(),
        })
    }

    pub fn ttl(&self, collection: &str, id: &str) -> Result<JsValue, JsValue> {
        let ttl = self
            .inner
            .ttl(collection, id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&ttl)
    }

    pub fn set_hook(&self, name: &str, request: JsValue) -> Result<(), JsValue> {
        let request = serde_wasm_bindgen::from_value::<WasmHookDefRequest>(request)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.inner
            .sethook(name, "browser://geofence-event", request.def)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn delete_hook(&self, name: &str) -> Result<bool, JsValue> {
        self.inner
            .delhook(name)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    pub fn hooks(&self, pattern: Option<String>) -> Result<JsValue, JsValue> {
        let pattern = pattern.unwrap_or_else(|| "*".to_owned());
        let hooks = self
            .inner
            .hooks(&pattern)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&hooks)
    }

    pub fn hook(&self, name: &str) -> Result<JsValue, JsValue> {
        to_js(&self.inner.hook_def(name))
    }

    pub fn nearby_query(&self, collection: &str, query: JsValue) -> Result<JsValue, JsValue> {
        let query = serde_wasm_bindgen::from_value::<NearbyQuery>(query)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let results = self
            .inner
            .nearby(collection, query)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&results)
    }

    pub fn within_query(&self, collection: &str, query: JsValue) -> Result<JsValue, JsValue> {
        let query = serde_wasm_bindgen::from_value::<WasmAreaQuery>(query)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let results = self
            .inner
            .within(collection, query.area, query.options)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&results)
    }

    pub fn intersects_query(&self, collection: &str, query: JsValue) -> Result<JsValue, JsValue> {
        let query = serde_wasm_bindgen::from_value::<WasmAreaQuery>(query)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let results = self
            .inner
            .intersects(collection, query.area, query.options)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&results)
    }

    pub fn scan(&self, collection: &str, options: JsValue) -> Result<JsValue, JsValue> {
        let options = if options.is_undefined() || options.is_null() {
            SearchOptions::default()
        } else {
            serde_wasm_bindgen::from_value::<SearchOptions>(options)
                .map_err(|error| JsValue::from_str(&error.to_string()))?
        };
        let results = self
            .inner
            .scan(collection, options)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&results)
    }

    pub fn search(&self, collection: &str, options: JsValue) -> Result<JsValue, JsValue> {
        let options = if options.is_undefined() || options.is_null() {
            SearchOptions::default()
        } else {
            serde_wasm_bindgen::from_value::<SearchOptions>(options)
                .map_err(|error| JsValue::from_str(&error.to_string()))?
        };
        let results = self
            .inner
            .search(collection, options)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&results)
    }

    pub fn bounds(&self, collection: &str) -> Result<JsValue, JsValue> {
        let bounds = self
            .inner
            .bounds(collection)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&bounds)
    }

    pub fn stats(&self, collections: JsValue) -> Result<JsValue, JsValue> {
        let collections = serde_wasm_bindgen::from_value::<Vec<String>>(collections)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let refs = collections.iter().map(String::as_str).collect::<Vec<_>>();
        let stats = self
            .inner
            .stats(&refs)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        to_js(&stats)
    }

    pub fn server_info(&self) -> Result<JsValue, JsValue> {
        to_js(&self.inner.server_info())
    }
}

#[cfg(feature = "wasm-bindings")]
fn encode_wasm_storage_entry(entry: StorageEntry) -> WasmStorageEntry {
    WasmStorageEntry {
        sequence: entry.sequence.to_string(),
        timestamp_ns: entry.timestamp_ns.to_string(),
        command_base64: STANDARD.encode(entry.command),
    }
}

fn encode_wasm_webhook_enqueue(record: WebhookEnqueueRecord) -> WasmWebhookEnqueueRecord {
    WasmWebhookEnqueueRecord {
        job_id: record.job_id,
        event: WasmGeofenceEvent {
            command: record.event.command,
            detect: record.event.detect,
            collection: record.event.collection,
            id: record.event.id,
            object: record.event.object,
            fields: record.event.fields,
            timestamp_ns: record.event.timestamp_ns.to_string(),
            event_id: record.event.event_id,
            job_id: record.event.job_id,
            hook: record.event.hook,
            group: record.event.group,
            nearby: record.event.nearby,
        },
        endpoint: record.endpoint,
        attempts_used: record.attempts_used,
        max_attempts: record.max_attempts,
        next_attempt_at_ms: record.next_attempt_at_ms,
    }
}

#[cfg(feature = "wasm-browser-bindings")]
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(|error| JsValue::from_str(&error.to_string()))
}

#[cfg(feature = "wasm-bindings")]
fn decode_wasm_storage_entry(entry: WasmStorageEntry) -> std::result::Result<StorageEntry, String> {
    let sequence = entry
        .sequence
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    let timestamp_ns = entry
        .timestamp_ns
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    let command = STANDARD
        .decode(entry.command_base64)
        .map_err(|error: base64::DecodeError| error.to_string())?;
    Ok(StorageEntry {
        sequence,
        timestamp_ns,
        command: Bytes::from(command),
    })
}

#[cfg(feature = "wasm-bindings")]
fn encode_log_entry_js(
    sequence: &str,
    timestamp_ns: &str,
    record: LogRecord,
) -> Result<JsValue, JsValue> {
    let sequence = sequence
        .parse::<u64>()
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let timestamp_ns = timestamp_ns
        .parse::<u64>()
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let command =
        super::encode_log_record(&record).map_err(|error| JsValue::from_str(&error.to_string()))?;
    serde_wasm_bindgen::to_value(&WasmStorageEntry {
        sequence: sequence.to_string(),
        timestamp_ns: timestamp_ns.to_string(),
        command_base64: STANDARD.encode(command),
    })
    .map_err(|error| JsValue::from_str(&error.to_string()))
}

#[cfg(feature = "wasm-bindings")]
fn encode_log_record_js_value(record: &LogRecord) -> Result<JsValue, JsValue> {
    match record {
        LogRecord::WebhookEnqueue(record) => serde_wasm_bindgen::to_value(&serde_json::json!({
            "WebhookEnqueue": encode_wasm_webhook_enqueue(record.clone())
        }))
        .map_err(|error| JsValue::from_str(&error.to_string())),
        LogRecord::WebhookAck(record) => serde_wasm_bindgen::to_value(&serde_json::json!({
            "WebhookAck": record
        }))
        .map_err(|error| JsValue::from_str(&error.to_string())),
        LogRecord::WebhookRetryScheduled(record) => {
            serde_wasm_bindgen::to_value(&serde_json::json!({ "WebhookRetryScheduled": record }))
                .map_err(|error| JsValue::from_str(&error.to_string()))
        }
        LogRecord::WebhookDeadLetter(record) => {
            serde_wasm_bindgen::to_value(&serde_json::json!({ "WebhookDeadLetter": record }))
                .map_err(|error| JsValue::from_str(&error.to_string()))
        }
        _ => serde_wasm_bindgen::to_value(record)
            .map_err(|error| JsValue::from_str(&error.to_string())),
    }
}
