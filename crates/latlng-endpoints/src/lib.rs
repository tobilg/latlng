#![forbid(unsafe_code)]

use std::time::Duration;

use latlng_geofence::GeofenceEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EndpointError {
    #[error("unsupported endpoint: {0}")]
    Unsupported(String),
    #[error("http delivery failed: {0}")]
    Http(String),
}

pub type EndpointResult<T> = Result<T, EndpointError>;

pub async fn deliver_event(
    client: &reqwest::Client,
    endpoint: &str,
    event: &GeofenceEvent,
    timeout: Duration,
) -> EndpointResult<()> {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        deliver_http_event(client, endpoint, event, timeout).await
    } else {
        Err(EndpointError::Unsupported(endpoint.to_owned()))
    }
}

pub async fn deliver_http_event(
    client: &reqwest::Client,
    endpoint: &str,
    event: &GeofenceEvent,
    timeout: Duration,
) -> EndpointResult<()> {
    let payload =
        serde_json::to_value(event).map_err(|error| EndpointError::Http(error.to_string()))?;
    let mut request = client.post(endpoint).timeout(timeout).json(&payload);
    if let Some(event_id) = event.event_id.as_deref() {
        request = request.header("X-LatLng-Event-Id", event_id);
    }
    if let Some(job_id) = event.job_id.as_deref() {
        request = request.header("X-LatLng-Job-Id", job_id);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(EndpointError::Http(format!("status {}", response.status()))),
        Err(error) => Err(EndpointError::Http(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use latlng_geo::{FieldMap, GeoType};
    use latlng_geofence::{DetectType, GeofenceEvent, MutationCommand};
    use tokio::net::TcpListener;

    use super::deliver_http_event;

    #[tokio::test]
    async fn webhook_timeout_is_applied() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app =
            Router::new()
                .route(
                    "/",
                    post(
                        |State(attempts): State<Arc<AtomicUsize>>,
                         Json(_): Json<serde_json::Value>| async move {
                            attempts.fetch_add(1, Ordering::Relaxed);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            StatusCode::OK
                        },
                    ),
                )
                .with_state(Arc::clone(&attempts));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let result = deliver_http_event(
            &reqwest::Client::new(),
            &format!("http://{addr}/"),
            &sample_event(),
            Duration::from_millis(10),
        )
        .await;

        server.abort();
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    fn sample_event() -> GeofenceEvent {
        GeofenceEvent {
            command: MutationCommand::Set,
            detect: DetectType::Enter,
            collection: "fleet".to_owned(),
            id: "truck-1".to_owned(),
            object: GeoType::point(52.52, 13.405),
            fields: FieldMap::new(),
            timestamp_ns: 1,
            event_id: Some("evt-1".to_owned()),
            job_id: Some("job-1".to_owned()),
            hook: Some("hook".to_owned()),
            group: Some("group".to_owned()),
            nearby: None,
            generation: 0,
        }
    }
}
