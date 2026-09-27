use std::path::PathBuf;

use clap::Parser;

/// Command-line flags for `latlng-server`.
///
/// Every setting is optional so that "not given" can be told apart from an
/// explicit value: flags override environment variables, which override the
/// config file, which overrides built-in defaults.
#[derive(Debug, Default, Parser)]
#[command(
    name = "latlng-server",
    version,
    about = "Geospatial database server with geofencing, HTTP, WebSocket and Cap'n Proto APIs."
)]
pub(crate) struct Cli {
    /// Path to a JSON or TOML config file.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Print the config reference as JSON and exit.
    #[arg(long)]
    pub print_config_reference: bool,
    /// Print the OpenAPI document as JSON and exit.
    #[arg(long)]
    pub print_openapi: bool,
    /// Validate the resolved config, print a summary and exit.
    #[arg(long)]
    pub check_config: bool,

    /// HTTP listen address.
    #[arg(long, visible_alias = "listen-addr", value_name = "ADDR")]
    pub listen: Option<String>,
    /// Enable the Cap'n Proto listener.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub capnp_enabled: Option<bool>,
    /// Disable the Cap'n Proto listener.
    #[arg(long)]
    pub no_capnp: bool,
    /// Cap'n Proto listen address.
    #[arg(long, value_name = "ADDR")]
    pub capnp_listen: Option<String>,
    /// Stable server identifier used by replication.
    #[arg(long, value_name = "ID")]
    pub server_id: Option<String>,

    /// Use append-only-file storage at this path.
    #[arg(long, value_name = "PATH")]
    pub aof: Option<PathBuf>,
    /// Use in-memory storage.
    #[arg(long)]
    pub memory: bool,
    /// Start in read-only mode.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub read_only: Option<bool>,

    /// Static bearer token required on every request.
    #[arg(long, value_name = "TOKEN")]
    pub bearer_token: Option<String>,
    /// Disable static bearer token authentication.
    #[arg(long)]
    pub disable_bearer_token: bool,
    /// HMAC secret for JWT authentication.
    #[arg(long, value_name = "SECRET")]
    pub jwt_secret: Option<String>,
    /// PEM public key for JWT authentication.
    #[arg(long, value_name = "PEM")]
    pub jwt_public_key_pem: Option<String>,
    /// Required JWT issuer.
    #[arg(long, value_name = "ISSUER")]
    pub jwt_issuer: Option<String>,
    /// Required JWT audience.
    #[arg(long, value_name = "AUDIENCE")]
    pub jwt_audience: Option<String>,
    /// JWT signing algorithm.
    #[arg(long, value_name = "ALG")]
    pub jwt_algorithm: Option<String>,
    /// Allowed JWT clock skew in seconds.
    #[arg(long, value_name = "SECONDS")]
    pub jwt_leeway: Option<u64>,
    /// JWKS URL for JWT authentication.
    #[arg(long, value_name = "URL")]
    pub jwks_url: Option<String>,
    /// JWKS provider preset identifier.
    #[arg(long, value_name = "ID")]
    pub jwks_provider_id: Option<String>,
    /// JWKS refresh interval in seconds.
    #[arg(long, value_name = "SECONDS")]
    pub jwks_refresh_interval_seconds: Option<u64>,
    /// JWKS cache TTL in seconds.
    #[arg(long, value_name = "SECONDS")]
    pub jwks_cache_ttl_seconds: Option<u64>,
    /// JWKS fetch timeout in milliseconds.
    #[arg(long, value_name = "MS")]
    pub jwks_http_timeout_ms: Option<u64>,
    /// Refuse to start without authentication configured.
    #[arg(long)]
    pub require_auth: bool,
    /// Enable production guardrails.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub production_mode: Option<bool>,

    /// Per-subscriber geofence event queue capacity.
    #[arg(long, value_name = "N")]
    pub subscriber_queue_capacity: Option<usize>,
    /// Path of the SQLite webhook queue.
    #[arg(long, value_name = "PATH")]
    pub webhook_queue_path: Option<PathBuf>,
    /// Webhook delivery timeout in milliseconds.
    #[arg(long, value_name = "MS")]
    pub webhook_timeout_ms: Option<u64>,
    /// Maximum concurrent webhook deliveries.
    #[arg(long, value_name = "N")]
    pub webhook_concurrency_limit: Option<usize>,
    /// Webhook delivery retry count.
    #[arg(long, value_name = "N")]
    pub webhook_retry_count: Option<u32>,
    /// Initial webhook retry backoff in milliseconds.
    #[arg(long, value_name = "MS")]
    pub webhook_retry_initial_backoff_ms: Option<u64>,
    /// Maximum webhook retry backoff in milliseconds.
    #[arg(long, value_name = "MS")]
    pub webhook_retry_max_backoff_ms: Option<u64>,
    /// Webhook job lease duration in milliseconds.
    #[arg(long, value_name = "MS")]
    pub webhook_lease_ms: Option<u64>,
    /// How often the leader deletes expired objects, in milliseconds. 0 disables the sweep.
    #[arg(long, value_name = "MS")]
    pub expiry_sweep_interval_ms: Option<u64>,

    /// Native executor worker threads.
    #[arg(long, value_name = "N")]
    pub native_executor_threads: Option<usize>,
    /// Native executor queue limit.
    #[arg(long, value_name = "N")]
    pub native_executor_queue_limit: Option<usize>,
    /// AOF writer queue limit.
    #[arg(long, value_name = "N")]
    pub aof_writer_queue_limit: Option<usize>,
    /// AOF group commit delay in milliseconds.
    #[arg(long, value_name = "MS")]
    pub aof_group_commit_delay_ms: Option<u64>,
    /// Maximum requests per AOF group commit.
    #[arg(long, value_name = "N")]
    pub aof_group_commit_max_requests: Option<usize>,

    /// Leader host to follow.
    #[arg(long, value_name = "HOST")]
    pub follow_host: Option<String>,
    /// Leader port to follow.
    #[arg(long, value_name = "PORT")]
    pub follow_port: Option<u16>,
    /// Credential used to authenticate against the leader.
    #[arg(long, value_name = "TOKEN")]
    pub replication_credential: Option<String>,
    /// Replication batch size.
    #[arg(long, value_name = "N")]
    pub replication_batch_size: Option<usize>,
    /// Replication reconnect backoff in milliseconds.
    #[arg(long, value_name = "MS")]
    pub replication_reconnect_backoff_ms: Option<u64>,

    /// Enable CORS.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub http_cors_enabled: Option<bool>,
    /// Comma-separated CORS allowed origins.
    #[arg(long, value_name = "LIST")]
    pub http_cors_allowed_origins: Option<String>,
    /// Comma-separated CORS allowed methods.
    #[arg(long, value_name = "LIST")]
    pub http_cors_allowed_methods: Option<String>,
    /// Comma-separated CORS allowed headers.
    #[arg(long, value_name = "LIST")]
    pub http_cors_allowed_headers: Option<String>,
    /// CORS preflight max age in seconds.
    #[arg(long, value_name = "SECONDS")]
    pub http_cors_max_age_seconds: Option<u64>,
    /// Maximum HTTP request body size in bytes.
    #[arg(long, value_name = "BYTES")]
    pub http_max_body_bytes: Option<usize>,
    /// HTTP request timeout in milliseconds.
    #[arg(long, value_name = "MS")]
    pub http_request_timeout_ms: Option<u64>,
    /// Enable the global HTTP rate limiter.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub http_rate_limit_enabled: Option<bool>,
    /// Global HTTP rate limit in requests per second.
    #[arg(long, value_name = "N")]
    pub http_rate_limit_requests_per_second: Option<u64>,
    /// Global HTTP rate limit burst.
    #[arg(long, value_name = "N")]
    pub http_rate_limit_burst: Option<u64>,
    /// Enable the per-principal HTTP rate limiter.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub http_principal_rate_limit_enabled: Option<bool>,
    /// Per-principal HTTP rate limit in requests per second.
    #[arg(long, value_name = "N")]
    pub http_principal_rate_limit_requests_per_second: Option<u64>,
    /// Per-principal HTTP rate limit burst.
    #[arg(long, value_name = "N")]
    pub http_principal_rate_limit_burst: Option<u64>,

    /// Enable logging.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true", value_parser = parse_flag_bool)]
    pub logging_enabled: Option<bool>,
    /// Disable logging.
    #[arg(long)]
    pub no_logging: bool,
    /// Log format (text or json).
    #[arg(long, value_name = "FORMAT")]
    pub log_format: Option<String>,
    /// Log level filter.
    #[arg(long, value_name = "LEVEL")]
    pub log_level: Option<String>,
    /// Log destination (stdout, stderr or file).
    #[arg(long, value_name = "DEST")]
    pub log_destination: Option<String>,
    /// Log file path when logging to a file.
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<PathBuf>,
}

fn parse_flag_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!("expected true or false, got `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use clap::error::ErrorKind;

    use super::Cli;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("latlng-server").chain(args.iter().copied()))
    }

    #[test]
    fn unknown_flags_are_rejected() {
        let error = parse(&["--reqiure-auth"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
        assert!(error.to_string().contains("--reqiure-auth"));
    }

    #[test]
    fn optional_value_booleans_accept_all_forms() {
        for args in [
            &["--capnp-enabled"][..],
            &["--capnp-enabled", "true"],
            &["--capnp-enabled=true"],
        ] {
            assert_eq!(parse(args).unwrap().capnp_enabled, Some(true), "{args:?}");
        }
        assert_eq!(
            parse(&["--capnp-enabled=false"]).unwrap().capnp_enabled,
            Some(false)
        );
        assert_eq!(
            parse(&["--capnp-enabled", "--memory"])
                .unwrap()
                .capnp_enabled,
            Some(true)
        );
        assert!(parse(&["--capnp-enabled=maybe"]).is_err());
    }

    #[test]
    fn listen_accepts_both_spellings_and_equals_form() {
        assert_eq!(
            parse(&["--listen", "1.2.3.4:5"]).unwrap().listen.as_deref(),
            Some("1.2.3.4:5")
        );
        assert_eq!(
            parse(&["--listen-addr=1.2.3.4:5"])
                .unwrap()
                .listen
                .as_deref(),
            Some("1.2.3.4:5")
        );
    }

    #[test]
    fn invalid_numbers_are_rejected() {
        assert!(parse(&["--webhook-timeout-ms", "soon"]).is_err());
    }

    #[test]
    fn help_and_version_are_display_errors() {
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["--version"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
        assert_eq!(
            parse(&["-V"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }
}
