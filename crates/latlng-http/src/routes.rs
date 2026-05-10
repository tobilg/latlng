use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, Method};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{delete, get, post};
use latlng_auth::Authenticator;
use latlng_config::RuntimeConfig;
use latlng_core::storage::StorageBackend;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;

use crate::handlers::*;
use crate::middleware::{
    GlobalRateLimiter, PrincipalRateLimitMiddlewareState, PrincipalRateLimiter,
    RateLimitMiddlewareState, max_body_size_middleware, metrics_middleware,
    principal_rate_limit_middleware, rate_limit_middleware, request_context_middleware,
    request_timeout_middleware,
};
use crate::{HttpState, RequestMetrics};

pub struct StableHttpRoute {
    pub method: &'static str,
    pub path: &'static str,
}

pub const STABLE_HTTP_ROUTES: &[StableHttpRoute] = &[
    StableHttpRoute {
        method: "GET",
        path: "/ping",
    },
    StableHttpRoute {
        method: "GET",
        path: "/healthz",
    },
    StableHttpRoute {
        method: "GET",
        path: "/server",
    },
    StableHttpRoute {
        method: "GET",
        path: "/info",
    },
    StableHttpRoute {
        method: "GET",
        path: "/metrics",
    },
    StableHttpRoute {
        method: "GET",
        path: "/api-docs",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}",
    },
    StableHttpRoute {
        method: "PUT",
        path: "/collections/{collection}",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/collections/{collection}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/rename",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}/bounds",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}/stats",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/objects/{id}",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}/objects/{id}",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/collections/{collection}/objects/{id}",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/collections/{collection}/objects",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/objects/{id}/fields",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}/objects/{id}/fields/{field}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/objects/{id}/expire",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/collections/{collection}/objects/{id}/expire",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}/objects/{id}/ttl",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/objects/{id}/json",
    },
    StableHttpRoute {
        method: "GET",
        path: "/collections/{collection}/objects/{id}/json/{path}",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/collections/{collection}/objects/{id}/json/{path}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/search/nearby",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/search/within",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/search/intersects",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/search/scan",
    },
    StableHttpRoute {
        method: "POST",
        path: "/collections/{collection}/search/text",
    },
    StableHttpRoute {
        method: "POST",
        path: "/channels",
    },
    StableHttpRoute {
        method: "GET",
        path: "/channels",
    },
    StableHttpRoute {
        method: "GET",
        path: "/channels/{name}",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/channels/{name}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/hooks",
    },
    StableHttpRoute {
        method: "GET",
        path: "/hooks",
    },
    StableHttpRoute {
        method: "GET",
        path: "/hooks/{name}",
    },
    StableHttpRoute {
        method: "DELETE",
        path: "/hooks/{name}",
    },
    StableHttpRoute {
        method: "GET",
        path: "/config/{name}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/config/{name}",
    },
    StableHttpRoute {
        method: "POST",
        path: "/admin/flushdb",
    },
    StableHttpRoute {
        method: "POST",
        path: "/admin/gc",
    },
    StableHttpRoute {
        method: "POST",
        path: "/admin/aofshrink",
    },
    StableHttpRoute {
        method: "GET",
        path: "/admin/webhook-queue",
    },
    StableHttpRoute {
        method: "POST",
        path: "/admin/config/rewrite",
    },
    StableHttpRoute {
        method: "POST",
        path: "/admin/readonly",
    },
    StableHttpRoute {
        method: "POST",
        path: "/admin/timeout",
    },
];

pub fn stable_http_routes() -> &'static [StableHttpRoute] {
    STABLE_HTTP_ROUTES
}

pub fn apply_runtime_layers(router: Router, config: &RuntimeConfig) -> Result<Router, String> {
    let auth = config
        .auth
        .authenticator()
        .map_err(|error| error.to_string())?;
    apply_runtime_layers_inner(router, config, auth, None)
}

pub fn apply_runtime_layers_with_context(
    router: Router,
    config: &RuntimeConfig,
    auth: Authenticator,
    metrics: Arc<RequestMetrics>,
) -> Result<Router, String> {
    apply_runtime_layers_inner(router, config, auth, Some(metrics))
}

fn apply_runtime_layers_inner(
    mut router: Router,
    config: &RuntimeConfig,
    auth: Authenticator,
    metrics: Option<Arc<RequestMetrics>>,
) -> Result<Router, String> {
    if config.http_cors_enabled {
        router = router.layer(cors_layer(config)?);
    }
    if config.http_principal_rate_limit_enabled {
        router = router.layer(from_fn_with_state(
            Arc::new(PrincipalRateLimitMiddlewareState {
                limiter: Arc::new(PrincipalRateLimiter::new(
                    config.http_principal_rate_limit_requests_per_second,
                    config.http_principal_rate_limit_burst,
                )),
                auth,
                metrics: metrics.clone(),
            }),
            principal_rate_limit_middleware,
        ));
    }
    if config.http_rate_limit_enabled {
        router = router.layer(from_fn_with_state(
            Arc::new(RateLimitMiddlewareState {
                limiter: Arc::new(GlobalRateLimiter::new(
                    config.http_rate_limit_requests_per_second,
                    config.http_rate_limit_burst,
                )),
                metrics,
            }),
            rate_limit_middleware,
        ));
    }
    Ok(router
        .layer(from_fn_with_state(
            config.http_request_timeout_ms,
            request_timeout_middleware,
        ))
        .layer(RequestBodyLimitLayer::new(config.http_max_body_bytes))
        .layer(from_fn_with_state(
            config.http_max_body_bytes,
            max_body_size_middleware,
        )))
}

fn cors_layer(config: &RuntimeConfig) -> Result<CorsLayer, String> {
    let mut cors = CorsLayer::new();
    if config
        .http_cors_allowed_origins
        .iter()
        .any(|origin| origin.trim() == "*")
    {
        cors = cors.allow_origin(Any);
    } else {
        let origins = config
            .http_cors_allowed_origins
            .iter()
            .map(|origin| {
                HeaderValue::from_str(origin.trim())
                    .map_err(|error| format!("invalid CORS origin {origin:?}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        cors = cors.allow_origin(origins);
    }
    if config
        .http_cors_allowed_methods
        .iter()
        .any(|method| method.trim() == "*")
    {
        cors = cors.allow_methods(Any);
    } else {
        let methods = config
            .http_cors_allowed_methods
            .iter()
            .map(|method| {
                Method::from_bytes(method.trim().as_bytes())
                    .map_err(|error| format!("invalid CORS method {method:?}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        cors = cors.allow_methods(methods);
    }
    if config
        .http_cors_allowed_headers
        .iter()
        .any(|header| header.trim() == "*")
    {
        cors = cors.allow_headers(Any);
    } else {
        let headers = config
            .http_cors_allowed_headers
            .iter()
            .map(|header| {
                HeaderName::from_bytes(header.trim().as_bytes())
                    .map_err(|error| format!("invalid CORS header {header:?}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        cors = cors.allow_headers(headers);
    }
    if let Some(max_age) = config.http_cors_max_age_seconds {
        cors = cors.max_age(Duration::from_secs(max_age));
    }
    Ok(cors)
}

pub fn router<S>(state: HttpState<S>) -> Router
where
    S: StorageBackend + Send + Sync + 'static,
{
    Router::new()
        .route("/ping", get(ping::<S>))
        .route("/healthz", get(healthz::<S>))
        .route("/server", get(server::<S>))
        .route("/info", get(info::<S>))
        .route("/metrics", get(metrics::<S>))
        .route("/api-docs", get(api_docs::<S>))
        .route("/collections", get(collections::<S>))
        .route(
            "/collections/{collection}",
            get(collection_info::<S>)
                .post(create_collection::<S>)
                .put(create_collection::<S>)
                .delete(drop_collection::<S>),
        )
        .route("/collections/{collection}/rename", post(rename::<S>))
        .route("/collections/{collection}/bounds", get(bounds::<S>))
        .route("/collections/{collection}/stats", get(stats::<S>))
        .route(
            "/collections/{collection}/objects/{id}",
            post(set_object::<S>)
                .get(get_object::<S>)
                .delete(delete_object::<S>),
        )
        .route(
            "/collections/{collection}/objects",
            delete(delete_matching_objects::<S>),
        )
        .route(
            "/collections/{collection}/objects/{id}/fields",
            post(fset::<S>),
        )
        .route(
            "/collections/{collection}/objects/{id}/fields/{field}",
            get(fget::<S>),
        )
        .route(
            "/collections/{collection}/objects/{id}/expire",
            post(expire::<S>).delete(persist::<S>),
        )
        .route("/collections/{collection}/objects/{id}/ttl", get(ttl::<S>))
        .route(
            "/collections/{collection}/objects/{id}/json",
            post(jset::<S>),
        )
        .route(
            "/collections/{collection}/objects/{id}/json/{*path}",
            get(jget::<S>).delete(jdel::<S>),
        )
        .route("/collections/{collection}/search/nearby", post(nearby::<S>))
        .route("/collections/{collection}/search/within", post(within::<S>))
        .route(
            "/collections/{collection}/search/intersects",
            post(intersects::<S>),
        )
        .route("/collections/{collection}/search/scan", post(scan::<S>))
        .route("/collections/{collection}/search/text", post(search::<S>))
        .route("/channels", post(setchan::<S>).get(channels::<S>))
        .route("/channels/{name}", get(channel::<S>).delete(delchan::<S>))
        .route("/hooks", post(sethook::<S>).get(hooks::<S>))
        .route("/hooks/{name}", get(hook::<S>).delete(delhook::<S>))
        .route("/config/{name}", get(config_get::<S>).post(config_set::<S>))
        .route("/admin/flushdb", post(flushdb::<S>))
        .route("/admin/gc", post(gc::<S>))
        .route("/admin/aofshrink", post(aofshrink::<S>))
        .route("/admin/webhook-queue", get(webhook_queue_stats::<S>))
        .route("/admin/config/rewrite", post(config_rewrite::<S>))
        .route("/admin/readonly", post(readonly::<S>))
        .route("/admin/timeout", post(timeout::<S>))
        .layer(from_fn_with_state(state.clone(), metrics_middleware::<S>))
        .layer(from_fn(request_context_middleware))
        .with_state(state)
}
