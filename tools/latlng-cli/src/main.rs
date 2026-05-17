#![forbid(unsafe_code)]

use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand, ValueEnum};
use latlng_auth::{
    AuthAction, AuthConfig, HmacJwtAlgorithm, HmacSecretFormat, HmacTokenOptions,
    HmacTokenPermissionRule, create_hmac_jwt, decode_jwt_unverified, generate_hmac_secret,
};
use latlng_config::{config_reference_json, load_from_path};
use latlng_storage_aof::{backup_aof, restore_aof, verify_aof};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use url::form_urlencoded::byte_serialize;

pub const SUPPORTED_COMMANDS: &[&str] = &[
    "ping",
    "server",
    "healthz",
    "info",
    "collections",
    "collection-create",
    "collection-get",
    "collection-drop",
    "metrics",
    "get",
    "del",
    "pdel",
    "expire",
    "persist",
    "ttl",
    "fset",
    "fget",
    "jset",
    "jget",
    "jdel",
    "bounds",
    "stats",
    "set-point",
    "nearby",
    "channels",
    "channel-get",
    "channel-set",
    "channel-del",
    "hooks",
    "hook-get",
    "hook-set",
    "hook-del",
    "config-get",
    "config-set",
    "config-validate",
    "config-reference",
    "config-rewrite",
    "token",
    "token-create",
    "token-secret",
    "token-inspect",
    "token-verify",
    "readonly",
    "timeout",
    "aofshrink",
    "aof-verify",
    "aof-backup",
    "aof-restore",
];

#[derive(Debug, Parser)]
#[command(name = "latlng-cli", about = "CLI tooling for latlng", version)]
struct Cli {
    #[arg(long, global = true, default_value = "http://127.0.0.1:7421")]
    base_url: String,
    #[arg(long, global = true, env = "LATLNG_TOKEN")]
    token: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Ping,
    Server,
    Healthz,
    Info {
        section: Option<String>,
    },
    Collections {
        match_pattern: Option<String>,
    },
    CollectionCreate {
        collection: String,
    },
    CollectionGet {
        collection: String,
    },
    CollectionDrop {
        collection: String,
    },
    Metrics,
    Get {
        collection: String,
        id: String,
    },
    Del {
        collection: String,
        id: String,
    },
    Pdel {
        collection: String,
        match_pattern: String,
    },
    Expire {
        collection: String,
        id: String,
        seconds: u32,
    },
    Persist {
        collection: String,
        id: String,
    },
    Ttl {
        collection: String,
        id: String,
    },
    Fset {
        collection: String,
        id: String,
        field: String,
        value: String,
        #[arg(long)]
        xx: bool,
        #[arg(long)]
        json: bool,
    },
    Fget {
        collection: String,
        id: String,
        field: String,
    },
    Jset {
        collection: String,
        id: String,
        path: String,
        value: String,
        #[arg(long)]
        raw: bool,
    },
    Jget {
        collection: String,
        id: String,
        path: String,
    },
    Jdel {
        collection: String,
        id: String,
        path: String,
    },
    Bounds {
        collection: String,
    },
    Stats {
        collection: String,
    },
    SetPoint {
        collection: String,
        id: String,
        lat: f64,
        lon: f64,
    },
    Nearby {
        collection: String,
        lat: f64,
        lon: f64,
        meters: f64,
    },
    Channels {
        match_pattern: Option<String>,
    },
    ChannelGet {
        name: String,
    },
    ChannelSet {
        name: String,
        #[arg(long)]
        geojson: PathBuf,
        #[arg(long)]
        collection: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long, value_delimiter = ',')]
        detect: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        commands: Vec<String>,
    },
    ChannelDel {
        name: String,
    },
    Hooks {
        match_pattern: Option<String>,
    },
    HookGet {
        name: String,
    },
    HookSet {
        name: String,
        endpoint: String,
        #[arg(long)]
        geojson: PathBuf,
        #[arg(long)]
        collection: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long, value_delimiter = ',')]
        detect: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        commands: Vec<String>,
    },
    HookDel {
        name: String,
    },
    ConfigGet {
        name: String,
    },
    ConfigSet {
        name: String,
        value: String,
    },
    ConfigValidate {
        path: Option<PathBuf>,
    },
    ConfigReference,
    ConfigRewrite,
    Readonly {
        enabled: String,
    },
    Timeout {
        command: String,
        seconds: f64,
    },
    Aofshrink,
    AofVerify {
        path: PathBuf,
    },
    AofBackup {
        source: PathBuf,
        destination: PathBuf,
    },
    AofRestore {
        backup: PathBuf,
        destination: PathBuf,
        #[arg(long)]
        force: bool,
    },
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
}

#[derive(Debug, Subcommand)]
enum TokenCommand {
    Create(TokenCreateArgs),
    Secret(TokenSecretArgs),
    Inspect { token: String },
    Verify(TokenVerifyArgs),
}

#[derive(Debug, Args)]
struct TokenCreateArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(flatten)]
    secret: TokenSecretSourceArgs,
    #[arg(long)]
    subject: Option<String>,
    #[arg(long, default_value = "1h")]
    ttl: String,
    #[arg(long)]
    issuer: Option<String>,
    #[arg(long)]
    audience: Option<String>,
    #[arg(long, value_enum)]
    algorithm: Option<CliHmacAlgorithm>,
    #[arg(long, value_enum)]
    preset: Vec<TokenPreset>,
    #[arg(long)]
    collection: Vec<String>,
    #[arg(long)]
    action: Vec<String>,
    #[arg(long)]
    admin: bool,
    #[arg(long, value_enum, default_value = "token")]
    format: TokenOutputFormat,
}

#[derive(Debug, Args)]
struct TokenVerifyArgs {
    token: String,
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(flatten)]
    secret: TokenSecretSourceArgs,
    #[arg(long)]
    issuer: Option<String>,
    #[arg(long)]
    audience: Option<String>,
    #[arg(long, value_enum)]
    algorithm: Option<CliHmacAlgorithm>,
}

#[derive(Debug, Args, Default)]
struct TokenSecretSourceArgs {
    #[arg(long)]
    secret: Option<String>,
    #[arg(long)]
    secret_env: Option<String>,
    #[arg(long)]
    secret_file: Option<PathBuf>,
    #[arg(long)]
    secret_stdin: bool,
}

#[derive(Debug, Args)]
struct TokenSecretArgs {
    #[arg(long, default_value_t = 32)]
    bytes: usize,
    #[arg(long, value_enum, default_value = "base64-url")]
    format: TokenSecretOutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliHmacAlgorithm {
    #[value(name = "HS256", alias = "hs256")]
    Hs256,
    #[value(name = "HS384", alias = "hs384")]
    Hs384,
    #[value(name = "HS512", alias = "hs512")]
    Hs512,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum TokenPreset {
    Readonly,
    Writer,
    Dashboard,
    HooksAdmin,
    ChannelsAdmin,
    Metrics,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum TokenOutputFormat {
    Token,
    Json,
    Env,
    Curl,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum TokenSecretOutputFormat {
    Base64Url,
    Hex,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Ping);
    let client = reqwest::Client::new();
    let token = cli.token.as_deref();

    match command {
        Command::Ping => {
            print_response(send(get(&client, &cli.base_url, "/ping", token)).await?).await?
        }
        Command::Server => {
            print_response(send(get(&client, &cli.base_url, "/server", token)).await?).await?
        }
        Command::Healthz => {
            print_response(send(get(&client, &cli.base_url, "/healthz", token)).await?).await?
        }
        Command::Info { section } => {
            let path = section
                .map(|section| format!("/info?section={section}"))
                .unwrap_or_else(|| "/info".to_owned());
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Collections { match_pattern } => {
            let path = match_pattern
                .map(|pattern| format!("/collections?match_pattern={}", query_value(&pattern)))
                .unwrap_or_else(|| "/collections".to_owned());
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::CollectionCreate { collection } => {
            let path = format!("/collections/{}", path_segment(&collection));
            print_response(send(post_empty(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::CollectionGet { collection } => {
            let path = format!("/collections/{}", path_segment(&collection));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::CollectionDrop { collection } => {
            let path = format!("/collections/{}", path_segment(&collection));
            print_response(send(delete(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Metrics => {
            print_response(send(get(&client, &cli.base_url, "/metrics", token)).await?).await?
        }
        Command::Get { collection, id } => {
            let path = object_path(&collection, &id);
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Del { collection, id } => {
            let path = object_path(&collection, &id);
            print_response(send(delete(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Pdel {
            collection,
            match_pattern,
        } => {
            let path = format!(
                "/collections/{}/objects?match_pattern={}",
                path_segment(&collection),
                query_value(&match_pattern)
            );
            print_response(send(delete(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Expire {
            collection,
            id,
            seconds,
        } => {
            let path = format!("{}/expire", object_path(&collection, &id));
            let body = serde_json::json!({ "seconds": seconds });
            print_response(send(post_json(&client, &cli.base_url, &path, token, &body)).await?)
                .await?;
        }
        Command::Persist { collection, id } => {
            let path = format!("{}/expire", object_path(&collection, &id));
            print_response(send(delete(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Ttl { collection, id } => {
            let path = format!("{}/ttl", object_path(&collection, &id));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Fset {
            collection,
            id,
            field,
            value,
            xx,
            json,
        } => {
            let path = format!("{}/fields", object_path(&collection, &id));
            let body = serde_json::json!({
                "fields": [{
                    "name": field,
                    "value": parse_field_value(&value, json)?,
                }],
                "xx": xx,
            });
            print_response(send(post_json(&client, &cli.base_url, &path, token, &body)).await?)
                .await?;
        }
        Command::Fget {
            collection,
            id,
            field,
        } => {
            let path = format!(
                "{}/fields/{}",
                object_path(&collection, &id),
                path_segment(&field)
            );
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Jset {
            collection,
            id,
            path,
            value,
            raw,
        } => {
            let route = format!("{}/json", object_path(&collection, &id));
            let body = serde_json::json!({ "path": path, "value": value, "raw": raw });
            print_response(send(post_json(&client, &cli.base_url, &route, token, &body)).await?)
                .await?;
        }
        Command::Jget {
            collection,
            id,
            path,
        } => {
            let route = format!(
                "{}/json/{}",
                object_path(&collection, &id),
                wildcard_path(&path)
            );
            print_response(send(get(&client, &cli.base_url, &route, token)).await?).await?;
        }
        Command::Jdel {
            collection,
            id,
            path,
        } => {
            let route = format!(
                "{}/json/{}",
                object_path(&collection, &id),
                wildcard_path(&path)
            );
            print_response(send(delete(&client, &cli.base_url, &route, token)).await?).await?;
        }
        Command::Bounds { collection } => {
            let path = format!("/collections/{}/bounds", path_segment(&collection));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Stats { collection } => {
            let path = format!("/collections/{}/stats", path_segment(&collection));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::SetPoint {
            collection,
            id,
            lat,
            lon,
        } => {
            let path = object_path(&collection, &id);
            let body = serde_json::json!({
                "object": {
                    "Point": {
                        "lat": lat,
                        "lon": lon,
                        "z": null
                    }
                }
            });
            print_response(send(post_json(&client, &cli.base_url, &path, token, &body)).await?)
                .await?;
        }
        Command::Nearby {
            collection,
            lat,
            lon,
            meters,
        } => {
            let path = format!("/collections/{}/search/nearby", path_segment(&collection));
            let body = serde_json::json!({
                "lat": lat,
                "lon": lon,
                "meters": meters,
                "options": {}
            });
            print_response(send(post_json(&client, &cli.base_url, &path, token, &body)).await?)
                .await?;
        }
        Command::Channels { match_pattern } => {
            let path = match_pattern
                .map(|pattern| format!("/channels?match_pattern={}", query_value(&pattern)))
                .unwrap_or_else(|| "/channels".to_owned());
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::ChannelGet { name } => {
            let path = format!("/channels/{}", path_segment(&name));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::ChannelSet {
            name,
            geojson,
            collection,
            mode,
            detect,
            commands,
        } => {
            let def = geofence_def_from_geojson(&geojson, collection, mode, detect, commands)?;
            let body = serde_json::json!({ "name": name, "def": def });
            print_response(
                send(post_json(&client, &cli.base_url, "/channels", token, &body)).await?,
            )
            .await?;
        }
        Command::ChannelDel { name } => {
            let path = format!("/channels/{}", path_segment(&name));
            print_response(send(delete(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::Hooks { match_pattern } => {
            let path = match_pattern
                .map(|pattern| format!("/hooks?match_pattern={}", query_value(&pattern)))
                .unwrap_or_else(|| "/hooks".to_owned());
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::HookGet { name } => {
            let path = format!("/hooks/{}", path_segment(&name));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::HookSet {
            name,
            endpoint,
            geojson,
            collection,
            mode,
            detect,
            commands,
        } => {
            let def = geofence_def_from_geojson(&geojson, collection, mode, detect, commands)?;
            let body = serde_json::json!({ "name": name, "endpoint": endpoint, "def": def });
            print_response(send(post_json(&client, &cli.base_url, "/hooks", token, &body)).await?)
                .await?;
        }
        Command::HookDel { name } => {
            let path = format!("/hooks/{}", path_segment(&name));
            print_response(send(delete(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::ConfigGet { name } => {
            let path = format!("/config/{}", path_segment(&name));
            print_response(send(get(&client, &cli.base_url, &path, token)).await?).await?;
        }
        Command::ConfigSet { name, value } => {
            let path = format!("/config/{}", path_segment(&name));
            let body = serde_json::json!({ "value": value });
            print_response(send(post_json(&client, &cli.base_url, &path, token, &body)).await?)
                .await?;
        }
        Command::ConfigValidate { path } => {
            let path = path
                .or_else(|| std::env::var("LATLNG_CONFIG").ok().map(PathBuf::from))
                .ok_or("missing config path")?;
            let config = load_from_path(&path)?;
            config.validate_for_startup()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": true,
                    "path": path,
                    "listen_addr": config.listen_addr,
                    "capnp_listen_addr": config.capnp_listen_addr,
                    "production_mode": config.production_mode,
                    "require_auth": config.require_auth,
                    "auth_enabled": config.auth.auth_enabled(),
                    "production_guardrail_warnings": config.production_guardrail_warnings(),
                    "cors_enabled": config.http_cors_enabled,
                    "logging_enabled": config.logging_enabled
                }))?
            );
        }
        Command::ConfigReference => {
            println!(
                "{}",
                serde_json::to_string_pretty(&config_reference_json())?
            );
        }
        Command::ConfigRewrite => {
            print_response(
                send(post_empty(
                    &client,
                    &cli.base_url,
                    "/admin/config/rewrite",
                    token,
                ))
                .await?,
            )
            .await?;
        }
        Command::Readonly { enabled } => {
            let body = serde_json::json!({ "enabled": parse_boolish(&enabled) });
            print_response(
                send(post_json(
                    &client,
                    &cli.base_url,
                    "/admin/readonly",
                    token,
                    &body,
                ))
                .await?,
            )
            .await?;
        }
        Command::Timeout { command, seconds } => {
            let body = serde_json::json!({
                "command": command,
                "seconds": seconds
            });
            print_response(
                send(post_json(
                    &client,
                    &cli.base_url,
                    "/admin/timeout",
                    token,
                    &body,
                ))
                .await?,
            )
            .await?;
        }
        Command::Aofshrink => {
            print_response(
                send(post_empty(
                    &client,
                    &cli.base_url,
                    "/admin/aofshrink",
                    token,
                ))
                .await?,
            )
            .await?;
        }
        Command::AofVerify { path } => {
            let report = verify_aof(path)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::AofBackup {
            source,
            destination,
        } => {
            let report = backup_aof(source, destination)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::AofRestore {
            backup,
            destination,
            force,
        } => {
            let report = restore_aof(backup, destination, force)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Token { command } => {
            handle_token_command(command, &cli.base_url).await?;
        }
    }

    Ok(())
}

async fn handle_token_command(
    command: TokenCommand,
    base_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        TokenCommand::Create(args) => create_token(args, base_url),
        TokenCommand::Secret(args) => print_token_secret(args),
        TokenCommand::Inspect { token } => inspect_token(&token),
        TokenCommand::Verify(args) => verify_token(args).await,
    }
}

fn create_token(args: TokenCreateArgs, base_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_token_config(
        args.config.as_ref(),
        &args.secret,
        args.issuer.as_deref(),
        args.audience.as_deref(),
        args.algorithm,
    )?;
    let ttl_seconds = parse_duration_seconds(&args.ttl)?;
    let issued_at = unix_timestamp()?;
    let permissions = build_token_permissions(&args)?;
    let options = HmacTokenOptions {
        subject: args.subject.clone(),
        issuer: resolved.issuer,
        audience: resolved.audience,
        issued_at,
        not_before: Some(issued_at),
        expires_at: issued_at + ttl_seconds,
        permissions,
        admin: args.admin,
        algorithm: resolved.algorithm,
    };
    let token = create_hmac_jwt(&resolved.secret, &options)?;
    match args.format {
        TokenOutputFormat::Token => println!("{token}"),
        TokenOutputFormat::Env => println!("LATLNG_TOKEN={token}"),
        TokenOutputFormat::Curl => {
            println!("curl -H 'Authorization: Bearer {token}' {base_url}/ping")
        }
        TokenOutputFormat::Json => {
            let decoded = decode_jwt_unverified(&token)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "token": token,
                    "header": decoded.header,
                    "claims": decoded.claims
                }))?
            );
        }
    }
    Ok(())
}

fn print_token_secret(args: TokenSecretArgs) -> Result<(), Box<dyn std::error::Error>> {
    let format = match args.format {
        TokenSecretOutputFormat::Base64Url => HmacSecretFormat::Base64Url,
        TokenSecretOutputFormat::Hex => HmacSecretFormat::Hex,
    };
    println!("{}", generate_hmac_secret(args.bytes, format)?);
    Ok(())
}

fn inspect_token(token: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&decode_jwt_unverified(token)?)?
    );
    Ok(())
}

async fn verify_token(args: TokenVerifyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_token_config(
        args.config.as_ref(),
        &args.secret,
        args.issuer.as_deref(),
        args.audience.as_deref(),
        args.algorithm,
    )?;
    let auth = AuthConfig {
        jwt_secret: Some(resolved.secret),
        jwt_algorithm: Some(resolved.algorithm.as_str().to_owned()),
        jwt_issuer: resolved.issuer,
        jwt_audience: resolved.audience,
        ..AuthConfig::default()
    }
    .authenticator()?;
    let principal = auth.authenticate(Some(&args.token)).await?;
    let decoded = decode_jwt_unverified(&args.token)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "ok": true,
            "principal": {
                "rate_limit_key": principal.rate_limit_key,
                "admin": principal.is_admin()
            },
            "header": decoded.header,
            "claims": decoded.claims
        }))?
    );
    Ok(())
}

#[derive(Debug)]
struct ResolvedTokenConfig {
    secret: String,
    issuer: Option<String>,
    audience: Option<String>,
    algorithm: HmacJwtAlgorithm,
}

fn resolve_token_config(
    config_path: Option<&PathBuf>,
    secret_source: &TokenSecretSourceArgs,
    issuer_override: Option<&str>,
    audience_override: Option<&str>,
    algorithm_override: Option<CliHmacAlgorithm>,
) -> Result<ResolvedTokenConfig, Box<dyn std::error::Error>> {
    let config = match config_path {
        Some(path) => Some(load_from_path(path)?),
        None => std::env::var("LATLNG_CONFIG")
            .ok()
            .map(PathBuf::from)
            .map(|path| load_from_path(&path))
            .transpose()?,
    };
    if let Some(config) = &config
        && (config.auth.jwt_public_key_pem.is_some() || config.auth.jwks_url.is_some())
        && config.auth.jwt_secret.is_none()
    {
        return Err(
            "token create/verify currently supports HMAC JWTs only; configure jwt_secret or pass a secret source"
                .into(),
        );
    }

    let secret = resolve_token_secret(
        secret_source,
        config
            .as_ref()
            .and_then(|config| config.auth.jwt_secret.as_deref().map(str::to_owned)),
    )?;
    let issuer = issuer_override.map(str::to_owned).or_else(|| {
        config
            .as_ref()
            .and_then(|config| config.auth.jwt_issuer.clone())
    });
    let audience = audience_override.map(str::to_owned).or_else(|| {
        config
            .as_ref()
            .and_then(|config| config.auth.jwt_audience.clone())
    });
    let algorithm = match algorithm_override {
        Some(algorithm) => algorithm.into(),
        None => config
            .as_ref()
            .and_then(|config| config.auth.jwt_algorithm.as_deref())
            .map(HmacJwtAlgorithm::parse)
            .transpose()?
            .unwrap_or_default(),
    };

    if let Some(config) = &config
        && config.production_mode
        && issuer.as_deref().is_none_or(str::is_empty)
    {
        return Err("production token creation requires jwt_issuer or --issuer".into());
    }
    if let Some(config) = &config
        && config.production_mode
        && audience.as_deref().is_none_or(str::is_empty)
    {
        return Err("production token creation requires jwt_audience or --audience".into());
    }

    Ok(ResolvedTokenConfig {
        secret,
        issuer,
        audience,
        algorithm,
    })
}

fn resolve_token_secret(
    source: &TokenSecretSourceArgs,
    config_secret: Option<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let explicit_sources = usize::from(source.secret.is_some())
        + usize::from(source.secret_env.is_some())
        + usize::from(source.secret_file.is_some())
        + usize::from(source.secret_stdin);
    if explicit_sources > 1 {
        return Err(
            "pass only one of --secret, --secret-env, --secret-file, or --secret-stdin".into(),
        );
    }
    let secret = if let Some(secret) = &source.secret {
        secret.clone()
    } else if let Some(name) = &source.secret_env {
        std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))?
    } else if let Some(path) = &source.secret_file {
        strip_trailing_newlines(std::fs::read_to_string(path)?)
    } else if source.secret_stdin {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        strip_trailing_newlines(input)
    } else if let Some(secret) = config_secret {
        secret
    } else if let Ok(secret) = std::env::var("LATLNG_JWT_SECRET") {
        secret
    } else {
        return Err(
            "missing HMAC secret; pass --config, --secret-env, --secret-file, or --secret-stdin"
                .into(),
        );
    };
    if secret.is_empty() {
        return Err("jwt_secret must not be empty".into());
    }
    Ok(secret)
}

fn strip_trailing_newlines(mut value: String) -> String {
    while value.ends_with('\n') || value.ends_with('\r') {
        value.pop();
    }
    value
}

impl From<CliHmacAlgorithm> for HmacJwtAlgorithm {
    fn from(value: CliHmacAlgorithm) -> Self {
        match value {
            CliHmacAlgorithm::Hs256 => Self::Hs256,
            CliHmacAlgorithm::Hs384 => Self::Hs384,
            CliHmacAlgorithm::Hs512 => Self::Hs512,
        }
    }
}

fn build_token_permissions(
    args: &TokenCreateArgs,
) -> Result<Vec<HmacTokenPermissionRule>, Box<dyn std::error::Error>> {
    let mut permissions = Vec::new();
    for preset in &args.preset {
        permissions.push(permission_for_preset(*preset, &args.collection)?);
    }
    if !args.action.is_empty() {
        if args.collection.is_empty() {
            return Err("--action requires at least one --collection".into());
        }
        permissions.push(HmacTokenPermissionRule::from_action_names(
            args.collection.clone(),
            args.action.iter(),
        )?);
    }
    if !args.admin && permissions.is_empty() {
        return Err("token create requires --admin, --preset, or at least one --action".into());
    }
    Ok(permissions)
}

fn permission_for_preset(
    preset: TokenPreset,
    collections: &[String],
) -> Result<HmacTokenPermissionRule, Box<dyn std::error::Error>> {
    let collection_patterns = match preset {
        TokenPreset::Metrics => vec!["*".to_owned()],
        _ if collections.is_empty() => {
            return Err(format!("--preset {preset} requires --collection").into());
        }
        _ => collections.to_vec(),
    };
    Ok(HmacTokenPermissionRule::from_actions(
        collection_patterns,
        preset.actions().iter().copied(),
    ))
}

impl TokenPreset {
    fn actions(self) -> &'static [AuthAction] {
        match self {
            Self::Readonly => &[
                AuthAction::CollectionsList,
                AuthAction::CollectionsInspect,
                AuthAction::ObjectsRead,
                AuthAction::QueriesRead,
            ],
            Self::Writer => &[
                AuthAction::CollectionsList,
                AuthAction::CollectionsInspect,
                AuthAction::ObjectsRead,
                AuthAction::ObjectsWrite,
                AuthAction::ObjectsDelete,
                AuthAction::QueriesRead,
            ],
            Self::Dashboard => &[
                AuthAction::CollectionsList,
                AuthAction::QueriesRead,
                AuthAction::SubscriptionsRead,
            ],
            Self::HooksAdmin => &[AuthAction::HooksManage],
            Self::ChannelsAdmin => &[AuthAction::ChannelsManage],
            Self::Metrics => &[AuthAction::MetricsRead],
        }
    }
}

impl std::fmt::Display for TokenPreset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Readonly => "readonly",
            Self::Writer => "writer",
            Self::Dashboard => "dashboard",
            Self::HooksAdmin => "hooks-admin",
            Self::ChannelsAdmin => "channels-admin",
            Self::Metrics => "metrics",
        })
    }
}

fn parse_duration_seconds(value: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("duration must not be empty".into());
    }
    let split_at = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split_at);
    if number.is_empty() {
        return Err(format!("invalid duration: {value}").into());
    }
    let amount: u64 = number.parse()?;
    let multiplier = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        other => return Err(format!("unsupported duration unit: {other}").into()),
    };
    let seconds = amount
        .checked_mul(multiplier)
        .ok_or("duration is too large")?;
    if seconds == 0 {
        return Err("duration must be greater than zero".into());
    }
    Ok(seconds)
}

fn unix_timestamp() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn get(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    apply_auth(client.get(format!("{base}{path}")), token)
}

fn post_json(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> reqwest::RequestBuilder {
    apply_auth(client.post(format!("{base}{path}")), token)
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
}

fn post_empty(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    apply_auth(client.post(format!("{base}{path}")), token)
}

fn delete(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    apply_auth(client.delete(format!("{base}{path}")), token)
}

fn apply_auth(builder: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    if let Some(token) = token {
        builder.header(AUTHORIZATION, format!("Bearer {token}"))
    } else {
        builder
    }
}

async fn send(builder: reqwest::RequestBuilder) -> Result<reqwest::Response, reqwest::Error> {
    builder.send().await
}

async fn print_response(response: reqwest::Response) -> Result<(), Box<dyn std::error::Error>> {
    let status = response.status();
    let body = response.text().await?;
    if status.is_success() {
        println!("{body}");
    } else {
        eprintln!("{status}: {body}");
    }
    Ok(())
}

fn parse_boolish(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn object_path(collection: &str, id: &str) -> String {
    format!(
        "/collections/{}/objects/{}",
        path_segment(collection),
        path_segment(id)
    )
}

fn path_segment(value: &str) -> String {
    byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn wildcard_path(value: &str) -> String {
    value
        .split('/')
        .map(path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn query_value(value: &str) -> String {
    path_segment(value)
}

fn parse_field_value(
    value: &str,
    as_json: bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if as_json {
        let parsed: serde_json::Value = serde_json::from_str(value)?;
        return Ok(serde_json::json!({ "type": "json", "value": parsed.to_string() }));
    }
    if let Ok(number) = value.parse::<f64>() {
        Ok(serde_json::json!({ "type": "number", "value": number }))
    } else {
        Ok(serde_json::json!({ "type": "text", "value": value }))
    }
}

fn geofence_def_from_geojson(
    path: &PathBuf,
    collection_override: Option<String>,
    mode_override: Option<String>,
    detect_override: Vec<String>,
    commands_override: Vec<String>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let geojson: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let properties = geojson
        .get("properties")
        .and_then(serde_json::Value::as_object);
    let collection = collection_override
        .or_else(|| {
            properties
                .and_then(|props| props.get("collection"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or("GeoJSON geofence requires --collection or properties.collection")?;
    let mode = mode_override
        .or_else(|| {
            properties
                .and_then(|props| props.get("mode"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "within".to_owned());
    let detect = if detect_override.is_empty() {
        string_array_property(properties, "detect")
            .unwrap_or_else(|| vec!["Enter".to_owned(), "Exit".to_owned()])
    } else {
        detect_override
    }
    .into_iter()
    .map(|value| normalize_detect(&value))
    .collect::<Result<Vec<_>, _>>()?;
    let commands = if commands_override.is_empty() {
        string_array_property(properties, "commands").unwrap_or_else(|| vec!["Set".to_owned()])
    } else {
        commands_override
    }
    .into_iter()
    .map(|value| normalize_command(&value))
    .collect::<Result<Vec<_>, _>>()?;
    let area = serde_json::json!({ "GeoJson": geojson });
    let query = match mode.trim().to_ascii_lowercase().as_str() {
        "within" => serde_json::json!({ "Within": { "area": area, "options": {} } }),
        "intersects" => serde_json::json!({ "Intersects": { "area": area, "options": {} } }),
        other => return Err(format!("unsupported GeoJSON geofence mode: {other}").into()),
    };
    Ok(serde_json::json!({
        "collection": collection,
        "query": query,
        "detect": detect,
        "commands": commands,
    }))
}

fn string_array_property(
    properties: Option<&serde_json::Map<String, serde_json::Value>>,
    name: &str,
) -> Option<Vec<String>> {
    properties?.get(name)?.as_array().map(|items| {
        items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    })
}

fn normalize_detect(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "inside" => Ok("Inside".to_owned()),
        "outside" => Ok("Outside".to_owned()),
        "enter" => Ok("Enter".to_owned()),
        "exit" => Ok("Exit".to_owned()),
        "cross" => Ok("Cross".to_owned()),
        "roam" => Ok("Roam".to_owned()),
        other => Err(format!("unsupported detect type: {other}").into()),
    }
}

fn normalize_command(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "set" => Ok("Set".to_owned()),
        "del" => Ok("Del".to_owned()),
        "drop" => Ok("Drop".to_owned()),
        "fset" => Ok("Fset".to_owned()),
        other => Err(format!("unsupported geofence command: {other}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SUPPORTED_COMMANDS, TokenPreset, geofence_def_from_geojson, object_path,
        parse_duration_seconds, permission_for_preset, wildcard_path,
    };
    use std::fs;

    #[test]
    fn supported_commands_do_not_expose_internal_test_api() {
        assert!(!SUPPORTED_COMMANDS.contains(&"test"));
        assert!(!SUPPORTED_COMMANDS.contains(&"follow"));
    }

    #[test]
    fn paths_are_percent_encoded() {
        assert_eq!(
            object_path("fleet/eu", "truck 1"),
            "/collections/fleet%2Feu/objects/truck%201"
        );
        assert_eq!(
            wildcard_path("properties/driver name"),
            "properties/driver%20name"
        );
    }

    #[test]
    fn token_duration_parser_accepts_common_units() {
        assert_eq!(parse_duration_seconds("30s").unwrap(), 30);
        assert_eq!(parse_duration_seconds("15m").unwrap(), 900);
        assert_eq!(parse_duration_seconds("2h").unwrap(), 7_200);
        assert_eq!(parse_duration_seconds("7d").unwrap(), 604_800);
        assert_eq!(parse_duration_seconds("60").unwrap(), 60);
        assert!(parse_duration_seconds("0s").is_err());
        assert!(parse_duration_seconds("1w").is_err());
    }

    #[test]
    fn token_presets_expand_to_expected_permissions() {
        let readonly =
            permission_for_preset(TokenPreset::Readonly, &["fleet-*".to_owned()]).unwrap();
        assert_eq!(readonly.collections, vec!["fleet-*".to_owned()]);
        assert!(readonly.actions.contains(&"objects:read".to_owned()));
        assert!(readonly.actions.contains(&"queries:read".to_owned()));

        let metrics = permission_for_preset(TokenPreset::Metrics, &[]).unwrap();
        assert_eq!(metrics.collections, vec!["*".to_owned()]);
        assert_eq!(metrics.actions, vec!["metrics:read".to_owned()]);
        assert!(permission_for_preset(TokenPreset::Writer, &[]).is_err());
    }

    #[test]
    fn geojson_geofence_definition_uses_properties_and_overrides() {
        let path =
            std::env::temp_dir().join(format!("latlng-cli-geofence-{}.json", std::process::id()));
        fs::write(
            &path,
            r#"{
              "type": "Feature",
              "properties": {
                "collection": "fleet",
                "detect": ["enter"],
                "commands": ["set"]
              },
              "geometry": {
                "type": "Polygon",
                "coordinates": [[[13.0,52.0],[14.0,52.0],[14.0,53.0],[13.0,53.0],[13.0,52.0]]]
              }
            }"#,
        )
        .unwrap();
        let def = geofence_def_from_geojson(&path, None, None, Vec::new(), Vec::new()).unwrap();
        fs::remove_file(path).ok();
        assert_eq!(def["collection"], "fleet");
        assert!(def["query"]["Within"].is_object());
        assert_eq!(def["detect"], serde_json::json!(["Enter"]));
        assert_eq!(def["commands"], serde_json::json!(["Set"]));
    }
}
