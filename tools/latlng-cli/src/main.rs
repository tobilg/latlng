#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
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
    }

    Ok(())
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
    use super::{SUPPORTED_COMMANDS, geofence_def_from_geojson, object_path, wildcard_path};
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
