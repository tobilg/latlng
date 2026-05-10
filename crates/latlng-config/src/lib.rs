#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use latlng_auth::AuthConfig;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StorageMode {
    #[default]
    Memory,
    Aof {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Compact,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogDestination {
    #[default]
    Stderr,
    Stdout,
    File,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub production_mode: bool,
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default = "default_capnp_enabled")]
    pub capnp_enabled: bool,
    #[serde(default = "default_capnp_listen_addr")]
    pub capnp_listen_addr: String,
    #[serde(default = "default_server_id")]
    pub server_id: String,
    #[serde(default)]
    pub storage: StorageMode,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub command_timeouts: BTreeMap<String, f64>,
    #[serde(default = "default_subscriber_queue_capacity")]
    pub subscriber_queue_capacity: usize,
    #[serde(default)]
    pub webhook_queue_path: Option<PathBuf>,
    #[serde(default = "default_webhook_timeout_ms")]
    pub webhook_timeout_ms: u64,
    #[serde(default = "default_webhook_concurrency_limit")]
    pub webhook_concurrency_limit: usize,
    #[serde(default = "default_webhook_retry_count")]
    pub webhook_retry_count: u32,
    #[serde(default = "default_webhook_retry_initial_backoff_ms")]
    pub webhook_retry_initial_backoff_ms: u64,
    #[serde(default = "default_webhook_retry_max_backoff_ms")]
    pub webhook_retry_max_backoff_ms: u64,
    #[serde(default = "default_webhook_lease_ms")]
    pub webhook_lease_ms: u64,
    #[serde(default = "default_native_executor_threads")]
    pub native_executor_threads: usize,
    #[serde(default = "default_native_executor_queue_limit")]
    pub native_executor_queue_limit: usize,
    #[serde(default = "default_aof_writer_queue_limit")]
    pub aof_writer_queue_limit: usize,
    #[serde(default = "default_aof_group_commit_delay_ms")]
    pub aof_group_commit_delay_ms: u64,
    #[serde(default = "default_aof_group_commit_max_requests")]
    pub aof_group_commit_max_requests: usize,
    #[serde(default)]
    pub follow_host: Option<String>,
    #[serde(default)]
    pub follow_port: Option<u16>,
    #[serde(default)]
    pub replication_credential: Option<String>,
    #[serde(default = "default_replication_batch_size")]
    pub replication_batch_size: usize,
    #[serde(default = "default_replication_reconnect_backoff_ms")]
    pub replication_reconnect_backoff_ms: u64,
    #[serde(default)]
    pub http_cors_enabled: bool,
    #[serde(default)]
    pub http_cors_allowed_origins: Vec<String>,
    #[serde(default = "default_http_cors_allowed_methods")]
    pub http_cors_allowed_methods: Vec<String>,
    #[serde(default = "default_http_cors_allowed_headers")]
    pub http_cors_allowed_headers: Vec<String>,
    #[serde(default)]
    pub http_cors_max_age_seconds: Option<u64>,
    #[serde(default = "default_http_max_body_bytes")]
    pub http_max_body_bytes: usize,
    #[serde(default = "default_http_request_timeout_ms")]
    pub http_request_timeout_ms: u64,
    #[serde(default)]
    pub http_rate_limit_enabled: bool,
    #[serde(default = "default_http_rate_limit_requests_per_second")]
    pub http_rate_limit_requests_per_second: u64,
    #[serde(default = "default_http_rate_limit_burst")]
    pub http_rate_limit_burst: u64,
    #[serde(default)]
    pub http_principal_rate_limit_enabled: bool,
    #[serde(default = "default_http_principal_rate_limit_requests_per_second")]
    pub http_principal_rate_limit_requests_per_second: u64,
    #[serde(default = "default_http_principal_rate_limit_burst")]
    pub http_principal_rate_limit_burst: u64,
    #[serde(default = "default_logging_enabled")]
    pub logging_enabled: bool,
    #[serde(default)]
    pub log_format: LogFormat,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub log_destination: LogDestination,
    #[serde(default)]
    pub log_file_path: Option<PathBuf>,
    #[serde(default)]
    pub require_auth: bool,
    #[serde(flatten)]
    pub auth: AuthConfig,
    #[serde(skip)]
    pub config_path: Option<PathBuf>,
}

pub type SharedRuntimeConfig = Arc<RwLock<RuntimeConfig>>;
pub type FlushDbFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
pub type SharedFlushDbCoordinator = Arc<dyn FlushDbCoordinator>;

pub trait FlushDbCoordinator: Send + Sync {
    fn flushdb(&self) -> FlushDbFuture<'_>;
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigReferenceEntry {
    pub name: &'static str,
    pub kind: &'static str,
    pub default: serde_json::Value,
    pub description: &'static str,
}

pub fn config_reference() -> Vec<ConfigReferenceEntry> {
    vec![
        config_entry(
            "production_mode",
            "bool",
            false,
            "Enables strict production startup guardrails.",
        ),
        config_entry(
            "listen_addr",
            "string",
            default_listen_addr(),
            "HTTP listen address.",
        ),
        config_entry(
            "capnp_enabled",
            "bool",
            default_capnp_enabled(),
            "Enables the Cap'n Proto RPC and replication listener.",
        ),
        config_entry(
            "capnp_listen_addr",
            "string",
            default_capnp_listen_addr(),
            "Cap'n Proto listen address.",
        ),
        config_entry(
            "server_id",
            "string",
            "<generated uuid>",
            "Stable server identity used in replication status.",
        ),
        config_entry(
            "storage",
            "storage_mode",
            "memory",
            "Storage backend. Use memory or aof with a path.",
        ),
        config_entry(
            "read_only",
            "bool",
            false,
            "Rejects mutating commands when true.",
        ),
        config_entry(
            "command_timeouts",
            "map<string,float>",
            serde_json::json!({}),
            "Per-command timeout overrides in seconds.",
        ),
        config_entry(
            "subscriber_queue_capacity",
            "usize",
            default_subscriber_queue_capacity(),
            "Per-subscriber event queue capacity.",
        ),
        config_entry(
            "webhook_queue_path",
            "path|null",
            serde_json::Value::Null,
            "SQLite webhook queue path. Defaults near the AOF or current directory.",
        ),
        config_entry(
            "webhook_timeout_ms",
            "u64",
            default_webhook_timeout_ms(),
            "HTTP timeout for webhook deliveries.",
        ),
        config_entry(
            "webhook_concurrency_limit",
            "usize",
            default_webhook_concurrency_limit(),
            "Maximum concurrent webhook delivery attempts.",
        ),
        config_entry(
            "webhook_retry_count",
            "u32",
            default_webhook_retry_count(),
            "Maximum webhook retry attempts before dead-lettering.",
        ),
        config_entry(
            "webhook_retry_initial_backoff_ms",
            "u64",
            default_webhook_retry_initial_backoff_ms(),
            "Initial webhook retry backoff.",
        ),
        config_entry(
            "webhook_retry_max_backoff_ms",
            "u64",
            default_webhook_retry_max_backoff_ms(),
            "Maximum webhook retry backoff.",
        ),
        config_entry(
            "webhook_lease_ms",
            "u64",
            default_webhook_lease_ms(),
            "Webhook job lease duration.",
        ),
        config_entry(
            "native_executor_threads",
            "usize",
            default_native_executor_threads(),
            "Native worker thread count for core operations.",
        ),
        config_entry(
            "native_executor_queue_limit",
            "usize",
            default_native_executor_queue_limit(),
            "Native executor queue limit.",
        ),
        config_entry(
            "aof_writer_queue_limit",
            "usize",
            default_aof_writer_queue_limit(),
            "AOF writer queue limit.",
        ),
        config_entry(
            "aof_group_commit_delay_ms",
            "u64",
            default_aof_group_commit_delay_ms(),
            "Maximum AOF group commit delay.",
        ),
        config_entry(
            "aof_group_commit_max_requests",
            "usize",
            default_aof_group_commit_max_requests(),
            "Maximum requests per AOF commit cycle.",
        ),
        config_entry(
            "follow_host",
            "string|null",
            serde_json::Value::Null,
            "Leader host for follower replication.",
        ),
        config_entry(
            "follow_port",
            "u16|null",
            serde_json::Value::Null,
            "Leader Cap'n Proto port for follower replication.",
        ),
        config_entry(
            "replication_credential",
            "string|null",
            serde_json::Value::Null,
            "Dedicated credential for replication streams.",
        ),
        config_entry(
            "replication_batch_size",
            "usize",
            default_replication_batch_size(),
            "Maximum entries per replication stream response.",
        ),
        config_entry(
            "replication_reconnect_backoff_ms",
            "u64",
            default_replication_reconnect_backoff_ms(),
            "Follower reconnect backoff after failures.",
        ),
        config_entry(
            "http_cors_enabled",
            "bool",
            false,
            "Enables HTTP CORS middleware.",
        ),
        config_entry(
            "http_cors_allowed_origins",
            "list<string>",
            serde_json::json!([]),
            "Allowed CORS origins. Avoid '*' with auth.",
        ),
        config_entry(
            "http_cors_allowed_methods",
            "list<string>",
            default_http_cors_allowed_methods(),
            "Allowed CORS methods.",
        ),
        config_entry(
            "http_cors_allowed_headers",
            "list<string>",
            default_http_cors_allowed_headers(),
            "Allowed CORS headers.",
        ),
        config_entry(
            "http_cors_max_age_seconds",
            "u64|null",
            serde_json::Value::Null,
            "Optional CORS preflight cache max-age.",
        ),
        config_entry(
            "http_max_body_bytes",
            "usize",
            default_http_max_body_bytes(),
            "Maximum accepted HTTP request body size.",
        ),
        config_entry(
            "http_request_timeout_ms",
            "u64",
            default_http_request_timeout_ms(),
            "Maximum HTTP request duration.",
        ),
        config_entry(
            "http_rate_limit_enabled",
            "bool",
            false,
            "Enables a simple global HTTP token-bucket rate limit.",
        ),
        config_entry(
            "http_rate_limit_requests_per_second",
            "u64",
            default_http_rate_limit_requests_per_second(),
            "Global HTTP rate-limit refill rate.",
        ),
        config_entry(
            "http_rate_limit_burst",
            "u64",
            default_http_rate_limit_burst(),
            "Global HTTP rate-limit burst capacity.",
        ),
        config_entry(
            "http_principal_rate_limit_enabled",
            "bool",
            false,
            "Enables per-principal HTTP token-bucket rate limiting.",
        ),
        config_entry(
            "http_principal_rate_limit_requests_per_second",
            "u64",
            default_http_principal_rate_limit_requests_per_second(),
            "Per-principal HTTP rate-limit refill rate.",
        ),
        config_entry(
            "http_principal_rate_limit_burst",
            "u64",
            default_http_principal_rate_limit_burst(),
            "Per-principal HTTP rate-limit burst capacity.",
        ),
        config_entry(
            "logging_enabled",
            "bool",
            default_logging_enabled(),
            "Enables structured server logging.",
        ),
        config_entry(
            "log_format",
            "compact|json",
            "compact",
            "Log output format.",
        ),
        config_entry(
            "log_level",
            "string",
            default_log_level(),
            "Tracing filter level.",
        ),
        config_entry(
            "log_destination",
            "stderr|stdout|file|none",
            "stderr",
            "Log destination.",
        ),
        config_entry(
            "log_file_path",
            "path|null",
            serde_json::Value::Null,
            "Required when log_destination is file.",
        ),
        config_entry(
            "require_auth",
            "bool",
            false,
            "Rejects unauthenticated requests when true.",
        ),
        config_entry(
            "bearer_token",
            "string|null",
            serde_json::Value::Null,
            "Static full-admin bearer token.",
        ),
        config_entry(
            "disable_bearer_token",
            "bool",
            false,
            "Disables static bearer-token authentication even when configured.",
        ),
        config_entry(
            "jwt_secret",
            "string|null",
            serde_json::Value::Null,
            "HMAC JWT verification secret.",
        ),
        config_entry(
            "jwt_public_key_pem",
            "string|null",
            serde_json::Value::Null,
            "PEM public key for asymmetric JWT validation.",
        ),
        config_entry(
            "jwt_issuer",
            "string|null",
            serde_json::Value::Null,
            "Expected JWT issuer.",
        ),
        config_entry(
            "jwt_audience",
            "string|null",
            serde_json::Value::Null,
            "Expected JWT audience.",
        ),
        config_entry(
            "jwt_algorithm",
            "string|null",
            serde_json::Value::Null,
            "JWT algorithm override.",
        ),
        config_entry(
            "jwks_url",
            "string|null",
            serde_json::Value::Null,
            "JWKS endpoint URL.",
        ),
        config_entry(
            "jwks_provider_id",
            "string|null",
            serde_json::Value::Null,
            "Provider ID for logs/docs.",
        ),
        config_entry(
            "jwks_refresh_interval_seconds",
            "u64",
            latlng_auth::default_jwks_refresh_interval_seconds(),
            "JWKS background refresh interval.",
        ),
        config_entry(
            "jwks_cache_ttl_seconds",
            "u64",
            latlng_auth::default_jwks_cache_ttl_seconds(),
            "JWKS cache TTL.",
        ),
        config_entry(
            "jwks_http_timeout_ms",
            "u64",
            latlng_auth::default_jwks_http_timeout_ms(),
            "JWKS HTTP request timeout.",
        ),
        config_entry("jwt_leeway_seconds", "u64", 0, "JWT clock-skew leeway."),
    ]
}

pub fn config_reference_json() -> serde_json::Value {
    serde_json::to_value(config_reference()).unwrap_or_else(|_| serde_json::json!([]))
}

fn config_entry(
    name: &'static str,
    kind: &'static str,
    default: impl Serialize,
    description: &'static str,
) -> ConfigReferenceEntry {
    ConfigReferenceEntry {
        name,
        kind,
        default: serde_json::to_value(default).unwrap_or(serde_json::Value::Null),
        description,
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            production_mode: false,
            listen_addr: default_listen_addr(),
            capnp_enabled: default_capnp_enabled(),
            capnp_listen_addr: default_capnp_listen_addr(),
            server_id: default_server_id(),
            storage: StorageMode::Memory,
            read_only: false,
            command_timeouts: BTreeMap::new(),
            subscriber_queue_capacity: default_subscriber_queue_capacity(),
            webhook_queue_path: None,
            webhook_timeout_ms: default_webhook_timeout_ms(),
            webhook_concurrency_limit: default_webhook_concurrency_limit(),
            webhook_retry_count: default_webhook_retry_count(),
            webhook_retry_initial_backoff_ms: default_webhook_retry_initial_backoff_ms(),
            webhook_retry_max_backoff_ms: default_webhook_retry_max_backoff_ms(),
            webhook_lease_ms: default_webhook_lease_ms(),
            native_executor_threads: default_native_executor_threads(),
            native_executor_queue_limit: default_native_executor_queue_limit(),
            aof_writer_queue_limit: default_aof_writer_queue_limit(),
            aof_group_commit_delay_ms: default_aof_group_commit_delay_ms(),
            aof_group_commit_max_requests: default_aof_group_commit_max_requests(),
            follow_host: None,
            follow_port: None,
            replication_credential: None,
            replication_batch_size: default_replication_batch_size(),
            replication_reconnect_backoff_ms: default_replication_reconnect_backoff_ms(),
            http_cors_enabled: false,
            http_cors_allowed_origins: Vec::new(),
            http_cors_allowed_methods: default_http_cors_allowed_methods(),
            http_cors_allowed_headers: default_http_cors_allowed_headers(),
            http_cors_max_age_seconds: None,
            http_max_body_bytes: default_http_max_body_bytes(),
            http_request_timeout_ms: default_http_request_timeout_ms(),
            http_rate_limit_enabled: false,
            http_rate_limit_requests_per_second: default_http_rate_limit_requests_per_second(),
            http_rate_limit_burst: default_http_rate_limit_burst(),
            http_principal_rate_limit_enabled: false,
            http_principal_rate_limit_requests_per_second:
                default_http_principal_rate_limit_requests_per_second(),
            http_principal_rate_limit_burst: default_http_principal_rate_limit_burst(),
            logging_enabled: default_logging_enabled(),
            log_format: LogFormat::default(),
            log_level: default_log_level(),
            log_destination: LogDestination::default(),
            log_file_path: None,
            require_auth: false,
            auth: AuthConfig::default(),
            config_path: None,
        }
    }
}

impl RuntimeConfig {
    pub fn timeout_for(&self, command: &str) -> Option<f64> {
        self.command_timeouts
            .get(&normalize_command_key(command))
            .copied()
    }

    pub fn set_timeout(&mut self, command: &str, seconds: f64) {
        self.command_timeouts
            .insert(normalize_command_key(command), seconds.max(0.0));
    }

    pub fn clear_timeout(&mut self, command: &str) {
        self.command_timeouts
            .remove(&normalize_command_key(command));
    }

    pub fn assign_path(&mut self, path: Option<PathBuf>) {
        self.config_path = path;
    }

    pub fn validate_for_startup(&self) -> Result<(), ConfigError> {
        self.auth
            .validate()
            .map_err(|error| ConfigError::Validation(error.to_string()))?;
        if self.require_auth && !self.auth.auth_enabled() {
            return Err(ConfigError::Validation(
                "require_auth is enabled, but no bearer token or JWT verifier is configured"
                    .to_owned(),
            ));
        }
        if self.listen_addr.trim().is_empty() {
            return Err(ConfigError::Validation(
                "listen_addr must not be empty".to_owned(),
            ));
        }
        if self.capnp_enabled && self.capnp_listen_addr.trim().is_empty() {
            return Err(ConfigError::Validation(
                "capnp_listen_addr must not be empty".to_owned(),
            ));
        }
        if self.server_id.trim().is_empty() {
            return Err(ConfigError::Validation(
                "server_id must not be empty".to_owned(),
            ));
        }
        validate_positive_usize("subscriber_queue_capacity", self.subscriber_queue_capacity)?;
        validate_positive_u64("webhook_timeout_ms", self.webhook_timeout_ms)?;
        validate_positive_usize("webhook_concurrency_limit", self.webhook_concurrency_limit)?;
        validate_positive_u64(
            "webhook_retry_initial_backoff_ms",
            self.webhook_retry_initial_backoff_ms,
        )?;
        validate_positive_u64(
            "webhook_retry_max_backoff_ms",
            self.webhook_retry_max_backoff_ms,
        )?;
        validate_positive_u64("webhook_lease_ms", self.webhook_lease_ms)?;
        validate_positive_usize("native_executor_threads", self.native_executor_threads)?;
        validate_positive_usize(
            "native_executor_queue_limit",
            self.native_executor_queue_limit,
        )?;
        validate_positive_usize("aof_writer_queue_limit", self.aof_writer_queue_limit)?;
        validate_positive_usize(
            "aof_group_commit_max_requests",
            self.aof_group_commit_max_requests,
        )?;
        validate_positive_usize("replication_batch_size", self.replication_batch_size)?;
        validate_positive_u64(
            "replication_reconnect_backoff_ms",
            self.replication_reconnect_backoff_ms,
        )?;
        validate_positive_usize("http_max_body_bytes", self.http_max_body_bytes)?;
        validate_positive_u64("http_request_timeout_ms", self.http_request_timeout_ms)?;
        validate_positive_u64(
            "http_rate_limit_requests_per_second",
            self.http_rate_limit_requests_per_second,
        )?;
        validate_positive_u64("http_rate_limit_burst", self.http_rate_limit_burst)?;
        validate_positive_u64(
            "http_principal_rate_limit_requests_per_second",
            self.http_principal_rate_limit_requests_per_second,
        )?;
        validate_positive_u64(
            "http_principal_rate_limit_burst",
            self.http_principal_rate_limit_burst,
        )?;
        if self.webhook_retry_max_backoff_ms < self.webhook_retry_initial_backoff_ms {
            return Err(ConfigError::Validation(
                "webhook_retry_max_backoff_ms must be greater than or equal to webhook_retry_initial_backoff_ms"
                    .to_owned(),
            ));
        }
        match (&self.follow_host, self.follow_port) {
            (Some(host), Some(port)) if !host.trim().is_empty() && port > 0 => {
                if self
                    .replication_credential
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(ConfigError::Validation(
                        "replication_credential is required when follow_host/follow_port are configured"
                            .to_owned(),
                    ));
                }
            }
            (None, None) => {}
            _ => {
                return Err(ConfigError::Validation(
                    "follow_host and follow_port must be configured together".to_owned(),
                ));
            }
        }
        if self.http_cors_enabled {
            if self.http_cors_allowed_origins.is_empty() {
                return Err(ConfigError::Validation(
                    "http_cors_allowed_origins must not be empty when CORS is enabled".to_owned(),
                ));
            }
            validate_non_empty_list("http_cors_allowed_methods", &self.http_cors_allowed_methods)?;
            validate_non_empty_list("http_cors_allowed_headers", &self.http_cors_allowed_headers)?;
            if self.http_cors_allowed_origins.iter().any(|value| {
                let trimmed = value.trim();
                trimmed.is_empty() || (trimmed != "*" && !trimmed.contains("://"))
            }) {
                return Err(ConfigError::Validation(
                    "http_cors_allowed_origins entries must be '*' or absolute origins".to_owned(),
                ));
            }
        }
        if self.logging_enabled && matches!(self.log_destination, LogDestination::File) {
            match &self.log_file_path {
                Some(path) if !path.as_os_str().is_empty() => {}
                _ => {
                    return Err(ConfigError::Validation(
                        "log_file_path is required when log_destination is file".to_owned(),
                    ));
                }
            }
        }
        if self.logging_enabled && self.log_level.trim().is_empty() {
            return Err(ConfigError::Validation(
                "log_level must not be empty when logging is enabled".to_owned(),
            ));
        }
        let guardrail_warnings = self.production_guardrail_warnings();
        if self.production_mode && !guardrail_warnings.is_empty() {
            return Err(ConfigError::Validation(format!(
                "production guardrails failed: {}",
                guardrail_warnings.join("; ")
            )));
        }
        Ok(())
    }

    pub fn production_guardrail_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if !self.require_auth {
            warnings.push("require_auth should be true".to_owned());
        }
        let jwt_configured = self.auth.jwt_secret.is_some()
            || self.auth.jwt_public_key_pem.is_some()
            || self.auth.jwks_url.is_some();
        if self.auth.bearer_enabled() && !jwt_configured {
            warnings.push("static bearer token is the only configured auth mechanism".to_owned());
        }
        if jwt_configured && self.auth.bearer_token.is_some() && !self.auth.disable_bearer_token {
            warnings.push(
                "static bearer token should be disabled when JWT/JWKS is configured".to_owned(),
            );
        }
        if self
            .auth
            .jwks_url
            .as_deref()
            .is_some_and(|url| !url.starts_with("https://"))
        {
            warnings.push("jwks_url should use https".to_owned());
        }
        if jwt_configured && self.auth.jwt_issuer.as_deref().is_none_or(str::is_empty) {
            warnings.push("jwt_issuer should be configured".to_owned());
        }
        if jwt_configured && self.auth.jwt_audience.as_deref().is_none_or(str::is_empty) {
            warnings.push("jwt_audience should be configured".to_owned());
        }
        if self.http_cors_enabled
            && self.auth.auth_enabled()
            && self
                .http_cors_allowed_origins
                .iter()
                .any(|origin| origin.trim() == "*")
        {
            warnings.push("wildcard CORS origins should not be used with auth enabled".to_owned());
        }
        if self.follow_host.is_some() || self.follow_port.is_some() {
            match self.replication_credential.as_deref().map(str::trim) {
                Some(value) if value.len() >= 16 => {}
                _ => warnings.push(
                    "replication_credential should be at least 16 characters when following"
                        .to_owned(),
                ),
            }
        }
        warnings
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file is missing an extension; use .json or .toml")]
    MissingExtension,
    #[error("unsupported config format: {0}")]
    UnsupportedFormat(String),
    #[error("failed to read config: {0}")]
    Io(String),
    #[error("failed to parse config: {0}")]
    Parse(String),
    #[error("invalid config: {0}")]
    Validation(String),
}

pub fn load_from_path(path: &Path) -> Result<RuntimeConfig, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|error| ConfigError::Io(error.to_string()))?;
    let mut config = match extension(path)? {
        "json" => serde_json::from_str::<RuntimeConfig>(&raw)
            .map_err(|error| ConfigError::Parse(error.to_string()))?,
        "toml" => toml::from_str::<RuntimeConfig>(&raw)
            .map_err(|error| ConfigError::Parse(error.to_string()))?,
        other => return Err(ConfigError::UnsupportedFormat(other.to_owned())),
    };
    config.config_path = Some(path.to_path_buf());
    Ok(config)
}

pub fn save_to_path(config: &RuntimeConfig, path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| ConfigError::Io(error.to_string()))?;
    }

    let mut serializable = config.clone();
    serializable.config_path = None;

    let rendered = match extension(path)? {
        "json" => serde_json::to_string_pretty(&serializable)
            .map_err(|error| ConfigError::Parse(error.to_string()))?,
        "toml" => toml::to_string_pretty(&serializable)
            .map_err(|error| ConfigError::Parse(error.to_string()))?,
        other => return Err(ConfigError::UnsupportedFormat(other.to_owned())),
    };

    std::fs::write(path, rendered).map_err(|error| ConfigError::Io(error.to_string()))
}

pub fn default_listen_addr() -> String {
    "127.0.0.1:7421".to_owned()
}

pub fn default_capnp_listen_addr() -> String {
    "127.0.0.1:7422".to_owned()
}

pub fn default_capnp_enabled() -> bool {
    false
}

pub fn default_server_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn default_subscriber_queue_capacity() -> usize {
    4_096
}

pub fn default_webhook_timeout_ms() -> u64 {
    5_000
}

pub fn default_webhook_concurrency_limit() -> usize {
    128
}

pub fn default_webhook_retry_count() -> u32 {
    8
}

pub fn default_webhook_retry_initial_backoff_ms() -> u64 {
    200
}

pub fn default_webhook_retry_max_backoff_ms() -> u64 {
    30_000
}

pub fn default_webhook_lease_ms() -> u64 {
    30_000
}

pub fn default_native_executor_threads() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .max(1)
}

pub fn default_native_executor_queue_limit() -> usize {
    default_native_executor_threads().saturating_mul(64).max(1)
}

pub fn default_aof_writer_queue_limit() -> usize {
    4_096
}

pub fn default_aof_group_commit_delay_ms() -> u64 {
    1
}

pub fn default_aof_group_commit_max_requests() -> usize {
    128
}

pub fn default_replication_batch_size() -> usize {
    512
}

pub fn default_replication_reconnect_backoff_ms() -> u64 {
    1_000
}

pub fn default_http_cors_allowed_methods() -> Vec<String> {
    ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub fn default_http_cors_allowed_headers() -> Vec<String> {
    ["authorization", "content-type", "x-request-id"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub fn default_http_max_body_bytes() -> usize {
    10 * 1024 * 1024
}

pub fn default_http_request_timeout_ms() -> u64 {
    30_000
}

pub fn default_http_rate_limit_requests_per_second() -> u64 {
    1_000
}

pub fn default_http_rate_limit_burst() -> u64 {
    1_000
}

pub fn default_http_principal_rate_limit_requests_per_second() -> u64 {
    100
}

pub fn default_http_principal_rate_limit_burst() -> u64 {
    200
}

pub fn default_logging_enabled() -> bool {
    true
}

pub fn default_log_level() -> String {
    "info".to_owned()
}

pub fn normalize_command_key(command: &str) -> String {
    command.trim().to_ascii_lowercase()
}

fn validate_positive_usize(name: &str, value: usize) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::Validation(format!(
            "{name} must be greater than zero"
        )));
    }
    Ok(())
}

fn validate_positive_u64(name: &str, value: u64) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::Validation(format!(
            "{name} must be greater than zero"
        )));
    }
    Ok(())
}

fn validate_non_empty_list(name: &str, values: &[String]) -> Result<(), ConfigError> {
    if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
        return Err(ConfigError::Validation(format!(
            "{name} must contain non-empty values"
        )));
    }
    Ok(())
}

fn extension(path: &Path) -> Result<&str, ConfigError> {
    path.extension()
        .and_then(|value| value.to_str())
        .ok_or(ConfigError::MissingExtension)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        LogDestination, LogFormat, RuntimeConfig, StorageMode, load_from_path, save_to_path,
    };

    #[test]
    fn json_roundtrip_preserves_runtime_values() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("latlng.json");
        let mut config = RuntimeConfig {
            production_mode: true,
            listen_addr: "127.0.0.1:9999".to_owned(),
            capnp_enabled: false,
            server_id: "server-1".to_owned(),
            storage: StorageMode::Aof {
                path: dir.path().join("appendonly.aof"),
            },
            read_only: true,
            subscriber_queue_capacity: 32,
            webhook_queue_path: Some(dir.path().join("webhooks.sqlite")),
            webhook_timeout_ms: 1_500,
            webhook_concurrency_limit: 4,
            webhook_retry_count: 9,
            webhook_retry_initial_backoff_ms: 250,
            webhook_retry_max_backoff_ms: 5_000,
            webhook_lease_ms: 45_000,
            native_executor_threads: 6,
            native_executor_queue_limit: 96,
            aof_writer_queue_limit: 512,
            aof_group_commit_delay_ms: 2,
            aof_group_commit_max_requests: 32,
            follow_host: Some("127.0.0.1".to_owned()),
            follow_port: Some(9_851),
            replication_credential: Some("replication-secret".to_owned()),
            replication_batch_size: 256,
            replication_reconnect_backoff_ms: 2_500,
            http_cors_enabled: true,
            http_cors_allowed_origins: vec!["https://app.example.test".to_owned()],
            http_cors_allowed_methods: vec!["GET".to_owned(), "POST".to_owned()],
            http_cors_allowed_headers: vec!["authorization".to_owned()],
            http_cors_max_age_seconds: Some(600),
            http_max_body_bytes: 1_048_576,
            http_request_timeout_ms: 2_500,
            http_rate_limit_enabled: true,
            http_rate_limit_requests_per_second: 100,
            http_rate_limit_burst: 200,
            http_principal_rate_limit_enabled: true,
            http_principal_rate_limit_requests_per_second: 10,
            http_principal_rate_limit_burst: 20,
            logging_enabled: true,
            log_format: LogFormat::Json,
            log_level: "debug".to_owned(),
            log_destination: LogDestination::File,
            log_file_path: Some(dir.path().join("latlng.log")),
            ..RuntimeConfig::default()
        };
        config.set_timeout("set", 1.5);
        config.auth.bearer_token = Some("secret".to_owned());
        config.auth.disable_bearer_token = true;
        config.auth.jwt_public_key_pem = Some("-----BEGIN PUBLIC KEY-----".to_owned());
        config.auth.jwt_algorithm = Some("RS256".to_owned());
        config.auth.jwks_url = Some("https://auth.example.test/.well-known/jwks.json".to_owned());
        config.auth.jwks_provider_id = Some("auth-example".to_owned());
        config.auth.jwks_refresh_interval_seconds = 120;
        config.auth.jwks_cache_ttl_seconds = 600;
        config.auth.jwks_http_timeout_ms = 1_500;

        save_to_path(&config, &path).unwrap();
        let loaded = load_from_path(&path).unwrap();

        assert_eq!(loaded.listen_addr, "127.0.0.1:9999");
        assert!(!loaded.capnp_enabled);
        assert!(loaded.production_mode);
        assert_eq!(loaded.server_id, "server-1");
        assert!(loaded.read_only);
        assert_eq!(loaded.timeout_for("SET"), Some(1.5));
        assert_eq!(loaded.subscriber_queue_capacity, 32);
        assert_eq!(
            loaded.webhook_queue_path,
            Some(dir.path().join("webhooks.sqlite"))
        );
        assert_eq!(loaded.webhook_timeout_ms, 1_500);
        assert_eq!(loaded.webhook_concurrency_limit, 4);
        assert_eq!(loaded.webhook_retry_count, 9);
        assert_eq!(loaded.webhook_retry_initial_backoff_ms, 250);
        assert_eq!(loaded.webhook_retry_max_backoff_ms, 5_000);
        assert_eq!(loaded.webhook_lease_ms, 45_000);
        assert_eq!(loaded.native_executor_threads, 6);
        assert_eq!(loaded.native_executor_queue_limit, 96);
        assert_eq!(loaded.aof_writer_queue_limit, 512);
        assert_eq!(loaded.aof_group_commit_delay_ms, 2);
        assert_eq!(loaded.aof_group_commit_max_requests, 32);
        assert_eq!(loaded.follow_host.as_deref(), Some("127.0.0.1"));
        assert_eq!(loaded.follow_port, Some(9_851));
        assert_eq!(
            loaded.replication_credential.as_deref(),
            Some("replication-secret")
        );
        assert_eq!(loaded.replication_batch_size, 256);
        assert_eq!(loaded.replication_reconnect_backoff_ms, 2_500);
        assert!(loaded.http_cors_enabled);
        assert_eq!(
            loaded.http_cors_allowed_origins,
            vec!["https://app.example.test"]
        );
        assert_eq!(loaded.http_cors_allowed_methods, vec!["GET", "POST"]);
        assert_eq!(loaded.http_cors_allowed_headers, vec!["authorization"]);
        assert_eq!(loaded.http_cors_max_age_seconds, Some(600));
        assert_eq!(loaded.http_max_body_bytes, 1_048_576);
        assert_eq!(loaded.http_request_timeout_ms, 2_500);
        assert!(loaded.http_rate_limit_enabled);
        assert_eq!(loaded.http_rate_limit_requests_per_second, 100);
        assert_eq!(loaded.http_rate_limit_burst, 200);
        assert!(loaded.http_principal_rate_limit_enabled);
        assert_eq!(loaded.http_principal_rate_limit_requests_per_second, 10);
        assert_eq!(loaded.http_principal_rate_limit_burst, 20);
        assert!(loaded.logging_enabled);
        assert_eq!(loaded.log_format, LogFormat::Json);
        assert_eq!(loaded.log_level, "debug");
        assert_eq!(loaded.log_destination, LogDestination::File);
        assert_eq!(loaded.log_file_path, Some(dir.path().join("latlng.log")));
        assert_eq!(loaded.auth.bearer_token.as_deref(), Some("secret"));
        assert!(loaded.auth.disable_bearer_token);
        assert_eq!(
            loaded.auth.jwt_public_key_pem.as_deref(),
            Some("-----BEGIN PUBLIC KEY-----")
        );
        assert_eq!(loaded.auth.jwt_algorithm.as_deref(), Some("RS256"));
        assert_eq!(
            loaded.auth.jwks_url.as_deref(),
            Some("https://auth.example.test/.well-known/jwks.json")
        );
        assert_eq!(
            loaded.auth.jwks_provider_id.as_deref(),
            Some("auth-example")
        );
        assert_eq!(loaded.auth.jwks_refresh_interval_seconds, 120);
        assert_eq!(loaded.auth.jwks_cache_ttl_seconds, 600);
        assert_eq!(loaded.auth.jwks_http_timeout_ms, 1_500);
    }

    #[test]
    fn production_mode_rejects_unsafe_auth_configuration() {
        let config = RuntimeConfig {
            production_mode: true,
            require_auth: false,
            ..RuntimeConfig::default()
        };

        let error = config.validate_for_startup().unwrap_err().to_string();

        assert!(error.contains("production guardrails failed"));
        assert!(error.contains("require_auth should be true"));
    }

    #[test]
    fn non_production_reports_guardrail_warnings_without_failing() {
        let mut config = RuntimeConfig {
            require_auth: false,
            ..RuntimeConfig::default()
        };
        config.auth.bearer_token = Some("dev-token".to_owned());

        config.validate_for_startup().unwrap();
        assert!(
            config
                .production_guardrail_warnings()
                .iter()
                .any(|warning| warning.contains("static bearer token"))
        );
    }

    #[test]
    fn disabled_capnp_allows_empty_listen_addr() {
        let config = RuntimeConfig {
            capnp_enabled: false,
            capnp_listen_addr: String::new(),
            ..RuntimeConfig::default()
        };

        config.validate_for_startup().unwrap();
    }

    #[test]
    fn config_reference_includes_capnp_enabled() {
        let reference = super::config_reference();
        let capnp_enabled = reference
            .iter()
            .find(|entry| entry.name == "capnp_enabled")
            .expect("capnp_enabled must be documented");

        assert_eq!(capnp_enabled.kind, "bool");
        assert_eq!(capnp_enabled.default, serde_json::json!(false));
    }

    #[test]
    fn rate_limiter_defaults_and_validation_are_stable() {
        let config = RuntimeConfig::default();

        assert!(!config.http_rate_limit_enabled);
        assert!(!config.http_principal_rate_limit_enabled);
        assert_eq!(config.http_principal_rate_limit_requests_per_second, 100);
        assert_eq!(config.http_principal_rate_limit_burst, 200);
        config.validate_for_startup().unwrap();

        let invalid_rate = RuntimeConfig {
            http_principal_rate_limit_requests_per_second: 0,
            ..RuntimeConfig::default()
        };
        assert!(
            invalid_rate
                .validate_for_startup()
                .unwrap_err()
                .to_string()
                .contains("http_principal_rate_limit_requests_per_second")
        );

        let invalid_burst = RuntimeConfig {
            http_principal_rate_limit_burst: 0,
            ..RuntimeConfig::default()
        };
        assert!(
            invalid_burst
                .validate_for_startup()
                .unwrap_err()
                .to_string()
                .contains("http_principal_rate_limit_burst")
        );

        let reference = super::config_reference();
        assert!(
            reference
                .iter()
                .any(|entry| entry.name == "http_principal_rate_limit_enabled")
        );
    }
}
