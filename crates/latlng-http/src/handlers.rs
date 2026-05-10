use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use latlng_auth::{AuthAction, AuthPrincipal};
use latlng_config::{RuntimeConfig, SharedRuntimeConfig, save_to_path};
use latlng_core::index::{OutputFormat, SearchOptions};
use latlng_core::storage::StorageBackend;
use latlng_core::{FieldEntry, GetOptions, LatLngNative, NearbyQuery, SetCondition, SetRequest};
use latlng_geofence::GeofenceDef;
use latlng_native_executor::NativeExecutor;
use latlng_replication::ReplicationStatus;
use serde::Deserialize;
use serde_json::Value;

use crate::authz::{
    authenticate_headers, cached_auth_principal, ensure_admin, ensure_collection_action,
    ensure_global_action,
};
use crate::{HttpError, HttpState, openapi_spec};

pub(crate) async fn ping<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    Ok(Json(serde_json::json!({ "ok": true, "pong": true })))
}

pub(crate) async fn healthz<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_queries_allowed(&state)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn run_db<S, T, F>(executor: &NativeExecutor<S>, op: F) -> Result<T, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
    T: Send + 'static,
    F: FnOnce(&LatLngNative<S>) -> latlng_core::Result<T> + Send + 'static,
{
    executor
        .execute(op)
        .await
        .map_err(internal)?
        .map_err(core_error_to_http)
}

fn core_error_to_http(error: latlng_core::CoreError) -> HttpError {
    match error {
        latlng_core::CoreError::ReadOnly => {
            HttpError::BadRequest("read-only mode is enabled".to_owned())
        }
        other => internal(other),
    }
}

pub(crate) async fn server<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let mut info = run_db(&state.executor, move |db| Ok(db.server_info())).await?;
    apply_replication_to_server_info(&mut info, replication_snapshot(&state).as_ref());
    Ok(Json(serde_json::to_value(info).map_err(internal)?))
}

pub(crate) async fn info<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Query(query): Query<InfoQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let section = query.section.clone();
    let mut info = run_db(&state.executor, move |db| Ok(db.server_info())).await?;
    apply_replication_to_server_info(&mut info, replication_snapshot(&state).as_ref());
    let value = match section.as_deref().unwrap_or("server") {
        "" | "server" => serde_json::json!({ "server": info }),
        "stats" => serde_json::json!({
            "server": info,
            "metrics": state.metrics.snapshot()
        }),
        other => serde_json::json!({
            "section": other,
            "server": info
        }),
    };
    Ok(Json(value))
}

pub(crate) async fn metrics<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Response, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_global_action(&principal, AuthAction::MetricsRead)?;
    let replication = replication_snapshot(&state);
    let local_last_sequence = run_db(&state.executor, move |db| Ok(db.last_sequence()))
        .await
        .ok();
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state
            .metrics
            .prometheus_text_with_replication(replication.as_ref(), local_last_sequence),
    )
        .into_response())
}

pub(crate) async fn api_docs<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    Ok(Json(openapi_spec()))
}

#[derive(Deserialize)]
pub(crate) struct MatchQuery {
    match_pattern: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct InfoQuery {
    section: Option<String>,
}

pub(crate) async fn collections<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Query(query): Query<MatchQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_queries_allowed(&state)?;
    let pattern = query.match_pattern.unwrap_or_else(|| "*".to_owned());
    let names = run_db(&state.executor, move |db| db.collections(&pattern)).await?;
    if !principal.any_collection_permission(AuthAction::CollectionsList) && !principal.is_admin() {
        return Err(HttpError::Forbidden);
    }
    let visible = names
        .into_iter()
        .filter(|name| principal.can_view_collection(name))
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({ "collections": visible })))
}

pub(crate) async fn collection_info<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::CollectionsInspect, &collection)?;
    ensure_queries_allowed(&state)?;
    let collection_name = collection.clone();
    let (stats, bounds) = run_db(&state.executor, move |db| {
        Ok((
            db.stats(&[&collection])?.into_iter().next(),
            db.bounds(&collection)?,
        ))
    })
    .await?;
    let Some(stats) = stats else {
        return Err(HttpError::NotFound(format!(
            "collection not found: {collection_name}"
        )));
    };
    Ok(Json(serde_json::json!({
        "name": collection_name,
        "bounds": bounds,
        "stats": stats,
    })))
}

pub(crate) async fn create_collection<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::CollectionsCreate, &collection)?;
    let created = run_db(&state.executor, move |db| db.create_collection(&collection)).await?;
    if created {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true, "created": created })))
}

#[derive(Deserialize)]
pub(crate) struct RenameBody {
    new_name: String,
    nx: Option<bool>,
}

pub(crate) async fn rename<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    let nx = body.nx.unwrap_or(false);
    let new_name = body.new_name;
    ensure_collection_action(&principal, AuthAction::CollectionsDelete, &collection)?;
    ensure_collection_action(&principal, AuthAction::CollectionsCreate, &new_name)?;
    let renamed = run_db(&state.executor, move |db| {
        if nx {
            db.renamenx(&collection, &new_name)
        } else {
            db.rename(&collection, &new_name)?;
            Ok(true)
        }
    })
    .await?;
    if renamed {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true, "renamed": renamed })))
}

pub(crate) async fn bounds<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::CollectionsInspect, &collection)?;
    ensure_queries_allowed(&state)?;
    let bounds = run_db(&state.executor, move |db| db.bounds(&collection)).await?;
    Ok(Json(serde_json::json!({ "bounds": bounds })))
}

pub(crate) async fn stats<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::CollectionsInspect, &collection)?;
    ensure_queries_allowed(&state)?;
    let stats = run_db(&state.executor, move |db| db.stats(&[&collection])).await?;
    Ok(Json(serde_json::to_value(stats).map_err(internal)?))
}

#[derive(Deserialize)]
pub(crate) struct SetObjectBody {
    object: latlng_core::geo::GeoType,
    #[serde(default)]
    fields: Vec<FieldEntry>,
    expire_seconds: Option<u32>,
    condition: Option<SetCondition>,
}

pub(crate) async fn set_object<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
    Json(body): Json<SetObjectBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsWrite, &collection)?;
    let stored = run_db(&state.executor, move |db| {
        db.set(SetRequest {
            collection,
            id,
            object: body.object,
            fields: body.fields,
            expire_seconds: body.expire_seconds,
            condition: body.condition.unwrap_or(SetCondition::Always),
        })
    })
    .await?;
    if stored {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true, "stored": stored })))
}

#[derive(Deserialize, Default)]
pub(crate) struct GetQuery {
    with_fields: Option<bool>,
    format: Option<String>,
    hash_precision: Option<u8>,
}

pub(crate) async fn get_object<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
    Query(query): Query<GetQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let output = parse_output_format(query.format.as_deref(), query.hash_precision)?;
    let with_fields = query.with_fields.unwrap_or(false);
    let object = run_db(&state.executor, move |db| {
        db.get(
            &collection,
            &id,
            GetOptions {
                with_fields,
                output,
            },
        )
    })
    .await?;
    Ok(Json(serde_json::to_value(object).map_err(internal)?))
}

pub(crate) async fn delete_object<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsDelete, &collection)?;
    let deleted = run_db(&state.executor, move |db| db.del(&collection, &id)).await?;
    if deleted {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true, "deleted": deleted })))
}

pub(crate) async fn delete_matching_objects<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Query(query): Query<MatchQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsDelete, &collection)?;
    let pattern = query.match_pattern.unwrap_or_else(|| "*".to_owned());
    let deleted = run_db(&state.executor, move |db| db.pdel(&collection, &pattern)).await?;
    if deleted > 0 {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true, "deleted": deleted })))
}

#[derive(Deserialize)]
pub(crate) struct FsetBody {
    fields: Vec<FieldEntry>,
    xx: Option<bool>,
}

pub(crate) async fn fset<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
    Json(body): Json<FsetBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsWrite, &collection)?;
    let fields = body.fields;
    let xx = body.xx.unwrap_or(false);
    let updated = run_db(&state.executor, move |db| {
        db.fset(&collection, &id, &fields, xx)
    })
    .await?;
    if updated {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true, "updated": updated })))
}

pub(crate) async fn fget<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id, field)): Path<(String, String, String)>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let value = run_db(&state.executor, move |db| db.fget(&collection, &id, &field)).await?;
    Ok(Json(serde_json::to_value(value).map_err(internal)?))
}

#[derive(Deserialize)]
pub(crate) struct ExpireBody {
    seconds: u32,
}

pub(crate) async fn expire<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
    Json(body): Json<ExpireBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsWrite, &collection)?;
    let seconds = body.seconds;
    run_db(&state.executor, move |db| {
        db.expire(&collection, &id, seconds)
    })
    .await?;
    notify_outbox(&state);
    notify_replication(&state);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn persist<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsWrite, &collection)?;
    run_db(&state.executor, move |db| db.persist(&collection, &id)).await?;
    notify_outbox(&state);
    notify_replication(&state);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn ttl<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let ttl = run_db(&state.executor, move |db| db.ttl(&collection, &id)).await?;
    Ok(Json(serde_json::json!({ "ttl": ttl })))
}

#[derive(Deserialize)]
pub(crate) struct JsetBody {
    path: String,
    value: String,
    raw: Option<bool>,
}

pub(crate) async fn jset<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id)): Path<(String, String)>,
    Json(body): Json<JsetBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsWrite, &collection)?;
    let path = body.path;
    let value = body.value;
    let raw = body.raw.unwrap_or(false);
    run_db(&state.executor, move |db| {
        db.jset(&collection, &id, &path, &value, raw)
    })
    .await?;
    notify_outbox(&state);
    notify_replication(&state);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn jget<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id, path)): Path<(String, String, String)>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let value = run_db(&state.executor, move |db| db.jget(&collection, &id, &path)).await?;
    Ok(Json(serde_json::json!({ "value": value })))
}

pub(crate) async fn jdel<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path((collection, id, path)): Path<(String, String, String)>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ObjectsWrite, &collection)?;
    let deleted = run_db(&state.executor, move |db| db.jdel(&collection, &id, &path)).await?;
    if deleted {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

pub(crate) async fn nearby<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Json(body): Json<NearbyQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::QueriesRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let results = run_db(&state.executor, move |db| db.nearby(&collection, body)).await?;
    Ok(Json(serde_json::to_value(results).map_err(internal)?))
}

#[derive(Deserialize)]
pub(crate) struct AreaSearchBody {
    area: latlng_core::geo::Area,
    #[serde(default)]
    options: SearchOptions,
}

pub(crate) async fn within<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Json(body): Json<AreaSearchBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::QueriesRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let results = run_db(&state.executor, move |db| {
        db.within(&collection, body.area, body.options)
    })
    .await?;
    Ok(Json(serde_json::to_value(results).map_err(internal)?))
}

pub(crate) async fn intersects<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Json(body): Json<AreaSearchBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::QueriesRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let results = run_db(&state.executor, move |db| {
        db.intersects(&collection, body.area, body.options)
    })
    .await?;
    Ok(Json(serde_json::to_value(results).map_err(internal)?))
}

pub(crate) async fn scan<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Json(options): Json<SearchOptions>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::QueriesRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let results = run_db(&state.executor, move |db| db.scan(&collection, options)).await?;
    Ok(Json(serde_json::to_value(results).map_err(internal)?))
}

pub(crate) async fn search<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
    Json(options): Json<SearchOptions>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::QueriesRead, &collection)?;
    ensure_queries_allowed(&state)?;
    let results = run_db(&state.executor, move |db| db.search(&collection, options)).await?;
    Ok(Json(serde_json::to_value(results).map_err(internal)?))
}

#[derive(Deserialize)]
pub(crate) struct SetChannelBody {
    name: String,
    def: GeofenceDef,
}

pub(crate) async fn setchan<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Json(body): Json<SetChannelBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::ChannelsManage, &body.def.collection)?;
    let name = body.name;
    let def = body.def;
    let response_name = name.clone();
    run_db(&state.executor, move |db| db.setchan(&name, def)).await?;
    notify_outbox(&state);
    notify_replication(&state);
    Ok(Json(
        serde_json::json!({ "ok": true, "name": response_name }),
    ))
}

pub(crate) async fn delchan<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    let existing = run_db(&state.executor, {
        let lookup = name.clone();
        move |db| Ok(db.channel_def(&lookup))
    })
    .await?;
    if let Some(channel) = existing.as_ref() {
        ensure_collection_action(
            &principal,
            AuthAction::ChannelsManage,
            &channel.def.collection,
        )?;
    }
    let deleted = run_db(&state.executor, move |db| db.delchan(&name)).await?;
    if deleted {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

pub(crate) async fn channel<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_queries_allowed(&state)?;
    let channel_name = name.clone();
    let channel = run_db(&state.executor, move |db| Ok(db.channel_def(&name))).await?;
    let Some(channel) = channel else {
        return Err(HttpError::NotFound(format!(
            "channel not found: {channel_name}"
        )));
    };
    ensure_collection_action(
        &principal,
        AuthAction::ChannelsManage,
        &channel.def.collection,
    )?;
    Ok(Json(serde_json::to_value(channel).map_err(internal)?))
}

pub(crate) async fn channels<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Query(query): Query<MatchQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_queries_allowed(&state)?;
    let pattern = query.match_pattern.unwrap_or_else(|| "*".to_owned());
    let channel_names = run_db(&state.executor, {
        let pattern = pattern.clone();
        move |db| db.chans(&pattern)
    })
    .await?;
    let channels = run_db(&state.executor, move |db| {
        Ok(channel_names
            .into_iter()
            .filter_map(|name| db.channel_def(&name))
            .collect::<Vec<_>>())
    })
    .await?;
    let visible = channels
        .into_iter()
        .filter(|channel| principal.allows(AuthAction::ChannelsManage, &channel.def.collection))
        .map(|channel| channel.name)
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({ "channels": visible })))
}

#[derive(Deserialize)]
pub(crate) struct SetHookBody {
    name: String,
    endpoint: String,
    def: GeofenceDef,
}

pub(crate) async fn sethook<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Json(body): Json<SetHookBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::HooksManage, &body.def.collection)?;
    let name = body.name;
    let endpoint = body.endpoint;
    let def = body.def;
    run_db(&state.executor, move |db| db.sethook(&name, &endpoint, def)).await?;
    notify_outbox(&state);
    notify_replication(&state);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn hook<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_queries_allowed(&state)?;
    let hook_name = name.clone();
    let hook = run_db(&state.executor, move |db| Ok(db.hook_def(&name))).await?;
    let Some(hook) = hook else {
        return Err(HttpError::NotFound(format!("hook not found: {hook_name}")));
    };
    ensure_collection_action(&principal, AuthAction::HooksManage, &hook.def.collection)?;
    Ok(Json(serde_json::to_value(hook).map_err(internal)?))
}

pub(crate) async fn delhook<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    let existing = run_db(&state.executor, {
        let lookup = name.clone();
        move |db| Ok(db.hook_def(&lookup))
    })
    .await?;
    if let Some(hook) = existing.as_ref() {
        ensure_collection_action(&principal, AuthAction::HooksManage, &hook.def.collection)?;
    }
    let deleted = run_db(&state.executor, move |db| db.delhook(&name)).await?;
    if deleted {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

pub(crate) async fn hooks<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Query(query): Query<MatchQuery>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_queries_allowed(&state)?;
    let pattern = query.match_pattern.unwrap_or_else(|| "*".to_owned());
    let hooks = run_db(&state.executor, move |db| db.hooks(&pattern)).await?;
    let visible = hooks
        .into_iter()
        .filter(|hook| principal.allows(AuthAction::HooksManage, &hook.collection))
        .collect::<Vec<_>>();
    Ok(Json(serde_json::to_value(visible).map_err(internal)?))
}

#[derive(Deserialize)]
pub(crate) struct ConfigValue {
    value: String,
}

pub(crate) async fn config_get<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let readonly = run_db(&state.executor, move |db| {
        Ok(db
            .config()
            .read()
            .map(|guard| guard.read_only.to_string())
            .unwrap_or_else(|poisoned| poisoned.into_inner().read_only.to_string()))
    })
    .await?;
    let value = match name.as_str() {
        "readonly" => readonly,
        "webhook_timeout_ms" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard.webhook_timeout_ms.to_string(),
                Err(poisoned) => poisoned.into_inner().webhook_timeout_ms.to_string(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        "webhook_concurrency_limit" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard.webhook_concurrency_limit.to_string(),
                Err(poisoned) => poisoned.into_inner().webhook_concurrency_limit.to_string(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        "webhook_retry_count" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard.webhook_retry_count.to_string(),
                Err(poisoned) => poisoned.into_inner().webhook_retry_count.to_string(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        "webhook_retry_initial_backoff_ms" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard.webhook_retry_initial_backoff_ms.to_string(),
                Err(poisoned) => poisoned
                    .into_inner()
                    .webhook_retry_initial_backoff_ms
                    .to_string(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        "webhook_retry_max_backoff_ms" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard.webhook_retry_max_backoff_ms.to_string(),
                Err(poisoned) => poisoned
                    .into_inner()
                    .webhook_retry_max_backoff_ms
                    .to_string(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        "webhook_lease_ms" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard.webhook_lease_ms.to_string(),
                Err(poisoned) => poisoned.into_inner().webhook_lease_ms.to_string(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        "webhook_queue_path" => state
            .runtime_config
            .as_ref()
            .map(|runtime| match runtime.read() {
                Ok(guard) => guard
                    .webhook_queue_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                Err(poisoned) => poisoned
                    .into_inner()
                    .webhook_queue_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            })
            .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?,
        _ => return Err(HttpError::NotFound(name)),
    };
    Ok(Json(serde_json::json!({ "value": value })))
}

pub(crate) async fn config_set<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(name): Path<String>,
    Json(body): Json<ConfigValue>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let wake_outbox = matches!(
        name.as_str(),
        "webhook_timeout_ms"
            | "webhook_concurrency_limit"
            | "webhook_retry_count"
            | "webhook_retry_initial_backoff_ms"
            | "webhook_retry_max_backoff_ms"
            | "webhook_lease_ms"
    );
    match name.as_str() {
        "readonly" => {
            let enabled = matches!(body.value.as_str(), "true" | "yes" | "1");
            update_runtime_config(&state, |config| config.read_only = enabled);
            sync_effective_read_only(&state).await?;
        }
        "webhook_timeout_ms" => {
            let timeout_ms = body
                .value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    HttpError::BadRequest(
                        "webhook_timeout_ms must be a positive integer".to_owned(),
                    )
                })?;
            update_runtime_config(&state, |config| config.webhook_timeout_ms = timeout_ms);
        }
        "webhook_concurrency_limit" => {
            let limit = body
                .value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    HttpError::BadRequest(
                        "webhook_concurrency_limit must be a positive integer".to_owned(),
                    )
                })?;
            update_runtime_config(&state, |config| config.webhook_concurrency_limit = limit);
        }
        "webhook_retry_count" => {
            let retry_count = body.value.parse::<u32>().map_err(|_| {
                HttpError::BadRequest("webhook_retry_count must be an integer".to_owned())
            })?;
            update_runtime_config(&state, |config| {
                config.webhook_retry_count = retry_count;
            });
            run_db(&state.executor, move |db| {
                match db.config().write() {
                    Ok(mut guard) => guard.webhook_retry_count = retry_count,
                    Err(poisoned) => poisoned.into_inner().webhook_retry_count = retry_count,
                }
                Ok(())
            })
            .await?;
        }
        "webhook_retry_initial_backoff_ms" => {
            let backoff = body
                .value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    HttpError::BadRequest(
                        "webhook_retry_initial_backoff_ms must be a positive integer".to_owned(),
                    )
                })?;
            update_runtime_config(&state, |config| {
                config.webhook_retry_initial_backoff_ms = backoff;
            });
        }
        "webhook_retry_max_backoff_ms" => {
            let backoff = body
                .value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    HttpError::BadRequest(
                        "webhook_retry_max_backoff_ms must be a positive integer".to_owned(),
                    )
                })?;
            update_runtime_config(&state, |config| {
                config.webhook_retry_max_backoff_ms = backoff;
            });
        }
        "webhook_lease_ms" => {
            let lease_ms = body
                .value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    HttpError::BadRequest("webhook_lease_ms must be a positive integer".to_owned())
                })?;
            update_runtime_config(&state, |config| config.webhook_lease_ms = lease_ms);
        }
        _ => return Err(HttpError::NotFound(name)),
    }
    if wake_outbox {
        notify_outbox(&state);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn flushdb<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    if let Some(coordinator) = &state.flushdb_coordinator {
        coordinator.flushdb().await.map_err(internal)?;
    } else {
        run_db(&state.executor, move |db| db.flushdb()).await?;
    }
    notify_outbox(&state);
    notify_replication(&state);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn gc<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    run_db(&state.executor, move |db| {
        db.gc();
        Ok(())
    })
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn aofshrink<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let result = if let Some(queue) = &state.webhook_queue {
        let queue_records = queue.snapshot_log_records().map_err(internal)?;
        run_db(&state.executor, move |db| {
            let mut records = db.snapshot_base_log_records();
            records.extend(queue_records);
            db.rewrite_log_snapshot(&records)
        })
        .await?
    } else {
        run_db(&state.executor, move |db| db.aofshrink()).await?
    };
    notify_replication(&state);
    Ok(Json(serde_json::to_value(result).map_err(internal)?))
}

pub(crate) async fn webhook_queue_stats<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let queue = state
        .webhook_queue
        .as_ref()
        .ok_or_else(|| HttpError::BadRequest("webhook queue is not attached".to_owned()))?;
    let stats = queue.stats(now_ms()).map_err(internal)?;
    Ok(Json(serde_json::json!({
        "pending": stats.pending,
        "leased": stats.leased,
        "dead_letter": stats.dead_letter,
        "oldest_pending_age_ms": stats.oldest_pending_age_ms,
    })))
}

pub(crate) async fn config_rewrite<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    let runtime = state
        .runtime_config
        .as_ref()
        .ok_or_else(|| HttpError::BadRequest("runtime config is not attached".to_owned()))?;
    rewrite_runtime_config(runtime)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub(crate) struct ReadOnlyBody {
    enabled: bool,
}

pub(crate) async fn readonly<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Json(body): Json<ReadOnlyBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    update_runtime_config(&state, |config| config.read_only = body.enabled);
    sync_effective_read_only(&state).await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "read_only": body.enabled }),
    ))
}

#[derive(Deserialize)]
pub(crate) struct TimeoutBody {
    seconds: f64,
    command: String,
}

pub(crate) async fn timeout<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Json(body): Json<TimeoutBody>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_admin(&principal)?;
    if body.command.trim().is_empty() {
        return Err(HttpError::BadRequest("missing command".to_owned()));
    }
    let command = body.command.clone();
    let seconds = body.seconds;
    if body.seconds <= 0.0 {
        let command_for_db = command.clone();
        run_db(&state.executor, move |db| {
            match db.config().write() {
                Ok(mut guard) => guard.clear_timeout(&command_for_db),
                Err(poisoned) => poisoned.into_inner().clear_timeout(&command_for_db),
            }
            Ok(())
        })
        .await?;
        update_runtime_config(&state, |config| config.clear_timeout(&body.command));
    } else {
        let command_for_db = command.clone();
        run_db(&state.executor, move |db| {
            db.set_timeout(&command_for_db, seconds);
            Ok(())
        })
        .await?;
        update_runtime_config(&state, |config| {
            config.set_timeout(&body.command, body.seconds);
        });
    }
    let timeout = run_db(&state.executor, move |db| Ok(db.timeout(&command))).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "command": body.command,
        "seconds": timeout
    })))
}

pub(crate) async fn drop_collection<S>(
    State(state): State<HttpState<S>>,
    headers: HeaderMap,
    cached_principal: Option<Extension<AuthPrincipal>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let principal = authenticate_headers(
        &state.auth,
        &headers,
        cached_auth_principal(&cached_principal),
    )
    .await?;
    ensure_collection_action(&principal, AuthAction::CollectionsDelete, &collection)?;
    let dropped = run_db(&state.executor, move |db| db.drop_collection(&collection)).await?;
    if dropped {
        notify_outbox(&state);
        notify_replication(&state);
    }
    Ok(Json(serde_json::json!({ "dropped": dropped })))
}

fn update_runtime_config<S>(state: &HttpState<S>, update: impl FnOnce(&mut RuntimeConfig))
where
    S: StorageBackend,
{
    if let Some(runtime) = &state.runtime_config {
        match runtime.write() {
            Ok(mut guard) => update(&mut guard),
            Err(poisoned) => update(&mut poisoned.into_inner()),
        }
    }
}

fn rewrite_runtime_config(runtime: &SharedRuntimeConfig) -> Result<(), HttpError> {
    let snapshot = match runtime.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let path = snapshot
        .config_path
        .clone()
        .ok_or_else(|| HttpError::BadRequest("no config file path is configured".to_owned()))?;
    save_to_path(&snapshot, &path).map_err(internal)
}

fn notify_outbox<S: StorageBackend>(state: &HttpState<S>) {
    if let Some(notify) = &state.outbox_notify {
        notify.notify_waiters();
    }
}

fn notify_replication<S: StorageBackend>(state: &HttpState<S>) {
    if let Some(notify) = &state.replication_notify {
        notify.notify_waiters();
    }
}

fn replication_snapshot<S: StorageBackend>(state: &HttpState<S>) -> Option<ReplicationStatus> {
    state
        .replication_status
        .as_ref()
        .map(|status| match status.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        })
}

fn apply_replication_to_server_info(
    info: &mut latlng_core::ServerInfo,
    replication: Option<&ReplicationStatus>,
) {
    if let Some(replication) = replication {
        info.server_id = replication.server_id.clone();
        info.following = replication.following();
        info.caught_up = replication.caught_up;
        info.caught_up_once = replication.caught_up_once;
        info.leader = !replication.is_follower();
        if replication.is_follower() {
            info.read_only = true;
        }
    }
}

fn ensure_queries_allowed<S: StorageBackend>(state: &HttpState<S>) -> Result<(), HttpError> {
    if replication_snapshot(state).is_some_and(|status| !status.queries_allowed()) {
        return Err(HttpError::Unavailable("catching up to leader".to_owned()));
    }
    Ok(())
}

pub(crate) async fn sync_effective_read_only<S>(state: &HttpState<S>) -> Result<(), HttpError>
where
    S: StorageBackend + Send + Sync + 'static,
{
    let configured_read_only = state
        .runtime_config
        .as_ref()
        .map(|runtime| match runtime.read() {
            Ok(guard) => guard.read_only,
            Err(poisoned) => poisoned.into_inner().read_only,
        })
        .unwrap_or(false);
    let effective = replication_snapshot(state)
        .map(|status| status.effective_read_only(configured_read_only))
        .unwrap_or(configured_read_only);
    run_db(&state.executor, move |db| {
        match db.config().write() {
            Ok(mut guard) => guard.read_only = effective,
            Err(poisoned) => poisoned.into_inner().read_only = effective,
        }
        Ok(())
    })
    .await
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_output_format(
    format: Option<&str>,
    hash_precision: Option<u8>,
) -> Result<OutputFormat, HttpError> {
    match format.unwrap_or("objects").to_ascii_lowercase().as_str() {
        "objects" => Ok(OutputFormat::Objects),
        "points" => Ok(OutputFormat::Points),
        "bounds" => Ok(OutputFormat::Bounds),
        "hashes" => Ok(OutputFormat::Hashes {
            precision: hash_precision.unwrap_or(7),
        }),
        "ids" => Ok(OutputFormat::Ids),
        "count" => Ok(OutputFormat::Count),
        other => Err(HttpError::BadRequest(format!(
            "unknown output format: {other}"
        ))),
    }
}

fn internal(error: impl ToString) -> HttpError {
    HttpError::Internal(error.to_string())
}
