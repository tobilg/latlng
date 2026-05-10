#![forbid(unsafe_code)]

use latlng_config::{LogDestination, LogFormat, RuntimeConfig};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

pub type LogGuard = Option<tracing_appender::non_blocking::WorkerGuard>;

pub fn log_auth_mode(config: &RuntimeConfig) {
    let bearer = if config.auth.bearer_enabled() {
        "enabled"
    } else if config.auth.disable_bearer_token {
        "disabled"
    } else {
        "absent"
    };
    let jwt = if config.auth.jwt_secret.is_some() {
        "hmac"
    } else if config.auth.jwt_public_key_pem.is_some() {
        "pem-public-key"
    } else if config.auth.jwks_url.is_some() {
        "jwks"
    } else {
        "none"
    };
    let jwks_provider = config.auth.jwks_provider_id.as_deref().unwrap_or("none");
    if config.auth.auth_enabled() {
        info!(
            require_auth = config.require_auth,
            bearer, jwt, jwks_provider, "authentication configured"
        );
    } else {
        warn!(
            require_auth = config.require_auth,
            bearer,
            jwt,
            jwks_provider,
            "authentication is disabled; set require_auth=true or LATLNG_REQUIRE_AUTH=1 for production guardrails"
        );
    }
}

pub fn log_production_guardrails(config: &RuntimeConfig) {
    for warning in config.production_guardrail_warnings() {
        warn!(
            warning,
            production_mode = config.production_mode,
            "production guardrail warning"
        );
    }
}

pub fn init_logging(config: &RuntimeConfig) -> Result<LogGuard, Box<dyn std::error::Error>> {
    if !config.logging_enabled || matches!(config.log_destination, LogDestination::None) {
        return Ok(None);
    }
    let filter =
        EnvFilter::try_new(config.log_level.clone()).or_else(|_| EnvFilter::try_new("info"))?;
    match config.log_destination {
        LogDestination::Stderr => {
            match config.log_format {
                LogFormat::Compact => tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::io::stderr)
                    .try_init()
                    .map_err(|error| error.to_string())?,
                LogFormat::Json => tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(filter)
                    .with_writer(std::io::stderr)
                    .try_init()
                    .map_err(|error| error.to_string())?,
            }
            Ok(None)
        }
        LogDestination::Stdout => {
            match config.log_format {
                LogFormat::Compact => tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::io::stdout)
                    .try_init()
                    .map_err(|error| error.to_string())?,
                LogFormat::Json => tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(filter)
                    .with_writer(std::io::stdout)
                    .try_init()
                    .map_err(|error| error.to_string())?,
            }
            Ok(None)
        }
        LogDestination::File => {
            let path = config
                .log_file_path
                .as_ref()
                .ok_or("log_file_path is required when log_destination is file")?;
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            let (writer, guard) = tracing_appender::non_blocking(file);
            match config.log_format {
                LogFormat::Compact => tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(writer)
                    .try_init()
                    .map_err(|error| error.to_string())?,
                LogFormat::Json => tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(filter)
                    .with_writer(writer)
                    .try_init()
                    .map_err(|error| error.to_string())?,
            }
            Ok(Some(guard))
        }
        LogDestination::None => Ok(None),
    }
}
