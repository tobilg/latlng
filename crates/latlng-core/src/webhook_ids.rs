use super::*;

pub(crate) fn opaque_webhook_job_id(sequence: u64) -> String {
    opaque_webhook_id("job", &format!("job:{sequence}"))
}

pub(crate) fn opaque_webhook_event_id(
    command_sequence: u64,
    event_index: usize,
    event: &GeofenceEvent,
) -> String {
    opaque_webhook_id(
        "evt",
        &format!(
            "event:{command_sequence}:{event_index}:{}:{}:{}:{:?}:{:?}",
            event.collection,
            event.id,
            event.hook.as_deref().unwrap_or_default(),
            event.command,
            event.detect
        ),
    )
}

pub(crate) fn opaque_webhook_id(prefix: &str, seed: &str) -> String {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let mut context = md5::Context::new();
    context.consume(b"latlng-webhook-id-v1\0");
    context.consume(prefix.as_bytes());
    context.consume(b"\0");
    context.consume(seed.as_bytes());
    format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(context.finalize().0))
}
