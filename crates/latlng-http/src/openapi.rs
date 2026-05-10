#![forbid(unsafe_code)]
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{OpenApi, ToSchema};

#[derive(OpenApi)]
#[openapi(
    paths(
        api_docs,
        ping,
        healthz,
        server,
        info,
        metrics,
        collections,
        collection_get,
        collection_create_post,
        collection_create_put,
        collection_delete,
        collection_rename,
        collection_bounds,
        collection_stats,
        object_set,
        object_get,
        object_delete,
        objects_delete_matching,
        object_fields_set,
        object_field_get,
        object_expire,
        object_persist,
        object_ttl,
        object_json_set,
        object_json_get,
        object_json_delete,
        search_nearby,
        search_within,
        search_intersects,
        search_scan,
        search_text,
        channels_post,
        channels_get,
        channel_get,
        channel_delete,
        hooks_post,
        hooks_get,
        hook_get,
        hook_delete,
        config_get,
        config_set,
        admin_flushdb,
        admin_gc,
        admin_aofshrink,
        admin_webhook_queue,
        admin_config_rewrite,
        admin_readonly,
        admin_timeout
    ),
    components(schemas(
        ErrorResponse,
        OpenApiDocument,
        PingResponse,
        HealthResponse,
        OkResponse,
        ServerInfoResponse,
        InfoResponse,
        CollectionsResponse,
        CollectionInfoResponse,
        CreateCollectionResponse,
        DropCollectionResponse,
        RenameRequest,
        RenameResponse,
        BoundsResponse,
        StatsResponse,
        FieldEntrySchema,
        SetObjectRequest,
        SetObjectResponse,
        GetObjectResponse,
        DeleteResponse,
        DeleteCountResponse,
        FsetRequest,
        UpdatedResponse,
        FieldValueResponse,
        ExpireRequest,
        TtlResponse,
        JsetRequest,
        JsonValueResponse,
        NearbyRequest,
        AreaSearchRequest,
        SearchOptionsSchema,
        SearchResponse,
        SetChannelRequest,
        ChannelMutationResponse,
        ChannelsResponse,
        ChannelDefResponse,
        SetHookRequest,
        HookMutationResponse,
        HooksResponse,
        HookDefResponse,
        ConfigValueRequest,
        ConfigValueResponse,
        AofshrinkResponse,
        WebhookQueueStatsResponse,
        ReadonlyRequest,
        ReadonlyResponse,
        TimeoutRequest,
        TimeoutResponse
    )),
    tags(
        (name = "system", description = "Server status, docs, and metrics"),
        (name = "collections", description = "Collection lifecycle and metadata"),
        (name = "objects", description = "Object CRUD and field operations"),
        (name = "queries", description = "Spatial and text queries"),
        (name = "channels", description = "In-process geofence subscriptions"),
        (name = "hooks", description = "Durable webhook geofences"),
        (name = "admin", description = "Administrative operations")
    )
)]
struct ApiDoc;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OpenApiDocument {
    #[serde(flatten)]
    #[schema(value_type = Object)]
    pub document: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PingResponse {
    pub ok: bool,
    pub pong: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerInfoResponse {
    pub version: String,
    pub api_version: String,
    pub protocol_version: String,
    pub storage_format_version: String,
    pub num_collections: u32,
    pub num_objects: u64,
    pub num_points: u64,
    pub heap_bytes: u64,
    pub read_only: bool,
    pub leader: bool,
    pub server_id: String,
    pub following: Option<String>,
    pub caught_up: bool,
    pub caught_up_once: bool,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InfoResponse {
    #[schema(value_type = Object)]
    pub server: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub metrics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CollectionsResponse {
    pub collections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CollectionInfoResponse {
    pub name: String,
    #[schema(value_type = Object)]
    pub bounds: Value,
    #[schema(value_type = Object)]
    pub stats: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateCollectionResponse {
    pub ok: bool,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DropCollectionResponse {
    pub dropped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RenameRequest {
    pub new_name: String,
    pub nx: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RenameResponse {
    pub ok: bool,
    pub renamed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BoundsResponse {
    #[schema(value_type = Object)]
    pub bounds: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StatsResponse {
    #[schema(value_type = Object)]
    pub stats: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FieldEntrySchema {
    pub name: String,
    #[schema(value_type = Object)]
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetObjectRequest {
    #[schema(value_type = Object)]
    pub object: Value,
    #[serde(default)]
    pub fields: Vec<FieldEntrySchema>,
    pub expire_seconds: Option<u32>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetObjectResponse {
    pub ok: bool,
    pub stored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GetObjectResponse {
    #[schema(value_type = Object)]
    pub object: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeleteResponse {
    pub ok: Option<bool>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeleteCountResponse {
    pub ok: bool,
    pub deleted: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsetRequest {
    pub fields: Vec<FieldEntrySchema>,
    pub xx: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdatedResponse {
    pub ok: bool,
    pub updated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FieldValueResponse {
    #[schema(value_type = Object)]
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExpireRequest {
    pub seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TtlResponse {
    pub ttl: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JsetRequest {
    pub path: String,
    pub value: String,
    pub raw: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JsonValueResponse {
    #[schema(value_type = Object)]
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SearchOptionsSchema {
    #[schema(value_type = Object)]
    pub options: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NearbyRequest {
    pub lat: f64,
    pub lon: f64,
    pub meters: f64,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub options: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AreaSearchRequest {
    #[schema(value_type = Object)]
    pub area: Value,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub options: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SearchResponse {
    #[schema(value_type = Object)]
    pub results: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetChannelRequest {
    pub name: String,
    #[schema(value_type = Object)]
    pub def: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChannelMutationResponse {
    pub ok: Option<bool>,
    pub name: Option<String>,
    pub deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChannelsResponse {
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChannelDefResponse {
    pub name: String,
    #[schema(value_type = Object)]
    pub def: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetHookRequest {
    pub name: String,
    pub endpoint: String,
    #[schema(value_type = Object)]
    pub def: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HookMutationResponse {
    pub ok: Option<bool>,
    pub deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HooksResponse {
    #[schema(value_type = Vec<Object>)]
    pub hooks: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HookDefResponse {
    pub name: String,
    pub endpoint: String,
    #[schema(value_type = Object)]
    pub def: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConfigValueRequest {
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConfigValueResponse {
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AofshrinkResponse {
    pub before_entries: u64,
    pub after_entries: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WebhookQueueStatsResponse {
    pub pending: u64,
    pub leased: u64,
    pub dead_letter: u64,
    pub oldest_pending_age_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReadonlyRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReadonlyResponse {
    pub ok: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TimeoutRequest {
    pub command: String,
    pub seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TimeoutResponse {
    pub ok: bool,
    pub command: String,
    pub seconds: Option<f64>,
}

pub fn spec() -> Value {
    let mut spec = serde_json::to_value(ApiDoc::openapi()).expect("generated OpenAPI is JSON");
    spec["openapi"] = json!("3.0.3");
    spec["info"] = json!({
        "title": "latlng HTTP API",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Stable native HTTP API for latlng. Replication management and diagnostic test endpoints are intentionally excluded from the public API document.",
    });
    spec["components"]["securitySchemes"]["bearerAuth"] = json!({
        "type": "http",
        "scheme": "bearer",
        "bearerFormat": "JWT"
    });
    spec["security"] = json!([{ "bearerAuth": [] }]);
    spec
}

#[utoipa::path(get, path = "/api-docs", tag = "system", responses((status = 200, description = "Generated OpenAPI v3 document", body = OpenApiDocument)))]
fn api_docs() {}

#[utoipa::path(get, path = "/ping", tag = "system", responses((status = 200, description = "Liveness probe", body = PingResponse)))]
fn ping() {}

#[utoipa::path(get, path = "/healthz", tag = "system", responses((status = 200, description = "Health and readiness probe", body = HealthResponse), (status = 503, description = "Follower is catching up", body = ErrorResponse)))]
fn healthz() {}

#[utoipa::path(get, path = "/server", tag = "system", responses((status = 200, description = "Server runtime information", body = ServerInfoResponse), (status = 403, description = "Missing admin scope", body = ErrorResponse)))]
fn server() {}

#[utoipa::path(get, path = "/info", tag = "system", params(("section" = Option<String>, Query, description = "Optional section. Use `stats` to include JSON metrics.")), responses((status = 200, description = "Grouped server information", body = InfoResponse)))]
fn info() {}

#[utoipa::path(get, path = "/metrics", tag = "system", responses((status = 200, description = "Prometheus text exposition metrics", content_type = "text/plain; version=0.0.4", body = String), (status = 403, description = "Missing metrics:read scope", body = ErrorResponse)))]
fn metrics() {}

#[utoipa::path(get, path = "/collections", tag = "collections", params(("match_pattern" = Option<String>, Query, description = "Glob pattern used to filter collection names.")), responses((status = 200, description = "Visible collection names", body = CollectionsResponse)))]
fn collections() {}

#[utoipa::path(get, path = "/collections/{collection}", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), responses((status = 200, description = "Collection metadata", body = CollectionInfoResponse), (status = 404, description = "Collection was not found", body = ErrorResponse)))]
fn collection_get() {}

#[utoipa::path(post, path = "/collections/{collection}", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), responses((status = 200, description = "Collection creation status", body = CreateCollectionResponse)))]
fn collection_create_post() {}

#[utoipa::path(put, path = "/collections/{collection}", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), responses((status = 200, description = "Collection creation status", body = CreateCollectionResponse)))]
fn collection_create_put() {}

#[utoipa::path(delete, path = "/collections/{collection}", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), responses((status = 200, description = "Collection deletion status", body = DropCollectionResponse)))]
fn collection_delete() {}

#[utoipa::path(post, path = "/collections/{collection}/rename", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), request_body = RenameRequest, responses((status = 200, description = "Rename status", body = RenameResponse)))]
fn collection_rename() {}

#[utoipa::path(get, path = "/collections/{collection}/bounds", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), responses((status = 200, description = "Collection bounds", body = BoundsResponse)))]
fn collection_bounds() {}

#[utoipa::path(get, path = "/collections/{collection}/stats", tag = "collections", params(("collection" = String, Path, description = "Collection name.")), responses((status = 200, description = "Collection stats", body = StatsResponse)))]
fn collection_stats() {}

#[utoipa::path(post, path = "/collections/{collection}/objects/{id}", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), request_body = SetObjectRequest, responses((status = 200, description = "Object write result", body = SetObjectResponse)))]
fn object_set() {}

#[utoipa::path(get, path = "/collections/{collection}/objects/{id}", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id."), ("with_fields" = Option<bool>, Query, description = "Include fields."), ("format" = Option<String>, Query, description = "Projection format."), ("hash_precision" = Option<u8>, Query, description = "Geohash precision when format=hashes.")), responses((status = 200, description = "Object payload or null", body = GetObjectResponse)))]
fn object_get() {}

#[utoipa::path(delete, path = "/collections/{collection}/objects/{id}", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), responses((status = 200, description = "Object deletion status", body = DeleteResponse)))]
fn object_delete() {}

#[utoipa::path(delete, path = "/collections/{collection}/objects", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("match_pattern" = Option<String>, Query, description = "Glob pattern used to select object ids.")), responses((status = 200, description = "Matching deletion status", body = DeleteCountResponse)))]
fn objects_delete_matching() {}

#[utoipa::path(post, path = "/collections/{collection}/objects/{id}/fields", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), request_body = FsetRequest, responses((status = 200, description = "Field update status", body = UpdatedResponse)))]
fn object_fields_set() {}

#[utoipa::path(get, path = "/collections/{collection}/objects/{id}/fields/{field}", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id."), ("field" = String, Path, description = "Field name.")), responses((status = 200, description = "Field value", body = FieldValueResponse)))]
fn object_field_get() {}

#[utoipa::path(post, path = "/collections/{collection}/objects/{id}/expire", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), request_body = ExpireRequest, responses((status = 200, description = "Expiration status", body = OkResponse)))]
fn object_expire() {}

#[utoipa::path(delete, path = "/collections/{collection}/objects/{id}/expire", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), responses((status = 200, description = "Persistence status", body = OkResponse)))]
fn object_persist() {}

#[utoipa::path(get, path = "/collections/{collection}/objects/{id}/ttl", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), responses((status = 200, description = "Object TTL", body = TtlResponse)))]
fn object_ttl() {}

#[utoipa::path(post, path = "/collections/{collection}/objects/{id}/json", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id.")), request_body = JsetRequest, responses((status = 200, description = "JSON mutation status", body = OkResponse)))]
fn object_json_set() {}

#[utoipa::path(get, path = "/collections/{collection}/objects/{id}/json/{path}", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id."), ("path" = String, Path, description = "JSON path.")), responses((status = 200, description = "JSON value", body = JsonValueResponse)))]
fn object_json_get() {}

#[utoipa::path(delete, path = "/collections/{collection}/objects/{id}/json/{path}", tag = "objects", params(("collection" = String, Path, description = "Collection name."), ("id" = String, Path, description = "Object id."), ("path" = String, Path, description = "JSON path.")), responses((status = 200, description = "JSON deletion status", body = DeleteResponse)))]
fn object_json_delete() {}

#[utoipa::path(post, path = "/collections/{collection}/search/nearby", tag = "queries", params(("collection" = String, Path, description = "Collection name.")), request_body = NearbyRequest, responses((status = 200, description = "Nearby query results", body = SearchResponse)))]
fn search_nearby() {}

#[utoipa::path(post, path = "/collections/{collection}/search/within", tag = "queries", params(("collection" = String, Path, description = "Collection name.")), request_body = AreaSearchRequest, responses((status = 200, description = "Within query results", body = SearchResponse)))]
fn search_within() {}

#[utoipa::path(post, path = "/collections/{collection}/search/intersects", tag = "queries", params(("collection" = String, Path, description = "Collection name.")), request_body = AreaSearchRequest, responses((status = 200, description = "Intersects query results", body = SearchResponse)))]
fn search_intersects() {}

#[utoipa::path(post, path = "/collections/{collection}/search/scan", tag = "queries", params(("collection" = String, Path, description = "Collection name.")), request_body = SearchOptionsSchema, responses((status = 200, description = "Scan query results", body = SearchResponse)))]
fn search_scan() {}

#[utoipa::path(post, path = "/collections/{collection}/search/text", tag = "queries", params(("collection" = String, Path, description = "Collection name.")), request_body = SearchOptionsSchema, responses((status = 200, description = "Text query results", body = SearchResponse)))]
fn search_text() {}

#[utoipa::path(post, path = "/channels", tag = "channels", request_body = SetChannelRequest, responses((status = 200, description = "Channel creation status", body = ChannelMutationResponse)))]
fn channels_post() {}

#[utoipa::path(get, path = "/channels", tag = "channels", params(("match_pattern" = Option<String>, Query, description = "Glob pattern used to filter channel names.")), responses((status = 200, description = "Matching channel names", body = ChannelsResponse)))]
fn channels_get() {}

#[utoipa::path(get, path = "/channels/{name}", tag = "channels", params(("name" = String, Path, description = "Channel name.")), responses((status = 200, description = "Channel definition", body = ChannelDefResponse), (status = 404, description = "Channel was not found", body = ErrorResponse)))]
fn channel_get() {}

#[utoipa::path(delete, path = "/channels/{name}", tag = "channels", params(("name" = String, Path, description = "Channel name.")), responses((status = 200, description = "Channel deletion status", body = ChannelMutationResponse)))]
fn channel_delete() {}

#[utoipa::path(post, path = "/hooks", tag = "hooks", request_body = SetHookRequest, responses((status = 200, description = "Hook creation status", body = HookMutationResponse)))]
fn hooks_post() {}

#[utoipa::path(get, path = "/hooks", tag = "hooks", params(("match_pattern" = Option<String>, Query, description = "Glob pattern used to filter hook names.")), responses((status = 200, description = "Matching hook summaries", body = HooksResponse)))]
fn hooks_get() {}

#[utoipa::path(get, path = "/hooks/{name}", tag = "hooks", params(("name" = String, Path, description = "Hook name.")), responses((status = 200, description = "Hook definition", body = HookDefResponse), (status = 404, description = "Hook was not found", body = ErrorResponse)))]
fn hook_get() {}

#[utoipa::path(delete, path = "/hooks/{name}", tag = "hooks", params(("name" = String, Path, description = "Hook name.")), responses((status = 200, description = "Hook deletion status", body = HookMutationResponse)))]
fn hook_delete() {}

#[utoipa::path(get, path = "/config/{name}", tag = "admin", params(("name" = String, Path, description = "Runtime config key.")), responses((status = 200, description = "Runtime config value", body = ConfigValueResponse), (status = 404, description = "Unknown config key", body = ErrorResponse)))]
fn config_get() {}

#[utoipa::path(post, path = "/config/{name}", tag = "admin", params(("name" = String, Path, description = "Runtime config key.")), request_body = ConfigValueRequest, responses((status = 200, description = "Runtime config update status", body = OkResponse), (status = 404, description = "Unknown config key", body = ErrorResponse)))]
fn config_set() {}

#[utoipa::path(post, path = "/admin/flushdb", tag = "admin", responses((status = 200, description = "Flush status", body = OkResponse)))]
fn admin_flushdb() {}

#[utoipa::path(post, path = "/admin/gc", tag = "admin", responses((status = 200, description = "GC status", body = OkResponse)))]
fn admin_gc() {}

#[utoipa::path(post, path = "/admin/aofshrink", tag = "admin", responses((status = 200, description = "Compaction result", body = AofshrinkResponse)))]
fn admin_aofshrink() {}

#[utoipa::path(get, path = "/admin/webhook-queue", tag = "admin", responses((status = 200, description = "Webhook queue stats", body = WebhookQueueStatsResponse)))]
fn admin_webhook_queue() {}

#[utoipa::path(post, path = "/admin/config/rewrite", tag = "admin", responses((status = 200, description = "Config rewrite status", body = OkResponse)))]
fn admin_config_rewrite() {}

#[utoipa::path(post, path = "/admin/readonly", tag = "admin", request_body = ReadonlyRequest, responses((status = 200, description = "Readonly state", body = ReadonlyResponse)))]
fn admin_readonly() {}

#[utoipa::path(post, path = "/admin/timeout", tag = "admin", request_body = TimeoutRequest, responses((status = 200, description = "Timeout state", body = TimeoutResponse)))]
fn admin_timeout() {}
