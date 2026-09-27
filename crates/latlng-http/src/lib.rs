#![forbid(unsafe_code)]
#![recursion_limit = "512"]

mod authz;
mod error;
mod handlers;
mod metrics;
mod middleware;
mod openapi;
mod routes;
mod state;

use serde_json::Value;

pub use error::HttpError;
pub(crate) use error::json_error_response;
pub use latlng_core as core;
pub use metrics::{EngineMetrics, RequestMetrics};
pub use routes::{
    STABLE_HTTP_ROUTES, StableHttpRoute, apply_runtime_layers, apply_runtime_layers_with_context,
    router, stable_http_routes,
};
pub use state::HttpState;

pub fn openapi_spec() -> Value {
    openapi::spec()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use latlng_auth::AuthConfig;
    use latlng_config::RuntimeConfig;
    use latlng_core::{LatLng, LatLngNative, geo::GeoType};
    use latlng_native_executor::NativeExecutor;
    use latlng_storage_memory::MemoryBackend;
    use tower::ServiceExt;

    use latlng_replication::{FollowTarget, ReplicationStatus};

    use super::{HttpState, RequestMetrics, apply_runtime_layers, router, stable_http_routes};

    #[tokio::test]
    async fn http_roundtrip_and_metrics_are_exposed() {
        let app = app(AuthConfig::default());

        let set_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet/objects/truck-1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "object": {
                            "Point": {
                                "lat": 52.52,
                                "lon": 13.405,
                                "z": null
                            }
                        }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_response.status(), StatusCode::OK);

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/collections/fleet/objects/truck-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        assert!(get_response.headers().contains_key("x-request-id"));
        let get_json = response_json(get_response).await;
        assert_eq!(get_json["geo"]["Point"]["lat"], 52.52);

        let nearby_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet/search/nearby")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "lat": 52.52,
                        "lon": 13.405,
                        "meters": 500.0,
                        "options": {}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(nearby_response.status(), StatusCode::OK);
        let nearby_json = response_json(nearby_response).await;
        assert_eq!(nearby_json["count"], 1);

        let metrics_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metrics_response.status(), StatusCode::OK);
        assert_eq!(
            metrics_response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let metrics_text = response_text(metrics_response).await;
        for (name, metric_type) in [
            ("latlng_http_requests_total", "counter"),
            ("latlng_http_unauthorized_total", "counter"),
            ("latlng_http_forbidden_total", "counter"),
            ("latlng_http_server_errors_total", "counter"),
            ("latlng_http_global_rate_limited_total", "counter"),
            ("latlng_http_principal_rate_limited_total", "counter"),
            ("latlng_hook_attempts_total", "counter"),
            ("latlng_hook_success_total", "counter"),
            ("latlng_hook_failure_total", "counter"),
            ("latlng_hook_retry_total", "counter"),
            ("latlng_hook_dead_letter_total", "counter"),
            ("latlng_webhook_jobs_pending", "gauge"),
            ("latlng_webhook_jobs_leased", "gauge"),
            ("latlng_webhook_jobs_dead_letter", "gauge"),
            ("latlng_webhook_oldest_pending_age_ms", "gauge"),
        ] {
            assert!(metrics_text.contains(&format!("# HELP {name} ")));
            assert!(metrics_text.contains(&format!("# TYPE {name} {metric_type}")));
        }
        assert!(metric_value(&metrics_text, "latlng_http_requests_total") >= Some(4));
    }

    #[tokio::test]
    async fn cors_runtime_layer_adds_preflight_headers() {
        let config = RuntimeConfig {
            http_cors_enabled: true,
            http_cors_allowed_origins: vec!["https://app.example.test".to_owned()],
            ..RuntimeConfig::default()
        };
        config.validate_for_startup().unwrap();
        let app = apply_runtime_layers(app(AuthConfig::default()), &config).unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/ping")
                    .header(header::ORIGIN, "https://app.example.test")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("https://app.example.test")
        );
    }

    #[test]
    fn replication_metrics_are_prometheus_text() {
        let metrics = RequestMetrics::default();
        let mut status = ReplicationStatus::follower(
            "follower-1",
            FollowTarget {
                host: "127.0.0.1".to_owned(),
                port: 7422,
            },
        );
        status.caught_up = false;
        status.local_last_sequence = 7;
        status.leader_last_sequence = 10;
        status.reconnects_total = 2;
        status.checksum_mismatches_total = 1;
        status.resyncs_total = 1;

        let text = metrics.prometheus_text_with_replication(Some(&status), None);

        assert_eq!(metric_value(&text, "latlng_replication_role"), Some(1));
        assert_eq!(
            metric_value(&text, "latlng_replication_lag_sequences"),
            Some(3)
        );
        assert_eq!(
            metric_value(&text, "latlng_replication_reconnects_total"),
            Some(2)
        );
        assert_eq!(
            metric_value(&text, "latlng_replication_checksum_mismatches_total"),
            Some(1)
        );
        assert_eq!(
            metric_value(&text, "latlng_replication_resyncs_total"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn collections_can_be_created_read_and_dropped_explicitly() {
        let app = app(AuthConfig::default());

        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);
        let create_json = response_json(create_response).await;
        assert_eq!(create_json["created"], true);

        let read_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/collections/fleet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read_response.status(), StatusCode::OK);
        let read_json = response_json(read_response).await;
        assert_eq!(read_json["name"], "fleet");
        assert_eq!(read_json["stats"]["object_count"], 0);

        let drop_response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/collections/fleet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(drop_response.status(), StatusCode::OK);
        let drop_json = response_json(drop_response).await;
        assert_eq!(drop_json["dropped"], true);
    }

    #[tokio::test]
    async fn api_docs_expose_the_full_native_http_surface() {
        let app = app(AuthConfig::default());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api-docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let spec = response_json(response).await;

        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "latlng HTTP API");

        assert!(spec["paths"]["/api-docs"]["get"].is_object());
        assert!(spec["paths"]["/collections"]["get"].is_object());
        assert!(spec["paths"]["/collections/{collection}"]["post"].is_object());
        assert!(spec["paths"]["/collections/{collection}"]["put"].is_object());
        assert!(spec["paths"]["/channels/{name}"]["get"].is_object());
        assert!(spec["paths"]["/collections/{collection}/objects/{id}"]["post"].is_object());
        assert!(spec["paths"]["/collections/{collection}/objects/{id}"]["get"].is_object());
        assert!(spec["paths"]["/collections/{collection}/objects/{id}"]["put"].is_null());
        assert!(spec["paths"]["/hooks/{name}"]["get"].is_object());
        assert!(spec["paths"]["/hooks"]["post"].is_object());
        assert!(spec["paths"]["/hooks"]["put"].is_null());
        assert!(spec["paths"]["/admin/follow"].is_null());
        assert!(spec["paths"]["/test"].is_null());
        assert!(spec["components"]["schemas"]["SetObjectRequest"].is_object());
        assert!(spec["components"]["schemas"]["SetHookRequest"].is_object());
        assert!(spec["components"]["schemas"]["SetChannelRequest"].is_object());
        assert!(spec["components"]["schemas"]["WebhookQueueStatsResponse"].is_object());
        assert!(
            spec["paths"]["/collections/{collection}/objects/{id}"]["post"]["requestBody"]
                ["content"]["application/json"]["schema"]["$ref"]
                .as_str()
                .is_some_and(|value| value.ends_with("/SetObjectRequest"))
        );
        assert!(
            spec["paths"]["/metrics"]["get"]["responses"]["200"]["content"]
                ["text/plain; version=0.0.4"]
                .is_object()
        );
        assert_eq!(
            spec["components"]["securitySchemes"]["bearerAuth"]["scheme"],
            "bearer"
        );
    }

    #[test]
    fn openapi_scan_and_text_bodies_are_unwrapped_search_options() {
        let spec = crate::openapi_spec();
        let schemas = &spec["components"]["schemas"];
        for path in [
            "/collections/{collection}/search/scan",
            "/collections/{collection}/search/text",
        ] {
            let reference =
                spec["paths"][path]["post"]["requestBody"]["content"]["application/json"]["schema"]
                    ["$ref"]
                    .as_str()
                    .unwrap();
            let name = reference.rsplit('/').next().unwrap();
            let properties = &schemas[name]["properties"];
            assert!(properties["match_pattern"].is_object(), "{path}");
            assert!(properties["where_filters"].is_object(), "{path}");
            assert!(properties["options"].is_null(), "{path}");
        }
        let condition =
            serde_json::to_string(&schemas["SetObjectRequest"]["properties"]["condition"]).unwrap();
        assert!(condition.contains("SetConditionSchema"), "{condition}");
        assert_eq!(
            schemas["SetConditionSchema"]["enum"],
            serde_json::json!(["Always", "Nx", "Xx"])
        );
        assert!(schemas["SetHookRequest"]["properties"]["def"]["$ref"].is_string());
    }

    #[tokio::test]
    async fn scan_filters_apply_and_wrapped_options_are_rejected() {
        let app = app(AuthConfig::default());
        for id in ["truck-1", "truck-2", "van-7"] {
            let (status, _) = send_json(
                &app,
                "POST",
                &format!("/collections/fleet/objects/{id}"),
                Some(serde_json::json!({"object": {"Point": {"lat": 1.0, "lon": 1.0, "z": null}}})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        let (status, body) = send_json(
            &app,
            "POST",
            "/collections/fleet/search/scan",
            Some(serde_json::json!({"match_pattern": "truck-*", "output": "Count"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["count"], 2);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet/search/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "options": {"match_pattern": "truck-*", "output": "Count"}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error(), "{}", response.status());
    }

    #[tokio::test]
    async fn diagnostic_test_route_is_not_publicly_routed() {
        let app = app(AuthConfig::default());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/test")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({ "payload": "ping" })))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stable_route_inventory_matches_openapi_paths_and_methods() {
        let app = app(AuthConfig::default());
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api-docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let spec = response_json(response).await;

        let expected = stable_http_routes()
            .iter()
            .map(|route| (route.method.to_ascii_lowercase(), route.path.to_owned()))
            .collect::<BTreeSet<_>>();
        let actual = spec["paths"]
            .as_object()
            .expect("OpenAPI paths must be an object")
            .iter()
            .flat_map(|(path, item)| {
                item.as_object().into_iter().flat_map(move |methods| {
                    methods
                        .keys()
                        .map(move |method| (method.clone(), path.clone()))
                })
            })
            .filter(|(method, _)| {
                matches!(method.as_str(), "get" | "post" | "put" | "delete" | "patch")
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(actual, expected);
        assert!(
            stable_http_routes()
                .iter()
                .all(|route| route.path != "/test" && route.path != "/admin/follow")
        );
    }

    #[tokio::test]
    async fn hook_and_channel_defs_can_be_read_individually() {
        let app = app(AuthConfig::default());

        let set_channel_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/channels")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "name": "fleet-channel",
                        "def": {
                            "collection": "fleet",
                            "query": {
                                "Nearby": {
                                    "lat": 52.52,
                                    "lon": 13.405,
                                    "meters": 500.0,
                                    "options": {}
                                }
                            },
                            "detect": ["Enter"],
                            "commands": ["Set"]
                        }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_channel_response.status(), StatusCode::OK);

        let set_hook_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/hooks")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "name": "fleet-hook",
                        "endpoint": "https://example.invalid/hook",
                        "def": {
                            "collection": "fleet",
                            "query": {
                                "Nearby": {
                                    "lat": 52.52,
                                    "lon": 13.405,
                                    "meters": 500.0,
                                    "options": {}
                                }
                            },
                            "detect": ["Enter"],
                            "commands": ["Set"]
                        }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_hook_response.status(), StatusCode::OK);

        let channel_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/channels/fleet-channel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(channel_response.status(), StatusCode::OK);
        let channel_json = response_json(channel_response).await;
        assert_eq!(channel_json["name"], "fleet-channel");
        assert_eq!(channel_json["def"]["collection"], "fleet");

        let hook_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/hooks/fleet-hook")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hook_response.status(), StatusCode::OK);
        let hook_json = response_json(hook_response).await;
        assert_eq!(hook_json["name"], "fleet-hook");
        assert_eq!(hook_json["endpoint"], "https://example.invalid/hook");
        assert_eq!(hook_json["def"]["collection"], "fleet");

        let missing_channel = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/channels/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_channel.status(), StatusCode::NOT_FOUND);

        let missing_hook = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/hooks/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_hook.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reference_area_fences_are_rejected_with_400() {
        let app = app(AuthConfig::default());
        let def = serde_json::json!({
            "collection": "fleet",
            "query": {"Within": {"area": {"Reference": {"collection": "zones", "id": "z1"}}, "options": {}}},
            "detect": ["Enter"],
            "commands": ["Set"]
        });
        let (status, body) = send_json(
            &app,
            "POST",
            "/hooks",
            Some(
                serde_json::json!({"name": "ref", "endpoint": "http://127.0.0.1:9/x", "def": def}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("reference areas"));

        let (status, _) = send_json(
            &app,
            "POST",
            "/channels",
            Some(serde_json::json!({"name": "ref", "def": def})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = send_json(
            &app,
            "POST",
            "/collections/fleet/objects/v1",
            Some(serde_json::json!({"object": {"Point": {"lat": 10.0, "lon": 10.0, "z": null}}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn client_errors_map_to_4xx_with_json_bodies() {
        let app = app(AuthConfig::default());
        let point = serde_json::json!({"object": {"Point": {"lat": 1.0, "lon": 1.0, "z": null}}});
        let (status, _) = send_json(
            &app,
            "POST",
            "/collections/fleet/objects/truck-1",
            Some(serde_json::json!({
                "object": {"Point": {"lat": 1.0, "lon": 1.0, "z": null}},
                "fields": [{"name": "driver", "value": {"type": "text", "value": "ann"}}]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let cases = [
            (
                "POST",
                "/collections/fleet/objects/ghost/fields",
                Some(
                    serde_json::json!({"fields": [{"name": "speed", "value": {"type": "number", "value": 1.0}}]}),
                ),
                StatusCode::NOT_FOUND,
            ),
            (
                "POST",
                "/collections/fleet/objects/ghost/expire",
                Some(serde_json::json!({"seconds": 10})),
                StatusCode::NOT_FOUND,
            ),
            (
                "POST",
                "/collections/nope/search/nearby",
                Some(serde_json::json!({"lat": 1.0, "lon": 1.0, "meters": 10.0})),
                StatusCode::NOT_FOUND,
            ),
            (
                "POST",
                "/collections/fleet/search/scan",
                Some(
                    serde_json::json!({"where_filters": [{"field": "driver", "comparison": {"Regex": "(("}}]}),
                ),
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/collections/fleet/search/scan",
                Some(serde_json::json!({"where_expr_filters": [{"expression": "nonsense"}]})),
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/collections/fleet/objects/bad",
                Some(
                    serde_json::json!({"object": {"GeoJson": {"type": "Polygon", "coordinates": "x"}}}),
                ),
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                "/collections/fleet/objects/truck-1/json",
                Some(serde_json::json!({"path": "properties.name", "value": "x"})),
                StatusCode::BAD_REQUEST,
            ),
            (
                "GET",
                "/collections/fleet/objects/truck-1/json/properties.name",
                None,
                StatusCode::BAD_REQUEST,
            ),
            (
                "GET",
                "/collections/fleet/objects/ghost",
                None,
                StatusCode::NOT_FOUND,
            ),
            (
                "POST",
                "/collections/fleet/objects/v1",
                Some(
                    serde_json::json!({"object": {"Point": {"lat": 1.0, "lon": 1.0, "z": null}}, "feilds": []}),
                ),
                StatusCode::BAD_REQUEST,
            ),
            ("GET", "/no/such/route", None, StatusCode::NOT_FOUND),
            ("PATCH", "/ping", None, StatusCode::METHOD_NOT_ALLOWED),
        ];
        for (method, uri, body, expected) in cases {
            let (status, json) = send_json(&app, method, uri, body).await;
            assert_eq!(status, expected, "{method} {uri}: {json}");
            let error = json["error"].as_str().unwrap_or_default();
            assert!(!error.is_empty(), "{method} {uri}: {json}");
            assert!(
                !error.starts_with("internal error"),
                "{method} {uri}: {error}"
            );
        }

        let malformed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet/objects/v1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert!(response_json(malformed).await["error"].is_string());

        let wrong_type = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet/objects/v1")
                    .body(json_body(&point))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(response_json(wrong_type).await["error"].is_string());
    }

    #[tokio::test]
    async fn invalid_coordinates_and_query_parameters_return_400() {
        let app = app(AuthConfig::default());
        let (status, _) = send_json(
            &app,
            "POST",
            "/collections/geo/objects/ok",
            Some(serde_json::json!({"object": {"Point": {"lat": 53.55, "lon": 9.99, "z": null}}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let cases = [
            (
                "/collections/geo/objects/badlat",
                serde_json::json!({"object": {"Point": {"lat": 999.0, "lon": 9.99, "z": null}}}),
            ),
            (
                "/collections/geo/objects/badlon",
                serde_json::json!({"object": {"Point": {"lat": 53.55, "lon": -720.0, "z": null}}}),
            ),
            (
                "/collections/geo/search/nearby",
                serde_json::json!({"lat": 200.0, "lon": 9.99, "meters": 10.0}),
            ),
            (
                "/collections/geo/search/nearby",
                serde_json::json!({"lat": 53.55, "lon": 9.99, "meters": -5.0}),
            ),
            (
                "/collections/geo/search/within",
                serde_json::json!({"area": {"Bounds": {"min_lat": 60.0, "min_lon": 20.0, "max_lat": 50.0, "max_lon": 0.0}}}),
            ),
            (
                "/collections/geo/search/intersects",
                serde_json::json!({"area": {"Circle": {"lat": 53.55, "lon": 9.99, "meters": 0.0}}}),
            ),
        ];
        for (uri, body) in cases {
            let (status, json) = send_json(&app, "POST", uri, Some(body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {body}: {json}");
        }

        let (_, bounds) = send_json(&app, "GET", "/collections/geo/bounds", None).await;
        assert_eq!(bounds["bounds"]["max_lat"], 53.55);
    }

    #[tokio::test]
    async fn bearer_auth_rejects_missing_token() {
        let app = app(AuthConfig {
            bearer_token: Some("secret".to_owned()),
            ..AuthConfig::default()
        });

        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/ping")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn global_rate_limiter_rejects_all_traffic_together() {
        let config = RuntimeConfig {
            http_rate_limit_enabled: true,
            http_rate_limit_requests_per_second: 1,
            http_rate_limit_burst: 1,
            ..RuntimeConfig::default()
        };
        let (app, metrics) = app_with_runtime_layers(AuthConfig::default(), &config);

        let first = get(&app, "/ping", None).await;
        assert_eq!(first.status(), StatusCode::OK);
        let second = get(&app, "/healthz", None).await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            second
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );
        assert_eq!(
            metric_value(
                &metrics.prometheus_text(),
                "latlng_http_global_rate_limited_total"
            ),
            Some(1)
        );
    }

    #[tokio::test]
    async fn principal_rate_limiter_isolates_jwt_subjects() {
        let auth = AuthConfig {
            jwt_secret: Some("jwt-secret".to_owned()),
            ..AuthConfig::default()
        };
        let config = RuntimeConfig {
            http_principal_rate_limit_enabled: true,
            http_principal_rate_limit_requests_per_second: 1,
            http_principal_rate_limit_burst: 1,
            auth: auth.clone(),
            ..RuntimeConfig::default()
        };
        let (app, metrics) = app_with_runtime_layers(auth, &config);
        let alice = issue_jwt(&serde_json::json!({
            "sub": "alice",
            "exp": 4_102_444_800usize
        }));
        let bob = issue_jwt(&serde_json::json!({
            "sub": "bob",
            "exp": 4_102_444_800usize
        }));

        assert_eq!(
            get(&app, "/ping", Some(&alice)).await.status(),
            StatusCode::OK
        );
        let limited = get(&app, "/ping", Some(&alice)).await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            get(&app, "/ping", Some(&bob)).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            metric_value(
                &metrics.prometheus_text(),
                "latlng_http_principal_rate_limited_total"
            ),
            Some(1)
        );
    }

    #[tokio::test]
    async fn principal_rate_limiter_groups_static_bearer_and_anonymous() {
        let auth = AuthConfig {
            bearer_token: Some("secret".to_owned()),
            ..AuthConfig::default()
        };
        let config = RuntimeConfig {
            http_principal_rate_limit_enabled: true,
            http_principal_rate_limit_requests_per_second: 1,
            http_principal_rate_limit_burst: 1,
            auth: auth.clone(),
            ..RuntimeConfig::default()
        };
        let (app, _) = app_with_runtime_layers(auth, &config);

        assert_eq!(
            get(&app, "/ping", Some("secret")).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            get(&app, "/healthz", Some("secret")).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );

        let missing_first = get(&app, "/ping", None).await;
        assert_eq!(missing_first.status(), StatusCode::UNAUTHORIZED);
        let missing_second = get(&app, "/ping", None).await;
        assert_eq!(missing_second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn webhook_timeout_runtime_config_can_be_read_and_updated() {
        let runtime_config = Arc::new(std::sync::RwLock::new(RuntimeConfig::default()));
        let app = app_with_runtime(AuthConfig::default(), Arc::clone(&runtime_config));

        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/config/webhook_timeout_ms")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        let get_json = response_json(get_response).await;
        assert_eq!(get_json["value"], "5000");

        let set_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/config/webhook_timeout_ms")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({ "value": "2500" })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_response.status(), StatusCode::OK);
        assert_eq!(runtime_config.read().unwrap().webhook_timeout_ms, 2_500);

        let get_limit_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/config/webhook_concurrency_limit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_limit_response.status(), StatusCode::OK);
        let get_limit_json = response_json(get_limit_response).await;
        assert_eq!(get_limit_json["value"], "128");

        let set_limit_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/config/webhook_concurrency_limit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({ "value": "8" })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_limit_response.status(), StatusCode::OK);
        assert_eq!(runtime_config.read().unwrap().webhook_concurrency_limit, 8);
    }

    #[tokio::test]
    async fn jwt_permissions_filter_collection_visibility_and_forbid_writes() {
        let app = app(AuthConfig {
            bearer_token: Some("admin".to_owned()),
            jwt_secret: Some("jwt-secret".to_owned()),
            ..AuthConfig::default()
        });

        let set_fleet = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/fleet/objects/truck-1")
                    .header(header::AUTHORIZATION, "Bearer admin")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "object": {
                            "Point": {
                                "lat": 52.52,
                                "lon": 13.405,
                                "z": null
                            }
                        }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_fleet.status(), StatusCode::OK);

        let set_zone = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/collections/zones/objects/zone-1")
                    .header(header::AUTHORIZATION, "Bearer admin")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(json_body(&serde_json::json!({
                        "object": {
                            "Point": {
                                "lat": 52.52,
                                "lon": 13.405,
                                "z": null
                            }
                        }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set_zone.status(), StatusCode::OK);

        let token = issue_jwt(&serde_json::json!({
            "sub": "reader",
            "exp": 4_102_444_800usize,
            "latlng_permissions": [
                {
                    "collections": ["fleet"],
                    "actions": ["collections:list", "objects:read"]
                }
            ]
        }));

        let collections = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/collections")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(collections.status(), StatusCode::OK);
        let collections_json = response_json(collections).await;
        assert_eq!(
            collections_json["collections"],
            serde_json::json!(["fleet"])
        );

        let fleet_object = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/collections/fleet/objects/truck-1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fleet_object.status(), StatusCode::OK);

        let delete_denied = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/collections/fleet/objects/truck-1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_denied.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn metrics_scope_is_separate_from_admin() {
        let app = app(AuthConfig {
            jwt_secret: Some("jwt-secret".to_owned()),
            ..AuthConfig::default()
        });

        let token = issue_jwt(&serde_json::json!({
            "sub": "metrics",
            "exp": 4_102_444_800usize,
            "latlng_permissions": [
                {
                    "collections": ["*"],
                    "actions": ["metrics:read"]
                }
            ]
        }));

        let metrics = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/metrics")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metrics.status(), StatusCode::OK);

        let server = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/server")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(server.status(), StatusCode::FORBIDDEN);
    }

    fn app(auth: AuthConfig) -> axum::Router {
        app_inner(auth, None)
    }

    fn app_with_runtime(
        auth: AuthConfig,
        runtime_config: Arc<std::sync::RwLock<RuntimeConfig>>,
    ) -> axum::Router {
        app_inner(auth, Some(runtime_config))
    }

    fn app_with_runtime_layers(
        auth: AuthConfig,
        config: &RuntimeConfig,
    ) -> (axum::Router, Arc<RequestMetrics>) {
        let metrics = Arc::new(RequestMetrics::default());
        let app = app_inner_with_metrics(auth.clone(), None, Arc::clone(&metrics));
        let app = super::apply_runtime_layers_with_context(
            app,
            config,
            auth.authenticator().unwrap(),
            Arc::clone(&metrics),
        )
        .unwrap();
        (app, metrics)
    }

    fn app_inner(
        auth: AuthConfig,
        runtime_config: Option<Arc<std::sync::RwLock<RuntimeConfig>>>,
    ) -> axum::Router {
        app_inner_with_metrics(auth, runtime_config, Arc::new(RequestMetrics::default()))
    }

    fn app_inner_with_metrics(
        auth: AuthConfig,
        runtime_config: Option<Arc<std::sync::RwLock<RuntimeConfig>>>,
        metrics: Arc<RequestMetrics>,
    ) -> axum::Router {
        let db: LatLngNative<MemoryBackend> = LatLng::builder()
            .storage(MemoryBackend::new())
            .build()
            .unwrap();
        let db = Arc::new(db);
        let executor = NativeExecutor::with_defaults(Arc::clone(&db)).unwrap();
        router(HttpState {
            db,
            executor,
            auth: auth.authenticator().unwrap(),
            metrics,
            runtime_config,
            webhook_queue: None,
            flushdb_coordinator: None,
            outbox_notify: None,
            replication_status: None,
            replication_coordinator: None,
            replication_notify: None,
        })
    }

    async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().method("GET").uri(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn send_json(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let request = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(json_body(&body))
                .unwrap(),
            None => request.body(Body::empty()).unwrap(),
        };
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                panic!(
                    "{method} {uri} returned non-JSON body: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, json)
    }

    fn json_body(value: &serde_json::Value) -> Body {
        Body::from(serde_json::to_vec(value).unwrap())
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn response_text(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn metric_value(metrics: &str, name: &str) -> Option<u64> {
        metrics.lines().find_map(|line| {
            let mut parts = line.split_whitespace();
            if parts.next()? == name {
                parts.next()?.parse::<u64>().ok()
            } else {
                None
            }
        })
    }

    fn issue_jwt(claims: &serde_json::Value) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(b"jwt-secret"),
        )
        .unwrap()
    }

    #[test]
    fn geo_type_serialization_shape_is_stable() {
        let value = serde_json::to_value(GeoType::point(52.52, 13.405)).unwrap();
        assert_eq!(value["Point"]["lat"], 52.52);
    }
}
