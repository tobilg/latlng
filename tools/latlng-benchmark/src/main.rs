#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use latlng_core::geo::{Area, BoundingBox, FieldValue, GeoType};
use latlng_core::geofence::{
    DetectType, GeofenceDef, GeofenceEvent, GeofenceQuery, MutationCommand,
};
use latlng_core::index::{SearchOptions, WhereComparison, WhereFilter, WhereInFilter};
use latlng_core::{FieldEntry, LatLng, NearbyQuery, SetCondition, SetRequest};
use latlng_endpoints::deliver_event;
use latlng_platform::NativePlatform;
use latlng_storage_memory::MemoryBackend;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = LatLng::<NativePlatform, MemoryBackend>::builder()
        .storage(MemoryBackend::new())
        .build()?;

    let objects = 10_000_u32;
    let insert_start = Instant::now();
    for index in 0..objects {
        db.set(SetRequest {
            collection: "bench".to_owned(),
            id: format!("point-{index}"),
            object: GeoType::point(52.0 + f64::from(index) * 0.00001, 13.0),
            fields: vec![
                FieldEntry {
                    name: "speed".to_owned(),
                    value: FieldValue::Number(f64::from(index % 120)),
                },
                FieldEntry {
                    name: "tag".to_owned(),
                    value: FieldValue::Text(
                        if index % 2 == 0 { "keep" } else { "drop" }.to_owned(),
                    ),
                },
            ],
            expire_seconds: None,
            condition: SetCondition::Always,
        })?;
    }
    let insert_elapsed = insert_start.elapsed();

    let query_runs = 100_u32;
    let query_start = Instant::now();
    let mut query_count = 0_u64;
    for _ in 0..query_runs {
        let results = db.nearby(
            "bench",
            NearbyQuery {
                lat: 52.1,
                lon: 13.0,
                meters: 5_000.0,
                options: SearchOptions::default(),
            },
        )?;
        query_count = u64::from(results.count);
    }
    let query_elapsed = query_start.elapsed();

    let within_runs = 100_u32;
    let within_start = Instant::now();
    let mut within_count = 0_u64;
    for _ in 0..within_runs {
        let results = db.within(
            "bench",
            Area::Bounds(BoundingBox::new(52.0, 13.0, 52.05, 13.05)),
            SearchOptions::default(),
        )?;
        within_count = u64::from(results.count);
    }
    let within_elapsed = within_start.elapsed();

    let intersects_runs = 100_u32;
    let intersects_start = Instant::now();
    let mut intersects_count = 0_u64;
    for _ in 0..intersects_runs {
        let results = db.intersects(
            "bench",
            Area::Bounds(BoundingBox::new(52.0, 13.0, 52.05, 13.05)),
            SearchOptions {
                clip: true,
                ..SearchOptions::default()
            },
        )?;
        intersects_count = u64::from(results.count);
    }
    let intersects_elapsed = intersects_start.elapsed();

    let scan_runs = 100_u32;
    let scan_start = Instant::now();
    let mut scan_count = 0_u64;
    for _ in 0..scan_runs {
        let results = db.scan(
            "bench",
            SearchOptions {
                output: latlng_core::index::OutputFormat::Ids,
                where_filters: vec![WhereFilter {
                    field: "speed".to_owned(),
                    comparison: WhereComparison::Range {
                        min: 32.0,
                        max: 96.0,
                    },
                }],
                ..SearchOptions::default()
            },
        )?;
        scan_count = u64::from(results.count);
    }
    let scan_elapsed = scan_start.elapsed();

    let text_objects = 10_000_u32;
    for index in 0..text_objects {
        db.set(SetRequest {
            collection: "bench-text".to_owned(),
            id: format!("msg-{index}"),
            object: GeoType::String(format!("note-{index}")),
            fields: vec![FieldEntry {
                name: "tag".to_owned(),
                value: FieldValue::Text(if index % 2 == 0 { "keep" } else { "drop" }.to_owned()),
            }],
            expire_seconds: None,
            condition: SetCondition::Always,
        })?;
    }

    let search_runs = 100_u32;
    let search_start = Instant::now();
    let mut search_count = 0_u64;
    for _ in 0..search_runs {
        let results = db.search(
            "bench-text",
            SearchOptions {
                output: latlng_core::index::OutputFormat::Ids,
                match_pattern: Some("msg-*".to_owned()),
                where_in_filters: vec![WhereInFilter {
                    field: "tag".to_owned(),
                    values: vec!["keep".to_owned()],
                }],
                ..SearchOptions::default()
            },
        )?;
        search_count = u64::from(results.count);
    }
    let search_elapsed = search_start.elapsed();

    db.setchan(
        "bench-events",
        GeofenceDef {
            collection: "bench".to_owned(),
            query: GeofenceQuery::Nearby {
                lat: 52.0,
                lon: 13.0,
                meters: 2_500.0,
                options: SearchOptions::default(),
            },
            detect: vec![DetectType::Enter],
            commands: vec![MutationCommand::Set],
        },
    )?;

    let fence_runs = 1_000_u32;
    let fence_start = Instant::now();
    for index in 0..fence_runs {
        db.set(SetRequest {
            collection: "bench".to_owned(),
            id: format!("geofence-{index}"),
            object: GeoType::point(52.0 + f64::from(index) * 0.000001, 13.0),
            fields: Vec::new(),
            expire_seconds: None,
            condition: SetCondition::Always,
        })?;
    }
    let fence_elapsed = fence_start.elapsed();
    let webhook_avg_us = benchmark_webhooks()?;

    println!(
        "set_count={objects} set_ms={} set_ops_per_sec={} nearby_runs={query_runs} nearby_count={} nearby_avg_us={} within_runs={within_runs} within_count={} within_avg_us={} intersects_runs={intersects_runs} intersects_count={} intersects_avg_us={} scan_runs={scan_runs} scan_count={} scan_avg_us={} search_runs={search_runs} search_count={} search_avg_us={} fence_runs={fence_runs} fence_avg_us={} webhook_avg_us={}",
        insert_elapsed.as_millis(),
        ops_per_sec(objects as u64, insert_elapsed),
        query_count,
        average_micros(query_elapsed, query_runs as u64),
        within_count,
        average_micros(within_elapsed, within_runs as u64),
        intersects_count,
        average_micros(intersects_elapsed, intersects_runs as u64),
        scan_count,
        average_micros(scan_elapsed, scan_runs as u64),
        search_count,
        average_micros(search_elapsed, search_runs as u64),
        average_micros(fence_elapsed, fence_runs as u64),
        webhook_avg_us,
    );
    Ok(())
}

fn ops_per_sec(ops: u64, elapsed: std::time::Duration) -> u64 {
    let seconds = elapsed.as_secs_f64();
    if seconds == 0.0 {
        ops
    } else {
        (ops as f64 / seconds) as u64
    }
}

fn average_micros(elapsed: std::time::Duration, runs: u64) -> u128 {
    if runs == 0 {
        0
    } else {
        elapsed.as_micros() / u128::from(runs)
    }
}

fn benchmark_webhooks() -> Result<u128, Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let seen = Arc::new(AtomicU64::new(0));
        let app = Router::new().route(
            "/hook",
            post({
                let seen = Arc::clone(&seen);
                move |Json(_body): Json<serde_json::Value>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.fetch_add(1, Ordering::Relaxed);
                        (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let event = GeofenceEvent {
            command: MutationCommand::Set,
            detect: DetectType::Enter,
            collection: "bench".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: latlng_core::geo::FieldMap::new(),
            timestamp_ns: 1,
            event_id: Some("bench-event-1".to_owned()),
            job_id: Some("bench-job-1".to_owned()),
            hook: Some("bench-hook".to_owned()),
            group: Some("bench-group".to_owned()),
            nearby: None,
            generation: 0,
        };

        let runs = 100_u64;
        let start = Instant::now();
        for _ in 0..runs {
            deliver_event(
                &client,
                &format!("http://{addr}/hook"),
                &event,
                Duration::from_millis(5_000),
            )
            .await?;
        }
        let elapsed = start.elapsed();
        server.abort();
        let _ = server.await;
        let delivered = seen.load(Ordering::Relaxed);
        if delivered != runs {
            return Err(format!("expected {runs} webhook deliveries, saw {delivered}").into());
        }
        Ok(average_micros(elapsed, runs))
    })
}
