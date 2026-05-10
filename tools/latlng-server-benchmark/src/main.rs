#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::future::Future;
use std::io;
use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use capnp::message::ReaderOptions;
use capnp_rpc::{RpcSystem, rpc_twoparty_capnp, twoparty};
use chrono::Utc;
use latlng_config::{RuntimeConfig, StorageMode, save_to_path};
use latlng_schema::latlng_capnp as capnp_api;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use uuid::Uuid;

type DynError = Box<dyn std::error::Error + Send + Sync>;

const DEFAULT_TOKEN: &str = "secret";
const DEFAULT_WARMUP_SECS: u64 = 2;
const DEFAULT_MEASURE_SECS: u64 = 5;
const DEFAULT_SEED_OBJECTS: usize = 10_000;
const DEFAULT_STARTUP_RECORDS: usize = 10_000;
const DEFAULT_STARTUP_WARMUP_SECS: u64 = 0;
const DEFAULT_STARTUP_MEASURE_SECS: u64 = 0;
const DEFAULT_FENCED_RELATED_CHANNELS: usize = 8;
const DEFAULT_FENCED_UNRELATED_CHANNELS: usize = 256;
const DEFAULT_TILE38_SERVER_BIN: &str = "tile38-server";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), DynError> {
    let cli = Cli::parse(env::args().skip(1).collect())?;
    match cli {
        Cli::Run(args) => run_command(args).await,
        Cli::Compare(args) => compare_command(args),
    }
}

enum Cli {
    Run(RunArgs),
    Compare(CompareArgs),
}

struct RunArgs {
    engine: BenchmarkEngine,
    latlng_transport: LatLngTransport,
    preset: BenchmarkPreset,
    scenario_selection: ScenarioSelection,
    profile_selection: ProfileSelection,
    concurrency_list: Option<Vec<usize>>,
    warmup_secs: u64,
    measure_secs: u64,
    seed_objects: usize,
    startup_records: usize,
    output: Option<PathBuf>,
    server_bin: Option<PathBuf>,
    tile38_server_bin: PathBuf,
    tile38_appendonly: Tile38AppendOnly,
    aof_overrides: AofOverrideArgs,
}

struct CompareArgs {
    baseline: PathBuf,
    candidate: PathBuf,
    output: Option<PathBuf>,
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Self, DynError> {
        let Some(command) = args.first().map(String::as_str) else {
            print_help();
            return Err(err("missing command"));
        };

        match command {
            "run" => Ok(Self::Run(RunArgs::parse(&args[1..])?)),
            "compare" => Ok(Self::Compare(CompareArgs::parse(&args[1..])?)),
            "--help" | "-h" | "help" => {
                print_help();
                Err(err("help requested"))
            }
            other => Err(err(format!("unknown command: {other}"))),
        }
    }
}

impl RunArgs {
    fn parse(args: &[String]) -> Result<Self, DynError> {
        let mut engine = BenchmarkEngine::LatLng;
        let mut latlng_transport = LatLngTransport::Http;
        let mut preset = BenchmarkPreset::Standard;
        let mut scenario_selection = ScenarioSelection::All;
        let mut profile_selection = ProfileSelection::Memory;
        let mut concurrency_list = None;
        let mut warmup_secs = DEFAULT_WARMUP_SECS;
        let mut measure_secs = DEFAULT_MEASURE_SECS;
        let mut seed_objects = DEFAULT_SEED_OBJECTS;
        let mut startup_records = DEFAULT_STARTUP_RECORDS;
        let mut output = None;
        let mut server_bin = None;
        let mut tile38_server_bin = PathBuf::from(DEFAULT_TILE38_SERVER_BIN);
        let mut tile38_appendonly = Tile38AppendOnly::No;
        let mut aof_overrides = AofOverrideArgs::default();

        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--engine" => {
                    engine = BenchmarkEngine::parse(expect_value(args, &mut index, "--engine")?)?;
                }
                "--latlng-transport" => {
                    latlng_transport = LatLngTransport::parse(expect_value(
                        args,
                        &mut index,
                        "--latlng-transport",
                    )?)?;
                }
                "--preset" => {
                    preset = BenchmarkPreset::parse(expect_value(args, &mut index, "--preset")?)?;
                }
                "--scenario" => {
                    scenario_selection =
                        ScenarioSelection::parse(expect_value(args, &mut index, "--scenario")?)?;
                }
                "--profile" => {
                    profile_selection =
                        ProfileSelection::parse(expect_value(args, &mut index, "--profile")?)?;
                }
                "--concurrency" => {
                    let value = expect_value(args, &mut index, "--concurrency")?;
                    let parsed = value
                        .parse::<usize>()
                        .map_err(|_| err(format!("invalid concurrency value: {value}")))?;
                    concurrency_list = Some(vec![parsed.max(1)]);
                }
                "--concurrency-list" => {
                    let value = expect_value(args, &mut index, "--concurrency-list")?;
                    concurrency_list = Some(parse_concurrency_list(value)?);
                }
                "--warmup-secs" => {
                    warmup_secs = parse_u64_flag(
                        expect_value(args, &mut index, "--warmup-secs")?,
                        "--warmup-secs",
                    )?;
                }
                "--measure-secs" => {
                    measure_secs = parse_u64_flag(
                        expect_value(args, &mut index, "--measure-secs")?,
                        "--measure-secs",
                    )?;
                }
                "--seed-objects" => {
                    seed_objects = parse_usize_flag(
                        expect_value(args, &mut index, "--seed-objects")?,
                        "--seed-objects",
                    )?;
                }
                "--startup-records" => {
                    startup_records = parse_usize_flag(
                        expect_value(args, &mut index, "--startup-records")?,
                        "--startup-records",
                    )?;
                }
                "--output" => {
                    output = Some(PathBuf::from(expect_value(args, &mut index, "--output")?));
                }
                "--server-bin" => {
                    server_bin = Some(PathBuf::from(expect_value(
                        args,
                        &mut index,
                        "--server-bin",
                    )?));
                }
                "--tile38-server-bin" => {
                    tile38_server_bin =
                        PathBuf::from(expect_value(args, &mut index, "--tile38-server-bin")?);
                }
                "--tile38-appendonly" => {
                    tile38_appendonly = Tile38AppendOnly::parse(expect_value(
                        args,
                        &mut index,
                        "--tile38-appendonly",
                    )?)?;
                }
                "--aof-writer-queue-limit" => {
                    aof_overrides.writer_queue_limit = Some(parse_usize_flag(
                        expect_value(args, &mut index, "--aof-writer-queue-limit")?,
                        "--aof-writer-queue-limit",
                    )?);
                }
                "--aof-group-commit-delay-ms" => {
                    aof_overrides.group_commit_delay_ms = Some(parse_u64_flag(
                        expect_value(args, &mut index, "--aof-group-commit-delay-ms")?,
                        "--aof-group-commit-delay-ms",
                    )?);
                }
                "--aof-group-commit-max-requests" => {
                    aof_overrides.group_commit_max_requests = Some(parse_usize_flag(
                        expect_value(args, &mut index, "--aof-group-commit-max-requests")?,
                        "--aof-group-commit-max-requests",
                    )?);
                }
                "--help" | "-h" => {
                    print_help();
                    return Err(err("help requested"));
                }
                other => return Err(err(format!("unknown flag: {other}"))),
            }
            index += 1;
        }

        Ok(Self {
            engine,
            latlng_transport,
            preset,
            scenario_selection,
            profile_selection,
            concurrency_list,
            warmup_secs,
            measure_secs,
            seed_objects,
            startup_records,
            output,
            server_bin,
            tile38_server_bin,
            tile38_appendonly,
            aof_overrides,
        })
    }
}

impl CompareArgs {
    fn parse(args: &[String]) -> Result<Self, DynError> {
        if args.is_empty() {
            print_help();
            return Err(err("compare requires baseline and candidate JSON files"));
        }
        let mut positional = Vec::new();
        let mut output = None;
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--output" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| err("missing value for --output"))?;
                    output = Some(PathBuf::from(value));
                }
                "--help" | "-h" => {
                    print_help();
                    return Err(err("help requested"));
                }
                value => positional.push(value.to_owned()),
            }
            index += 1;
        }

        if positional.len() != 2 {
            return Err(err("compare requires exactly two JSON files"));
        }

        Ok(Self {
            baseline: PathBuf::from(&positional[0]),
            candidate: PathBuf::from(&positional[1]),
            output,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BenchmarkEngine {
    LatLng,
    Tile38,
}

impl BenchmarkEngine {
    fn parse(value: &str) -> Result<Self, DynError> {
        match value {
            "latlng" => Ok(Self::LatLng),
            "tile38" => Ok(Self::Tile38),
            other => Err(err(format!("unknown engine: {other}"))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::LatLng => "latlng",
            Self::Tile38 => "tile38",
        }
    }
}

fn default_engine() -> BenchmarkEngine {
    BenchmarkEngine::LatLng
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LatLngTransport {
    Http,
    Capnp,
    Resp,
}

impl LatLngTransport {
    fn parse(value: &str) -> Result<Self, DynError> {
        match value {
            "http" => Ok(Self::Http),
            "capnp" => Ok(Self::Capnp),
            other => Err(err(format!("unknown latlng transport: {other}"))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Capnp => "capnp",
            Self::Resp => "resp",
        }
    }
}

fn default_transport() -> LatLngTransport {
    LatLngTransport::Http
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tile38AppendOnly {
    Yes,
    No,
}

impl Tile38AppendOnly {
    fn parse(value: &str) -> Result<Self, DynError> {
        match value {
            "yes" => Ok(Self::Yes),
            "no" => Ok(Self::No),
            other => Err(err(format!("unknown Tile38 appendonly value: {other}"))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum StorageProfile {
    Memory,
    Aof,
}

impl StorageProfile {
    fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Aof => "aof",
        }
    }
}

enum ProfileSelection {
    Memory,
    Aof,
    Both,
}

impl ProfileSelection {
    fn parse(value: &str) -> Result<Self, DynError> {
        match value {
            "memory" => Ok(Self::Memory),
            "aof" => Ok(Self::Aof),
            "both" => Ok(Self::Both),
            other => Err(err(format!("unknown profile: {other}"))),
        }
    }

    fn expand(&self) -> Vec<StorageProfile> {
        match self {
            Self::Memory => vec![StorageProfile::Memory],
            Self::Aof => vec![StorageProfile::Aof],
            Self::Both => vec![StorageProfile::Memory, StorageProfile::Aof],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchmarkPreset {
    Standard,
    WriteHeavy,
    QueryHeavy,
    AofTuning,
    GeofenceHeavy,
}

impl BenchmarkPreset {
    fn parse(value: &str) -> Result<Self, DynError> {
        match value {
            "standard" => Ok(Self::Standard),
            "write-heavy" => Ok(Self::WriteHeavy),
            "query-heavy" => Ok(Self::QueryHeavy),
            "aof-tuning" => Ok(Self::AofTuning),
            "geofence-heavy" => Ok(Self::GeofenceHeavy),
            other => Err(err(format!("unknown preset: {other}"))),
        }
    }

    fn scenarios(self, explicit: &ScenarioSelection) -> Vec<ScenarioKind> {
        if !matches!(explicit, ScenarioSelection::All) {
            return explicit.expand();
        }
        match self {
            Self::Standard => explicit.expand(),
            Self::WriteHeavy => vec![ScenarioKind::SetPointWrite, ScenarioKind::MixedReadWrite],
            Self::QueryHeavy => vec![
                ScenarioKind::NearbyQuery,
                ScenarioKind::WithinQuery,
                ScenarioKind::IntersectsQuery,
                ScenarioKind::ScanQuery,
                ScenarioKind::SearchQuery,
            ],
            Self::AofTuning => vec![ScenarioKind::SetPointWrite, ScenarioKind::MixedReadWrite],
            Self::GeofenceHeavy => vec![ScenarioKind::FencedSetPointWrite],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
enum ScenarioKind {
    SetPointWrite,
    FencedSetPointWrite,
    GetObjectRead,
    NearbyQuery,
    WithinQuery,
    IntersectsQuery,
    ScanQuery,
    SearchQuery,
    MixedReadWrite,
    StartupReplay,
}

impl ScenarioKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SetPointWrite => "set_point_write",
            Self::FencedSetPointWrite => "fenced_set_point_write",
            Self::GetObjectRead => "get_object_read",
            Self::NearbyQuery => "nearby_query",
            Self::WithinQuery => "within_query",
            Self::IntersectsQuery => "intersects_query",
            Self::ScanQuery => "scan_query",
            Self::SearchQuery => "search_query",
            Self::MixedReadWrite => "mixed_read_write",
            Self::StartupReplay => "startup_replay",
        }
    }

    fn parse(value: &str) -> Result<Self, DynError> {
        match value {
            "set_point_write" => Ok(Self::SetPointWrite),
            "fenced_set_point_write" => Ok(Self::FencedSetPointWrite),
            "get_object_read" => Ok(Self::GetObjectRead),
            "nearby_query" => Ok(Self::NearbyQuery),
            "within_query" => Ok(Self::WithinQuery),
            "intersects_query" => Ok(Self::IntersectsQuery),
            "scan_query" => Ok(Self::ScanQuery),
            "search_query" => Ok(Self::SearchQuery),
            "mixed_read_write" => Ok(Self::MixedReadWrite),
            "startup_replay" => Ok(Self::StartupReplay),
            other => Err(err(format!("unknown scenario: {other}"))),
        }
    }

    fn default_concurrency(self) -> Vec<usize> {
        match self {
            Self::MixedReadWrite | Self::FencedSetPointWrite => vec![8, 32],
            Self::StartupReplay => Vec::new(),
            _ => vec![1, 8, 32],
        }
    }

    fn all() -> Vec<Self> {
        vec![
            Self::SetPointWrite,
            Self::FencedSetPointWrite,
            Self::GetObjectRead,
            Self::NearbyQuery,
            Self::WithinQuery,
            Self::IntersectsQuery,
            Self::ScanQuery,
            Self::SearchQuery,
            Self::MixedReadWrite,
            Self::StartupReplay,
        ]
    }
}

enum ScenarioSelection {
    All,
    One(ScenarioKind),
}

impl ScenarioSelection {
    fn parse(value: &str) -> Result<Self, DynError> {
        if value == "all" {
            return Ok(Self::All);
        }
        Ok(Self::One(ScenarioKind::parse(value)?))
    }

    fn expand(&self) -> Vec<ScenarioKind> {
        match self {
            Self::All => ScenarioKind::all(),
            Self::One(kind) => vec![*kind],
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AofOverrideArgs {
    writer_queue_limit: Option<usize>,
    group_commit_delay_ms: Option<u64>,
    group_commit_max_requests: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkFile {
    schema_version: u32,
    tool_version: String,
    generated_at: String,
    git_commit: Option<String>,
    results: Vec<BenchmarkResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkResult {
    #[serde(default = "default_engine")]
    engine: BenchmarkEngine,
    #[serde(default = "default_transport")]
    transport: LatLngTransport,
    scenario: String,
    profile: String,
    concurrency: Option<usize>,
    warmup_secs: u64,
    measure_secs: u64,
    seed_objects: usize,
    steady_state: Option<SteadyStateMetrics>,
    startup: Option<StartupMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SteadyStateMetrics {
    request_count: u64,
    error_count: u64,
    ops_per_sec: f64,
    latency_ms: LatencyStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatencyStats {
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartupMetrics {
    replay_duration_ms: f64,
    record_count: usize,
    log_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComparisonFile {
    schema_version: u32,
    generated_at: String,
    baseline_file: String,
    candidate_file: String,
    entries: Vec<ComparisonEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComparisonEntry {
    scenario: String,
    profile: String,
    concurrency: Option<usize>,
    baseline_engine: String,
    candidate_engine: String,
    baseline_transport: String,
    candidate_transport: String,
    steady_state: Option<SteadyStateComparison>,
    startup: Option<StartupComparison>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SteadyStateComparison {
    baseline_ops_per_sec: f64,
    candidate_ops_per_sec: f64,
    ops_per_sec_delta: f64,
    ops_per_sec_delta_pct: Option<f64>,
    baseline_p95_ms: f64,
    candidate_p95_ms: f64,
    p95_delta_ms: f64,
    p95_delta_pct: Option<f64>,
    baseline_p99_ms: f64,
    candidate_p99_ms: f64,
    p99_delta_ms: f64,
    p99_delta_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartupComparison {
    baseline_replay_ms: f64,
    candidate_replay_ms: f64,
    replay_delta_ms: f64,
    replay_delta_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ScenarioKey {
    scenario: String,
    profile: String,
    concurrency: Option<usize>,
}

struct BenchmarkHarness {
    repo_root: PathBuf,
    server_bin: Option<PathBuf>,
    tile38_server_bin: PathBuf,
}

struct StartedServer {
    base_url: String,
    capnp_addr: String,
    log_path: PathBuf,
    aof_path: Option<PathBuf>,
    _root_dir: TempDir,
    child: Child,
}

struct StartedTile38Server {
    port: u16,
    log_path: PathBuf,
    _root_dir: TempDir,
    child: Child,
}

struct ScenarioContext {
    client: reqwest::Client,
    base_url: String,
    seed_objects: usize,
}

struct CapnpScenarioContext {
    addr: String,
    seed_objects: usize,
}

struct Tile38ScenarioContext {
    port: u16,
    seed_objects: usize,
}

#[derive(Debug, Clone, Copy)]
struct ServerPorts {
    http: u16,
    capnp: u16,
}

#[derive(Debug)]
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create(prefix: &str) -> Result<Self, DynError> {
        let path = env::temp_dir().join(format!(
            "latlng-{prefix}-{}-{}",
            Uuid::new_v4(),
            std::process::id()
        ));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl BenchmarkHarness {
    fn discover(
        engine: BenchmarkEngine,
        server_bin: Option<PathBuf>,
        tile38_server_bin: PathBuf,
    ) -> Result<Self, DynError> {
        let repo_root = repo_root()?;
        let server_bin = if engine == BenchmarkEngine::LatLng {
            let server_bin = match server_bin {
                Some(path) => path,
                None => default_server_binary(&repo_root)?,
            };
            if !server_bin.exists() {
                return Err(err(format!(
                    "latlng-server binary not found at {}",
                    server_bin.display()
                )));
            }
            Some(server_bin)
        } else {
            server_bin
        };
        Ok(Self {
            repo_root,
            server_bin,
            tile38_server_bin,
        })
    }

    async fn start_server(
        &self,
        profile: StorageProfile,
        aof_overrides: AofOverrideArgs,
        transport: LatLngTransport,
    ) -> Result<StartedServer, DynError> {
        if transport == LatLngTransport::Resp {
            return Err(err("resp transport is only used for Tile38"));
        }

        let server_bin = self
            .server_bin
            .as_ref()
            .ok_or_else(|| err("latlng-server binary is not configured"))?;
        let root_dir = TempDir::create("server-bench")?;
        let ports = ServerPorts {
            http: free_port()?,
            capnp: free_port()?,
        };
        let config_path = root_dir.path().join("latlng-bench.json");
        let log_path = root_dir.path().join("server.log");
        let aof_path = root_dir.path().join("appendonly.aof");
        let webhook_queue_path = root_dir.path().join("webhook-queue.sqlite");

        write_config(
            &config_path,
            profile,
            ports,
            &aof_path,
            &webhook_queue_path,
            aof_overrides,
            transport == LatLngTransport::Capnp,
        )?;

        let stdout = File::create(&log_path)?;
        let stderr = stdout.try_clone()?;
        let mut command = Command::new(server_bin);
        command
            .current_dir(&self.repo_root)
            .kill_on_drop(true)
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = command.spawn()?;

        let mut started = StartedServer {
            base_url: format!("http://127.0.0.1:{}", ports.http),
            capnp_addr: format!("127.0.0.1:{}", ports.capnp),
            log_path,
            aof_path: matches!(profile, StorageProfile::Aof).then_some(aof_path),
            _root_dir: root_dir,
            child,
        };
        match transport {
            LatLngTransport::Http => started.wait_for_ping().await?,
            LatLngTransport::Capnp => started.wait_for_capnp().await?,
            LatLngTransport::Resp => unreachable!("resp transport rejected above"),
        }
        Ok(started)
    }

    async fn start_server_from_config(
        &self,
        config_path: &Path,
    ) -> Result<StartedServer, DynError> {
        let server_bin = self
            .server_bin
            .as_ref()
            .ok_or_else(|| err("latlng-server binary is not configured"))?;
        let root_dir = TempDir::create("server-bench-restart")?;
        let log_path = root_dir.path().join("server.log");
        let stdout = File::create(&log_path)?;
        let stderr = stdout.try_clone()?;
        let mut command = Command::new(server_bin);
        command
            .current_dir(&self.repo_root)
            .kill_on_drop(true)
            .arg("--config")
            .arg(config_path)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = command.spawn()?;

        let mut started = StartedServer {
            base_url: read_base_url_from_config(config_path)?,
            capnp_addr: read_capnp_addr_from_config(config_path)?,
            log_path,
            aof_path: read_aof_path_from_config(config_path)?,
            _root_dir: root_dir,
            child,
        };
        started.wait_for_ping().await?;
        Ok(started)
    }

    async fn start_tile38_server(
        &self,
        appendonly: Tile38AppendOnly,
    ) -> Result<StartedTile38Server, DynError> {
        let root_dir = TempDir::create("tile38-bench")?;
        let port = free_port()?;
        let log_path = root_dir.path().join("tile38.log");
        let stdout = File::create(&log_path)?;
        let stderr = stdout.try_clone()?;
        let mut command = Command::new(&self.tile38_server_bin);
        command
            .kill_on_drop(true)
            .arg("-h")
            .arg("127.0.0.1")
            .arg("-p")
            .arg(port.to_string())
            .arg("-d")
            .arg(root_dir.path())
            .arg("--appendonly")
            .arg(appendonly.as_str())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = command.spawn().map_err(|error| {
            err(format!(
                "failed to start Tile38 server via {}: {error}",
                self.tile38_server_bin.display()
            ))
        })?;

        let mut started = StartedTile38Server {
            port,
            log_path,
            _root_dir: root_dir,
            child,
        };
        started.wait_for_ping().await?;
        Ok(started)
    }
}

impl StartedServer {
    async fn wait_for_ping(&mut self) -> Result<(), DynError> {
        let client = build_client()?;
        for _ in 0..240 {
            if let Some(status) = self.child.try_wait()? {
                let logs = fs::read_to_string(&self.log_path).unwrap_or_default();
                return Err(err(format!(
                    "server exited early with status {status}: {}",
                    trim_logs(&logs)
                )));
            }

            match client.get(format!("{}/ping", self.base_url)).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(_) | Err(_) => sleep(Duration::from_millis(50)).await,
            }
        }

        let logs = fs::read_to_string(&self.log_path).unwrap_or_default();
        Err(err(format!(
            "server at {} did not become ready: {}",
            self.base_url,
            trim_logs(&logs)
        )))
    }

    async fn wait_for_capnp(&mut self) -> Result<(), DynError> {
        let addr = self.capnp_addr.clone();
        let log_path = self.log_path.clone();
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                for _ in 0..240 {
                    if let Some(status) = self.child.try_wait()? {
                        let logs = fs::read_to_string(&log_path).unwrap_or_default();
                        return Err(err(format!(
                            "server exited early with status {status}: {}",
                            trim_logs(&logs)
                        )));
                    }

                    match CapnpBenchmarkClient::connect_once(&addr).await {
                        Ok(_) => return Ok(()),
                        Err(_) => sleep(Duration::from_millis(50)).await,
                    }
                }

                let logs = fs::read_to_string(&log_path).unwrap_or_default();
                Err(err(format!(
                    "Cap'n Proto server at {addr} did not become ready: {}",
                    trim_logs(&logs)
                )))
            })
            .await
    }

    async fn stop(&mut self) -> Result<(), DynError> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status();
                for _ in 0..100 {
                    if self.child.try_wait()?.is_some() {
                        return Ok(());
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }

        self.child.kill().await?;
        let _ = self.child.wait().await?;
        Ok(())
    }
}

impl StartedTile38Server {
    async fn wait_for_ping(&mut self) -> Result<(), DynError> {
        for _ in 0..240 {
            if let Some(status) = self.child.try_wait()? {
                let logs = fs::read_to_string(&self.log_path).unwrap_or_default();
                return Err(err(format!(
                    "Tile38 server exited early with status {status}: {}",
                    trim_logs(&logs)
                )));
            }

            match Tile38Client::connect(self.port).await {
                Ok(mut client) => match client.command(vec!["PING".to_owned()]).await {
                    Ok(_) => return Ok(()),
                    Err(_) => sleep(Duration::from_millis(50)).await,
                },
                Err(_) => sleep(Duration::from_millis(50)).await,
            }
        }

        let logs = fs::read_to_string(&self.log_path).unwrap_or_default();
        Err(err(format!(
            "Tile38 server at 127.0.0.1:{} did not become ready: {}",
            self.port,
            trim_logs(&logs)
        )))
    }

    async fn stop(&mut self) -> Result<(), DynError> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status();
                for _ in 0..100 {
                    if self.child.try_wait()?.is_some() {
                        return Ok(());
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }

        self.child.kill().await?;
        let _ = self.child.wait().await?;
        Ok(())
    }
}

async fn run_command(args: RunArgs) -> Result<(), DynError> {
    let harness =
        BenchmarkHarness::discover(args.engine, args.server_bin, args.tile38_server_bin.clone())?;
    let now = Utc::now().to_rfc3339();
    let git_commit = git_commit();
    let mut results = Vec::new();
    let transport = match args.engine {
        BenchmarkEngine::LatLng => args.latlng_transport,
        BenchmarkEngine::Tile38 => LatLngTransport::Resp,
    };

    let scenarios = args.preset.scenarios(&args.scenario_selection);
    let profiles = args.profile_selection.expand();
    for profile in profiles {
        for scenario in &scenarios {
            if args.engine == BenchmarkEngine::Tile38 && *scenario == ScenarioKind::StartupReplay {
                println!(
                    "skipping scenario={} engine={}: startup replay is latlng-only",
                    scenario.as_str(),
                    args.engine.as_str()
                );
                continue;
            }
            if *scenario == ScenarioKind::StartupReplay && profile == StorageProfile::Memory {
                continue;
            }
            let concurrencies = if *scenario == ScenarioKind::StartupReplay {
                vec![0]
            } else {
                args.concurrency_list
                    .clone()
                    .unwrap_or_else(|| scenario.default_concurrency())
            };

            for concurrency in concurrencies {
                let result = match scenario {
                    ScenarioKind::StartupReplay => {
                        println!(
                            "running engine={} transport={} scenario={} profile={} records={}",
                            args.engine.as_str(),
                            transport.as_str(),
                            scenario.as_str(),
                            profile.as_str(),
                            args.startup_records
                        );
                        run_startup_replay(
                            &harness,
                            profile,
                            args.startup_records,
                            args.aof_overrides,
                        )
                        .await?
                    }
                    _ => {
                        println!(
                            "running engine={} transport={} scenario={} profile={} concurrency={} warmup={}s measure={}s",
                            args.engine.as_str(),
                            transport.as_str(),
                            scenario.as_str(),
                            profile.as_str(),
                            concurrency,
                            args.warmup_secs,
                            args.measure_secs
                        );
                        run_steady_state_scenario(SteadyStateScenarioConfig {
                            harness: &harness,
                            engine: args.engine,
                            transport,
                            profile,
                            scenario: *scenario,
                            concurrency,
                            warmup: Duration::from_secs(args.warmup_secs),
                            measure: Duration::from_secs(args.measure_secs),
                            seed_objects: args.seed_objects,
                            tile38_appendonly: args.tile38_appendonly,
                            aof_overrides: args.aof_overrides,
                        })
                        .await?
                    }
                };
                print_result(&result);
                results.push(result);
            }
        }
    }

    let file = BenchmarkFile {
        schema_version: 1,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        generated_at: now,
        git_commit,
        results,
    };
    if let Some(path) = args.output.as_ref() {
        write_json(path, &file)?;
        println!("wrote results to {}", path.display());
    }
    Ok(())
}

struct SteadyStateScenarioConfig<'a> {
    harness: &'a BenchmarkHarness,
    engine: BenchmarkEngine,
    transport: LatLngTransport,
    profile: StorageProfile,
    scenario: ScenarioKind,
    concurrency: usize,
    warmup: Duration,
    measure: Duration,
    seed_objects: usize,
    tile38_appendonly: Tile38AppendOnly,
    aof_overrides: AofOverrideArgs,
}

async fn run_steady_state_scenario(
    config: SteadyStateScenarioConfig<'_>,
) -> Result<BenchmarkResult, DynError> {
    match config.engine {
        BenchmarkEngine::LatLng => match config.transport {
            LatLngTransport::Http => run_latlng_steady_state_scenario(config).await,
            LatLngTransport::Capnp => run_latlng_capnp_steady_state_scenario(config).await,
            LatLngTransport::Resp => Err(err("resp transport is only used for Tile38")),
        },
        BenchmarkEngine::Tile38 => run_tile38_steady_state_scenario(config).await,
    }
}

async fn run_latlng_steady_state_scenario(
    config: SteadyStateScenarioConfig<'_>,
) -> Result<BenchmarkResult, DynError> {
    let mut server = config
        .harness
        .start_server(config.profile, config.aof_overrides, LatLngTransport::Http)
        .await?;
    let client = build_client()?;
    let context = ScenarioContext {
        client,
        base_url: server.base_url.clone(),
        seed_objects: config.seed_objects,
    };

    let prepare_result = prepare_scenario(&context, config.scenario, config.seed_objects).await;
    let run_result = match prepare_result {
        Ok(()) => {
            if config.warmup > Duration::ZERO {
                let _ =
                    run_phase(&context, config.scenario, config.concurrency, config.warmup).await?;
            }
            let steady_state = run_phase(
                &context,
                config.scenario,
                config.concurrency,
                config.measure,
            )
            .await?;
            Ok(BenchmarkResult {
                engine: BenchmarkEngine::LatLng,
                transport: LatLngTransport::Http,
                scenario: config.scenario.as_str().to_owned(),
                profile: config.profile.as_str().to_owned(),
                concurrency: Some(config.concurrency),
                warmup_secs: config.warmup.as_secs(),
                measure_secs: config.measure.as_secs(),
                seed_objects: config.seed_objects,
                steady_state: Some(steady_state),
                startup: None,
            })
        }
        Err(error) => Err(error),
    };
    let stop_result = server.stop().await;
    stop_result?;
    run_result
}

async fn run_latlng_capnp_steady_state_scenario(
    config: SteadyStateScenarioConfig<'_>,
) -> Result<BenchmarkResult, DynError> {
    let mut server = config
        .harness
        .start_server(config.profile, config.aof_overrides, LatLngTransport::Capnp)
        .await?;
    let context = CapnpScenarioContext {
        addr: server.capnp_addr.clone(),
        seed_objects: config.seed_objects,
    };

    let prepare_result =
        prepare_capnp_scenario(&context, config.scenario, config.seed_objects).await;
    let run_result = match prepare_result {
        Ok(()) => {
            if config.warmup > Duration::ZERO {
                let _ =
                    run_capnp_phase(&context, config.scenario, config.concurrency, config.warmup)
                        .await?;
            }
            let steady_state = run_capnp_phase(
                &context,
                config.scenario,
                config.concurrency,
                config.measure,
            )
            .await?;
            Ok(BenchmarkResult {
                engine: BenchmarkEngine::LatLng,
                transport: LatLngTransport::Capnp,
                scenario: config.scenario.as_str().to_owned(),
                profile: config.profile.as_str().to_owned(),
                concurrency: Some(config.concurrency),
                warmup_secs: config.warmup.as_secs(),
                measure_secs: config.measure.as_secs(),
                seed_objects: config.seed_objects,
                steady_state: Some(steady_state),
                startup: None,
            })
        }
        Err(error) => Err(error),
    };
    let stop_result = server.stop().await;
    stop_result?;
    run_result
}

async fn run_tile38_steady_state_scenario(
    config: SteadyStateScenarioConfig<'_>,
) -> Result<BenchmarkResult, DynError> {
    let mut server = config
        .harness
        .start_tile38_server(config.tile38_appendonly)
        .await?;
    let context = Tile38ScenarioContext {
        port: server.port,
        seed_objects: config.seed_objects,
    };

    let prepare_result =
        prepare_tile38_scenario(&context, config.scenario, config.seed_objects).await;
    let run_result = match prepare_result {
        Ok(()) => {
            if config.warmup > Duration::ZERO {
                let _ =
                    run_tile38_phase(&context, config.scenario, config.concurrency, config.warmup)
                        .await?;
            }
            let steady_state = run_tile38_phase(
                &context,
                config.scenario,
                config.concurrency,
                config.measure,
            )
            .await?;
            Ok(BenchmarkResult {
                engine: BenchmarkEngine::Tile38,
                transport: LatLngTransport::Resp,
                scenario: config.scenario.as_str().to_owned(),
                profile: config.profile.as_str().to_owned(),
                concurrency: Some(config.concurrency),
                warmup_secs: config.warmup.as_secs(),
                measure_secs: config.measure.as_secs(),
                seed_objects: config.seed_objects,
                steady_state: Some(steady_state),
                startup: None,
            })
        }
        Err(error) => Err(error),
    };
    let stop_result = server.stop().await;
    stop_result?;
    run_result
}

async fn run_startup_replay(
    harness: &BenchmarkHarness,
    profile: StorageProfile,
    startup_records: usize,
    aof_overrides: AofOverrideArgs,
) -> Result<BenchmarkResult, DynError> {
    if profile != StorageProfile::Aof {
        return Err(err("startup replay is only supported for the aof profile"));
    }

    let mut first = harness
        .start_server(StorageProfile::Aof, aof_overrides, LatLngTransport::Http)
        .await?;
    let client = build_client()?;
    let context = ScenarioContext {
        client,
        base_url: first.base_url.clone(),
        seed_objects: startup_records,
    };
    create_collection(&context.client, &context.base_url, "bench-replay").await?;
    for index in 0..startup_records {
        set_point(
            &context.client,
            &context.base_url,
            "bench-replay",
            &format!("point-{index}"),
            52.50 + (index % 1_000) as f64 * 0.00001,
            13.39 + (index % 500) as f64 * 0.00001,
            None,
        )
        .await?;
    }
    let aof_path = first
        .aof_path
        .clone()
        .ok_or_else(|| err("missing aof path for startup replay benchmark"))?;
    let config_path = first._root_dir.path().join("latlng-bench.json");
    first.stop().await?;

    let log_bytes = fs::metadata(&aof_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let start = Instant::now();
    let mut restarted = harness.start_server_from_config(&config_path).await?;
    let replay_duration = start.elapsed();
    restarted.stop().await?;

    Ok(BenchmarkResult {
        engine: BenchmarkEngine::LatLng,
        transport: LatLngTransport::Http,
        scenario: ScenarioKind::StartupReplay.as_str().to_owned(),
        profile: profile.as_str().to_owned(),
        concurrency: None,
        warmup_secs: DEFAULT_STARTUP_WARMUP_SECS,
        measure_secs: DEFAULT_STARTUP_MEASURE_SECS,
        seed_objects: startup_records,
        steady_state: None,
        startup: Some(StartupMetrics {
            replay_duration_ms: duration_ms(replay_duration),
            record_count: startup_records,
            log_bytes,
        }),
    })
}

async fn prepare_scenario(
    context: &ScenarioContext,
    scenario: ScenarioKind,
    seed_objects: usize,
) -> Result<(), DynError> {
    match scenario {
        ScenarioKind::SetPointWrite => Ok(()),
        ScenarioKind::FencedSetPointWrite => seed_fenced_channel_dataset(context).await,
        ScenarioKind::GetObjectRead
        | ScenarioKind::NearbyQuery
        | ScenarioKind::WithinQuery
        | ScenarioKind::IntersectsQuery
        | ScenarioKind::ScanQuery => seed_point_dataset(context, "bench-geo", seed_objects).await,
        ScenarioKind::SearchQuery => seed_text_dataset(context, "bench-text", seed_objects).await,
        ScenarioKind::MixedReadWrite => {
            seed_point_dataset(context, "bench-mixed-read", seed_objects).await
        }
        ScenarioKind::StartupReplay => Ok(()),
    }
}

async fn prepare_capnp_scenario(
    context: &CapnpScenarioContext,
    scenario: ScenarioKind,
    seed_objects: usize,
) -> Result<(), DynError> {
    match scenario {
        ScenarioKind::SetPointWrite => Ok(()),
        ScenarioKind::FencedSetPointWrite => capnp_seed_fenced_channel_dataset(context).await,
        ScenarioKind::GetObjectRead
        | ScenarioKind::NearbyQuery
        | ScenarioKind::WithinQuery
        | ScenarioKind::IntersectsQuery
        | ScenarioKind::ScanQuery => {
            capnp_seed_point_dataset(&context.addr, "bench-geo", seed_objects).await
        }
        ScenarioKind::SearchQuery => {
            capnp_seed_text_dataset(&context.addr, "bench-text", seed_objects).await
        }
        ScenarioKind::MixedReadWrite => {
            capnp_seed_point_dataset(&context.addr, "bench-mixed-read", seed_objects).await
        }
        ScenarioKind::StartupReplay => Err(err("startup replay is not a steady-state scenario")),
    }
}

async fn prepare_tile38_scenario(
    context: &Tile38ScenarioContext,
    scenario: ScenarioKind,
    seed_objects: usize,
) -> Result<(), DynError> {
    match scenario {
        ScenarioKind::SetPointWrite => Ok(()),
        ScenarioKind::FencedSetPointWrite => tile38_seed_fenced_channel_dataset(context).await,
        ScenarioKind::GetObjectRead
        | ScenarioKind::NearbyQuery
        | ScenarioKind::WithinQuery
        | ScenarioKind::IntersectsQuery
        | ScenarioKind::ScanQuery => {
            tile38_seed_point_dataset(context.port, "bench-geo", seed_objects).await
        }
        ScenarioKind::SearchQuery => {
            tile38_seed_text_dataset(context.port, "bench-text", seed_objects).await
        }
        ScenarioKind::MixedReadWrite => {
            tile38_seed_point_dataset(context.port, "bench-mixed-read", seed_objects).await
        }
        ScenarioKind::StartupReplay => Err(err("startup replay is latlng-only")),
    }
}

async fn run_phase(
    context: &ScenarioContext,
    scenario: ScenarioKind,
    concurrency: usize,
    duration: Duration,
) -> Result<SteadyStateMetrics, DynError> {
    let latencies = Arc::new(Mutex::new(Vec::<u64>::new()));
    let request_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let counter = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + duration;
    let mut workers = JoinSet::new();

    for _ in 0..concurrency.max(1) {
        let latencies = Arc::clone(&latencies);
        let request_count = Arc::clone(&request_count);
        let error_count = Arc::clone(&error_count);
        let counter = Arc::clone(&counter);
        let client = context.client.clone();
        let base_url = context.base_url.clone();
        let seed_objects = context.seed_objects;
        workers.spawn(async move {
            let local_context = ScenarioContext {
                client,
                base_url,
                seed_objects,
            };
            loop {
                if Instant::now() >= deadline {
                    break;
                }
                let iteration = counter.fetch_add(1, Ordering::Relaxed);
                let started = Instant::now();
                let result = execute_iteration(&local_context, scenario, iteration).await;
                let elapsed = started.elapsed();
                request_count.fetch_add(1, Ordering::Relaxed);
                if result.is_err() {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
                latencies
                    .lock()
                    .expect("latency mutex poisoned")
                    .push(duration_micros(elapsed));
            }
            Ok::<(), DynError>(())
        });
    }

    while let Some(result) = workers.join_next().await {
        result??;
    }

    let mut latencies = latencies.lock().expect("latency mutex poisoned").clone();
    latencies.sort_unstable();
    let total_requests = request_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let successful = total_requests.saturating_sub(errors);
    let ops_per_sec = if duration.as_secs_f64() > 0.0 {
        successful as f64 / duration.as_secs_f64()
    } else {
        successful as f64
    };

    Ok(SteadyStateMetrics {
        request_count: total_requests,
        error_count: errors,
        ops_per_sec,
        latency_ms: LatencyStats {
            mean: if latencies.is_empty() {
                0.0
            } else {
                latencies.iter().sum::<u64>() as f64 / latencies.len() as f64 / 1_000.0
            },
            p50: percentile_ms(&latencies, 0.50),
            p95: percentile_ms(&latencies, 0.95),
            p99: percentile_ms(&latencies, 0.99),
        },
    })
}

async fn run_capnp_phase(
    context: &CapnpScenarioContext,
    scenario: ScenarioKind,
    concurrency: usize,
    duration: Duration,
) -> Result<SteadyStateMetrics, DynError> {
    let latencies = Arc::new(Mutex::new(Vec::<u64>::new()));
    let request_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let counter = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + duration;
    let local = tokio::task::LocalSet::new();

    local
        .run_until(async {
            let mut workers = Vec::new();
            for _ in 0..concurrency.max(1) {
                let latencies = Arc::clone(&latencies);
                let request_count = Arc::clone(&request_count);
                let error_count = Arc::clone(&error_count);
                let counter = Arc::clone(&counter);
                let addr = context.addr.clone();
                let seed_objects = context.seed_objects;
                workers.push(tokio::task::spawn_local(async move {
                    let mut client = CapnpBenchmarkClient::connect(&addr).await?;
                    loop {
                        if Instant::now() >= deadline {
                            break;
                        }
                        let iteration = counter.fetch_add(1, Ordering::Relaxed);
                        let started = Instant::now();
                        let result =
                            execute_capnp_iteration(&mut client, scenario, iteration, seed_objects)
                                .await;
                        let elapsed = started.elapsed();
                        request_count.fetch_add(1, Ordering::Relaxed);
                        if result.is_err() {
                            error_count.fetch_add(1, Ordering::Relaxed);
                        }
                        latencies
                            .lock()
                            .expect("latency mutex poisoned")
                            .push(duration_micros(elapsed));
                    }
                    Ok::<(), DynError>(())
                }));
            }

            for worker in workers {
                worker.await??;
            }
            Ok::<(), DynError>(())
        })
        .await?;

    let mut latencies = latencies.lock().expect("latency mutex poisoned").clone();
    latencies.sort_unstable();
    let total_requests = request_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let successful = total_requests.saturating_sub(errors);
    let ops_per_sec = if duration.as_secs_f64() > 0.0 {
        successful as f64 / duration.as_secs_f64()
    } else {
        successful as f64
    };

    Ok(SteadyStateMetrics {
        request_count: total_requests,
        error_count: errors,
        ops_per_sec,
        latency_ms: LatencyStats {
            mean: if latencies.is_empty() {
                0.0
            } else {
                latencies.iter().sum::<u64>() as f64 / latencies.len() as f64 / 1_000.0
            },
            p50: percentile_ms(&latencies, 0.50),
            p95: percentile_ms(&latencies, 0.95),
            p99: percentile_ms(&latencies, 0.99),
        },
    })
}

async fn run_tile38_phase(
    context: &Tile38ScenarioContext,
    scenario: ScenarioKind,
    concurrency: usize,
    duration: Duration,
) -> Result<SteadyStateMetrics, DynError> {
    let latencies = Arc::new(Mutex::new(Vec::<u64>::new()));
    let request_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let counter = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + duration;
    let mut workers = JoinSet::new();

    for _ in 0..concurrency.max(1) {
        let latencies = Arc::clone(&latencies);
        let request_count = Arc::clone(&request_count);
        let error_count = Arc::clone(&error_count);
        let counter = Arc::clone(&counter);
        let port = context.port;
        let seed_objects = context.seed_objects;
        workers.spawn(async move {
            let mut client = Tile38Client::connect(port).await?;
            loop {
                if Instant::now() >= deadline {
                    break;
                }
                let iteration = counter.fetch_add(1, Ordering::Relaxed);
                let started = Instant::now();
                let result =
                    execute_tile38_iteration(&mut client, scenario, iteration, seed_objects).await;
                let elapsed = started.elapsed();
                request_count.fetch_add(1, Ordering::Relaxed);
                if result.is_err() {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
                latencies
                    .lock()
                    .expect("latency mutex poisoned")
                    .push(duration_micros(elapsed));
            }
            Ok::<(), DynError>(())
        });
    }

    while let Some(result) = workers.join_next().await {
        result??;
    }

    let mut latencies = latencies.lock().expect("latency mutex poisoned").clone();
    latencies.sort_unstable();
    let total_requests = request_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let successful = total_requests.saturating_sub(errors);
    let ops_per_sec = if duration.as_secs_f64() > 0.0 {
        successful as f64 / duration.as_secs_f64()
    } else {
        successful as f64
    };

    Ok(SteadyStateMetrics {
        request_count: total_requests,
        error_count: errors,
        ops_per_sec,
        latency_ms: LatencyStats {
            mean: if latencies.is_empty() {
                0.0
            } else {
                latencies.iter().sum::<u64>() as f64 / latencies.len() as f64 / 1_000.0
            },
            p50: percentile_ms(&latencies, 0.50),
            p95: percentile_ms(&latencies, 0.95),
            p99: percentile_ms(&latencies, 0.99),
        },
    })
}

async fn execute_iteration(
    context: &ScenarioContext,
    scenario: ScenarioKind,
    iteration: u64,
) -> Result<(), DynError> {
    match scenario {
        ScenarioKind::SetPointWrite => {
            let id = format!("point-{iteration}");
            set_point(
                &context.client,
                &context.base_url,
                "bench-write",
                &id,
                52.52 + (iteration % 500) as f64 * 0.00001,
                13.405 + (iteration % 250) as f64 * 0.00001,
                Some(point_fields(iteration as usize)),
            )
            .await
        }
        ScenarioKind::FencedSetPointWrite => {
            let id = format!("fenced-{iteration}");
            set_point(
                &context.client,
                &context.base_url,
                "bench-fenced",
                &id,
                52.52 + (iteration % 500) as f64 * 0.00001,
                13.405 + (iteration % 250) as f64 * 0.00001,
                Some(point_fields(iteration as usize)),
            )
            .await
        }
        ScenarioKind::GetObjectRead => {
            let index = iteration as usize % context.seed_objects.max(1);
            get_object(
                &context.client,
                &context.base_url,
                "bench-geo",
                &format!("point-{index}"),
            )
            .await
        }
        ScenarioKind::NearbyQuery => {
            post_json_ok(
                &context.client,
                &format!("{}/collections/bench-geo/search/nearby", context.base_url),
                serde_json::json!({
                    "lat": 52.52,
                    "lon": 13.405,
                    "meters": 2_000.0,
                    "options": {
                        "limit": 100,
                        "nofields": true,
                        "include_count": false,
                        "output": "Ids"
                    }
                }),
            )
            .await
        }
        ScenarioKind::WithinQuery => {
            post_json_ok(
                &context.client,
                &format!("{}/collections/bench-geo/search/within", context.base_url),
                serde_json::json!({
                    "area": {
                        "Bounds": {
                            "min_lat": 52.50,
                            "min_lon": 13.39,
                            "max_lat": 52.56,
                            "max_lon": 13.45
                        }
                    },
                    "options": {
                        "limit": 100,
                        "nofields": true,
                        "include_count": false,
                        "output": "Ids"
                    }
                }),
            )
            .await
        }
        ScenarioKind::IntersectsQuery => {
            post_json_ok(
                &context.client,
                &format!(
                    "{}/collections/bench-geo/search/intersects",
                    context.base_url
                ),
                serde_json::json!({
                    "area": {
                        "Bounds": {
                            "min_lat": 52.50,
                            "min_lon": 13.39,
                            "max_lat": 52.56,
                            "max_lon": 13.45
                        }
                    },
                    "options": {
                        "limit": 100,
                        "nofields": true,
                        "include_count": false,
                        "output": "Ids"
                    }
                }),
            )
            .await
        }
        ScenarioKind::ScanQuery => {
            post_json_ok(
                &context.client,
                &format!("{}/collections/bench-geo/search/scan", context.base_url),
                serde_json::json!({
                    "limit": 100,
                    "nofields": true,
                    "include_count": false,
                    "output": "Ids",
                    "where_filters": [{
                        "field": "speed",
                        "comparison": {
                            "Range": {
                                "min": 32.0,
                                "max": 96.0
                            }
                        }
                    }]
                }),
            )
            .await
        }
        ScenarioKind::SearchQuery => {
            post_json_ok(
                &context.client,
                &format!("{}/collections/bench-text/search/text", context.base_url),
                serde_json::json!({
                    "limit": 100,
                    "nofields": true,
                    "include_count": false,
                    "output": "Ids",
                    "match_pattern": "msg-*",
                    "where_filters": [{
                        "field": "tag_code",
                        "comparison": {
                            "Range": {
                                "min": 1.0,
                                "max": 1.0
                            }
                        }
                    }]
                }),
            )
            .await
        }
        ScenarioKind::MixedReadWrite => {
            if iteration.is_multiple_of(2) {
                let index = (iteration as usize / 2) % context.seed_objects.max(1);
                get_object(
                    &context.client,
                    &context.base_url,
                    "bench-mixed-read",
                    &format!("point-{index}"),
                )
                .await
            } else {
                let id = format!("mixed-{iteration}");
                set_point(
                    &context.client,
                    &context.base_url,
                    "bench-mixed-write",
                    &id,
                    52.52 + (iteration % 500) as f64 * 0.00001,
                    13.405 + (iteration % 250) as f64 * 0.00001,
                    Some(point_fields(iteration as usize)),
                )
                .await
            }
        }
        ScenarioKind::StartupReplay => Err(err("startup replay is not a steady-state scenario")),
    }
}

async fn execute_capnp_iteration(
    client: &mut CapnpBenchmarkClient,
    scenario: ScenarioKind,
    iteration: u64,
    seed_objects: usize,
) -> Result<(), DynError> {
    match scenario {
        ScenarioKind::SetPointWrite => {
            let id = format!("point-{iteration}");
            client
                .set_point(
                    "bench-write",
                    &id,
                    52.52 + (iteration % 500) as f64 * 0.00001,
                    13.405 + (iteration % 250) as f64 * 0.00001,
                    iteration as usize,
                )
                .await
        }
        ScenarioKind::FencedSetPointWrite => {
            let id = format!("fenced-{iteration}");
            client
                .set_point(
                    "bench-fenced",
                    &id,
                    52.52 + (iteration % 500) as f64 * 0.00001,
                    13.405 + (iteration % 250) as f64 * 0.00001,
                    iteration as usize,
                )
                .await
        }
        ScenarioKind::GetObjectRead => {
            let index = iteration as usize % seed_objects.max(1);
            client
                .get_object("bench-geo", &format!("point-{index}"))
                .await
        }
        ScenarioKind::NearbyQuery => client.nearby("bench-geo").await,
        ScenarioKind::WithinQuery => client.within("bench-geo").await,
        ScenarioKind::IntersectsQuery => client.intersects("bench-geo").await,
        ScenarioKind::ScanQuery => client.scan("bench-geo").await,
        ScenarioKind::SearchQuery => client.search("bench-text").await,
        ScenarioKind::MixedReadWrite => {
            if iteration.is_multiple_of(2) {
                let index = (iteration as usize / 2) % seed_objects.max(1);
                client
                    .get_object("bench-mixed-read", &format!("point-{index}"))
                    .await
            } else {
                let id = format!("mixed-{iteration}");
                client
                    .set_point(
                        "bench-mixed-write",
                        &id,
                        52.52 + (iteration % 500) as f64 * 0.00001,
                        13.405 + (iteration % 250) as f64 * 0.00001,
                        iteration as usize,
                    )
                    .await
            }
        }
        ScenarioKind::StartupReplay => Err(err("startup replay is not a steady-state scenario")),
    }
}

async fn execute_tile38_iteration(
    client: &mut Tile38Client,
    scenario: ScenarioKind,
    iteration: u64,
    seed_objects: usize,
) -> Result<(), DynError> {
    match scenario {
        ScenarioKind::SetPointWrite => {
            let id = format!("point-{iteration}");
            tile38_set_point(
                client,
                "bench-write",
                &id,
                52.52 + (iteration % 500) as f64 * 0.00001,
                13.405 + (iteration % 250) as f64 * 0.00001,
                iteration as usize,
            )
            .await
        }
        ScenarioKind::FencedSetPointWrite => {
            let id = format!("fenced-{iteration}");
            tile38_set_point(
                client,
                "bench-fenced",
                &id,
                52.52 + (iteration % 500) as f64 * 0.00001,
                13.405 + (iteration % 250) as f64 * 0.00001,
                iteration as usize,
            )
            .await
        }
        ScenarioKind::GetObjectRead => {
            let index = iteration as usize % seed_objects.max(1);
            client
                .command(vec![
                    "GET".to_owned(),
                    "bench-geo".to_owned(),
                    format!("point-{index}"),
                ])
                .await
                .map(|_| ())
        }
        ScenarioKind::NearbyQuery => client
            .command(vec![
                "NEARBY".to_owned(),
                "bench-geo".to_owned(),
                "LIMIT".to_owned(),
                "100".to_owned(),
                "IDS".to_owned(),
                "POINT".to_owned(),
                "52.52".to_owned(),
                "13.405".to_owned(),
                "2000".to_owned(),
            ])
            .await
            .map(|_| ()),
        ScenarioKind::WithinQuery => client
            .command(vec![
                "WITHIN".to_owned(),
                "bench-geo".to_owned(),
                "LIMIT".to_owned(),
                "100".to_owned(),
                "IDS".to_owned(),
                "BOUNDS".to_owned(),
                "52.50".to_owned(),
                "13.39".to_owned(),
                "52.56".to_owned(),
                "13.45".to_owned(),
            ])
            .await
            .map(|_| ()),
        ScenarioKind::IntersectsQuery => client
            .command(vec![
                "INTERSECTS".to_owned(),
                "bench-geo".to_owned(),
                "LIMIT".to_owned(),
                "100".to_owned(),
                "IDS".to_owned(),
                "BOUNDS".to_owned(),
                "52.50".to_owned(),
                "13.39".to_owned(),
                "52.56".to_owned(),
                "13.45".to_owned(),
            ])
            .await
            .map(|_| ()),
        ScenarioKind::ScanQuery => client
            .command(vec![
                "SCAN".to_owned(),
                "bench-geo".to_owned(),
                "LIMIT".to_owned(),
                "100".to_owned(),
                "WHERE".to_owned(),
                "speed".to_owned(),
                "32".to_owned(),
                "96".to_owned(),
                "IDS".to_owned(),
            ])
            .await
            .map(|_| ()),
        ScenarioKind::SearchQuery => client
            .command(vec![
                "SEARCH".to_owned(),
                "bench-text".to_owned(),
                "LIMIT".to_owned(),
                "100".to_owned(),
                "MATCH".to_owned(),
                "msg-*".to_owned(),
                "WHERE".to_owned(),
                "tag_code".to_owned(),
                "1".to_owned(),
                "1".to_owned(),
                "IDS".to_owned(),
            ])
            .await
            .map(|_| ()),
        ScenarioKind::MixedReadWrite => {
            if iteration.is_multiple_of(2) {
                let index = (iteration as usize / 2) % seed_objects.max(1);
                client
                    .command(vec![
                        "GET".to_owned(),
                        "bench-mixed-read".to_owned(),
                        format!("point-{index}"),
                    ])
                    .await
                    .map(|_| ())
            } else {
                let id = format!("mixed-{iteration}");
                tile38_set_point(
                    client,
                    "bench-mixed-write",
                    &id,
                    52.52 + (iteration % 500) as f64 * 0.00001,
                    13.405 + (iteration % 250) as f64 * 0.00001,
                    iteration as usize,
                )
                .await
            }
        }
        ScenarioKind::StartupReplay => Err(err("startup replay is not a steady-state scenario")),
    }
}

async fn seed_point_dataset(
    context: &ScenarioContext,
    collection: &str,
    count: usize,
) -> Result<(), DynError> {
    let client = context.client.clone();
    let base_url = context.base_url.clone();
    let collection = collection.to_owned();
    seed_with_concurrency(32, count, move |index| {
        let client = client.clone();
        let base_url = base_url.clone();
        let collection = collection.clone();
        async move {
            set_point(
                &client,
                &base_url,
                &collection,
                &format!("point-{index}"),
                52.50 + (index % 1_000) as f64 * 0.00001,
                13.39 + (index % 500) as f64 * 0.00001,
                Some(point_fields(index)),
            )
            .await
        }
    })
    .await
}

async fn seed_text_dataset(
    context: &ScenarioContext,
    collection: &str,
    count: usize,
) -> Result<(), DynError> {
    let client = context.client.clone();
    let base_url = context.base_url.clone();
    let collection = collection.to_owned();
    seed_with_concurrency(32, count, move |index| {
        let client = client.clone();
        let base_url = base_url.clone();
        let collection = collection.clone();
        async move {
            post_json_ok(
                &client,
                &format!(
                    "{}/collections/{}/objects/msg-{index}",
                    base_url, collection
                ),
                serde_json::json!({
                    "object": {
                        "String": format!("msg-{index}")
                    },
                    "fields": [{
                        "name": "tag_code",
                        "value": {
                            "type": "number",
                            "value": tag_code(index) as f64
                        }
                    }]
                }),
            )
            .await
        }
    })
    .await
}

async fn seed_fenced_channel_dataset(context: &ScenarioContext) -> Result<(), DynError> {
    create_collection(&context.client, &context.base_url, "bench-fenced").await?;
    for index in 0..DEFAULT_FENCED_RELATED_CHANNELS {
        set_channel(
            &context.client,
            &context.base_url,
            &format!("bench-fenced-related-{index}"),
            serde_json::json!({
                "collection": "bench-fenced",
                "query": {
                    "Nearby": {
                        "lat": 52.52,
                        "lon": 13.405,
                        "meters": 2_500.0,
                        "options": {}
                    }
                },
                "detect": ["Enter"],
                "commands": ["Set"]
            }),
        )
        .await?;
    }

    for index in 0..DEFAULT_FENCED_UNRELATED_CHANNELS {
        set_channel(
            &context.client,
            &context.base_url,
            &format!("bench-fenced-unrelated-{index}"),
            serde_json::json!({
                "collection": format!("bench-unrelated-{index}"),
                "query": {
                    "Nearby": {
                        "lat": 52.52,
                        "lon": 13.405,
                        "meters": 2_500.0,
                        "options": {}
                    }
                },
                "detect": ["Enter"],
                "commands": ["Set"]
            }),
        )
        .await?;
    }

    Ok(())
}

async fn capnp_seed_point_dataset(
    addr: &str,
    collection: &str,
    count: usize,
) -> Result<(), DynError> {
    let addr = addr.to_owned();
    let collection = collection.to_owned();
    let counter = Arc::new(AtomicU64::new(0));
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut tasks = Vec::new();
            for _ in 0..32 {
                let addr = addr.clone();
                let collection = collection.clone();
                let counter = Arc::clone(&counter);
                tasks.push(tokio::task::spawn_local(async move {
                    let mut client = CapnpBenchmarkClient::connect(&addr).await?;
                    loop {
                        let index = counter.fetch_add(1, Ordering::Relaxed) as usize;
                        if index >= count {
                            break;
                        }
                        client
                            .set_point(
                                &collection,
                                &format!("point-{index}"),
                                52.50 + (index % 1_000) as f64 * 0.00001,
                                13.39 + (index % 500) as f64 * 0.00001,
                                index,
                            )
                            .await?;
                    }
                    Ok::<(), DynError>(())
                }));
            }
            for task in tasks {
                task.await??;
            }
            Ok::<(), DynError>(())
        })
        .await
}

async fn capnp_seed_text_dataset(
    addr: &str,
    collection: &str,
    count: usize,
) -> Result<(), DynError> {
    let addr = addr.to_owned();
    let collection = collection.to_owned();
    let counter = Arc::new(AtomicU64::new(0));
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut tasks = Vec::new();
            for _ in 0..32 {
                let addr = addr.clone();
                let collection = collection.clone();
                let counter = Arc::clone(&counter);
                tasks.push(tokio::task::spawn_local(async move {
                    let mut client = CapnpBenchmarkClient::connect(&addr).await?;
                    loop {
                        let index = counter.fetch_add(1, Ordering::Relaxed) as usize;
                        if index >= count {
                            break;
                        }
                        client
                            .set_string(&collection, &format!("msg-{index}"), index)
                            .await?;
                    }
                    Ok::<(), DynError>(())
                }));
            }
            for task in tasks {
                task.await??;
            }
            Ok::<(), DynError>(())
        })
        .await
}

async fn capnp_seed_fenced_channel_dataset(context: &CapnpScenarioContext) -> Result<(), DynError> {
    let addr = context.addr.clone();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut client = CapnpBenchmarkClient::connect(&addr).await?;
            for index in 0..DEFAULT_FENCED_RELATED_CHANNELS {
                client
                    .set_nearby_channel(&format!("bench-fenced-related-{index}"), "bench-fenced")
                    .await?;
            }

            for index in 0..DEFAULT_FENCED_UNRELATED_CHANNELS {
                client
                    .set_nearby_channel(
                        &format!("bench-fenced-unrelated-{index}"),
                        &format!("bench-unrelated-{index}"),
                    )
                    .await?;
            }

            Ok::<(), DynError>(())
        })
        .await
}

async fn tile38_seed_point_dataset(
    port: u16,
    collection: &str,
    count: usize,
) -> Result<(), DynError> {
    let collection = collection.to_owned();
    let counter = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();

    for _ in 0..32 {
        let collection = collection.clone();
        let counter = Arc::clone(&counter);
        tasks.spawn(async move {
            let mut client = Tile38Client::connect(port).await?;
            loop {
                let index = counter.fetch_add(1, Ordering::Relaxed) as usize;
                if index >= count {
                    break;
                }
                tile38_set_point(
                    &mut client,
                    &collection,
                    &format!("point-{index}"),
                    52.50 + (index % 1_000) as f64 * 0.00001,
                    13.39 + (index % 500) as f64 * 0.00001,
                    index,
                )
                .await?;
            }
            Ok::<(), DynError>(())
        });
    }

    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

async fn tile38_seed_text_dataset(
    port: u16,
    collection: &str,
    count: usize,
) -> Result<(), DynError> {
    let collection = collection.to_owned();
    let counter = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();

    for _ in 0..32 {
        let collection = collection.clone();
        let counter = Arc::clone(&counter);
        tasks.spawn(async move {
            let mut client = Tile38Client::connect(port).await?;
            loop {
                let index = counter.fetch_add(1, Ordering::Relaxed) as usize;
                if index >= count {
                    break;
                }
                client
                    .command(vec![
                        "SET".to_owned(),
                        collection.clone(),
                        format!("msg-{index}"),
                        "FIELD".to_owned(),
                        "tag_code".to_owned(),
                        tag_code(index).to_string(),
                        "STRING".to_owned(),
                        format!("msg-{index}"),
                    ])
                    .await?;
            }
            Ok::<(), DynError>(())
        });
    }

    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

async fn tile38_seed_fenced_channel_dataset(
    context: &Tile38ScenarioContext,
) -> Result<(), DynError> {
    let mut client = Tile38Client::connect(context.port).await?;
    for index in 0..DEFAULT_FENCED_RELATED_CHANNELS {
        tile38_set_nearby_channel(
            &mut client,
            &format!("bench-fenced-related-{index}"),
            "bench-fenced",
        )
        .await?;
    }

    for index in 0..DEFAULT_FENCED_UNRELATED_CHANNELS {
        tile38_set_nearby_channel(
            &mut client,
            &format!("bench-fenced-unrelated-{index}"),
            &format!("bench-unrelated-{index}"),
        )
        .await?;
    }

    Ok(())
}

async fn tile38_set_nearby_channel(
    client: &mut Tile38Client,
    name: &str,
    collection: &str,
) -> Result<(), DynError> {
    client
        .command(vec![
            "SETCHAN".to_owned(),
            name.to_owned(),
            "NEARBY".to_owned(),
            collection.to_owned(),
            "FENCE".to_owned(),
            "DETECT".to_owned(),
            "enter".to_owned(),
            "COMMANDS".to_owned(),
            "set".to_owned(),
            "POINT".to_owned(),
            "52.52".to_owned(),
            "13.405".to_owned(),
            "2500".to_owned(),
        ])
        .await
        .map(|_| ())
}

async fn tile38_set_point(
    client: &mut Tile38Client,
    collection: &str,
    id: &str,
    lat: f64,
    lon: f64,
    index: usize,
) -> Result<(), DynError> {
    client
        .command(vec![
            "SET".to_owned(),
            collection.to_owned(),
            id.to_owned(),
            "FIELD".to_owned(),
            "speed".to_owned(),
            (index % 120).to_string(),
            "FIELD".to_owned(),
            "tag_code".to_owned(),
            tag_code(index).to_string(),
            "POINT".to_owned(),
            lat.to_string(),
            lon.to_string(),
        ])
        .await
        .map(|_| ())
}

async fn seed_with_concurrency<F, Fut>(
    concurrency: usize,
    count: usize,
    operation: F,
) -> Result<(), DynError>
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), DynError>> + Send + 'static,
{
    let operation = Arc::new(operation);
    let counter = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();

    for _ in 0..concurrency.max(1) {
        let operation = Arc::clone(&operation);
        let counter = Arc::clone(&counter);
        tasks.spawn(async move {
            loop {
                let next = counter.fetch_add(1, Ordering::Relaxed) as usize;
                if next >= count {
                    break;
                }
                operation(next).await?;
            }
            Ok::<(), DynError>(())
        });
    }

    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

async fn set_point(
    client: &reqwest::Client,
    base_url: &str,
    collection: &str,
    id: &str,
    lat: f64,
    lon: f64,
    fields: Option<serde_json::Value>,
) -> Result<(), DynError> {
    let mut body = serde_json::json!({
        "object": {
            "Point": {
                "lat": lat,
                "lon": lon,
                "z": null
            }
        }
    });
    if let Some(fields) = fields {
        body["fields"] = fields;
    }
    post_json_ok(
        client,
        &format!("{base_url}/collections/{collection}/objects/{id}"),
        body,
    )
    .await
}

async fn create_collection(
    client: &reqwest::Client,
    base_url: &str,
    collection: &str,
) -> Result<(), DynError> {
    post_json_ok(
        client,
        &format!("{base_url}/collections/{collection}"),
        serde_json::json!({}),
    )
    .await
}

async fn set_channel(
    client: &reqwest::Client,
    base_url: &str,
    name: &str,
    def: serde_json::Value,
) -> Result<(), DynError> {
    post_json_ok(
        client,
        &format!("{base_url}/channels"),
        serde_json::json!({
            "name": name,
            "def": def
        }),
    )
    .await
}

async fn get_object(
    client: &reqwest::Client,
    base_url: &str,
    collection: &str,
    id: &str,
) -> Result<(), DynError> {
    let response = client
        .get(format!("{base_url}/collections/{collection}/objects/{id}"))
        .send()
        .await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(err(format!(
        "GET /collections/{collection}/objects/{id} failed with {}: {body}",
        status
    )))
}

async fn post_json_ok(
    client: &reqwest::Client,
    url: &str,
    body: serde_json::Value,
) -> Result<(), DynError> {
    let response = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(err(format!("POST {url} failed with {status}: {body}")))
}

struct CapnpBenchmarkClient {
    client: capnp_api::lat_lng::Client,
}

impl CapnpBenchmarkClient {
    async fn connect(addr: &str) -> Result<Self, DynError> {
        let mut last_error = None;
        for _ in 0..240 {
            match Self::connect_once(addr).await {
                Ok(client) => return Ok(client),
                Err(error) => {
                    last_error = Some(error);
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| err(format!("failed to connect to Cap'n Proto at {addr}"))))
    }

    async fn connect_once(addr: &str) -> Result<Self, DynError> {
        let stream = TcpStream::connect(addr).await?;
        let (reader, writer) = tokio::io::split(stream);
        let network = twoparty::VatNetwork::new(
            reader.compat(),
            writer.compat_write(),
            rpc_twoparty_capnp::Side::Client,
            ReaderOptions::new(),
        );
        let mut rpc_system = RpcSystem::new(Box::new(network), None);
        let client: capnp_api::lat_lng::Client =
            rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);
        tokio::task::spawn_local(async move {
            let _ = rpc_system.await;
        });

        let mut auth = client.auth_request();
        auth.get().set_token(DEFAULT_TOKEN);
        let response = auth.send().promise.await?;
        let response = response.get()?.get_resp()?;
        if !response.get_ok() {
            return Err(err(capnp_text(response.get_error())?));
        }

        Ok(Self { client })
    }

    async fn set_point(
        &mut self,
        collection: &str,
        id: &str,
        lat: f64,
        lon: f64,
        index: usize,
    ) -> Result<(), DynError> {
        let mut request = self.client.set_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        payload.set_id(id);
        fill_capnp_point(payload.reborrow().init_object(), lat, lon);
        fill_capnp_point_fields(payload.reborrow().init_fields(2), index);
        payload.set_condition(capnp_api::SetCondition::Always);
        let response = request.send().promise.await?;
        ensure_capnp_ok(response.get()?.get_resp()?)
    }

    async fn set_string(
        &mut self,
        collection: &str,
        id: &str,
        index: usize,
    ) -> Result<(), DynError> {
        let mut request = self.client.set_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        payload.set_id(id);
        payload.reborrow().init_object().set_string(id);
        let mut fields = payload.reborrow().init_fields(1);
        fill_capnp_number_field(fields.reborrow().get(0), "tag_code", tag_code(index) as f64);
        payload.set_condition(capnp_api::SetCondition::Always);
        let response = request.send().promise.await?;
        ensure_capnp_ok(response.get()?.get_resp()?)
    }

    async fn get_object(&mut self, collection: &str, id: &str) -> Result<(), DynError> {
        let mut request = self.client.get_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        payload.set_id(id);
        let response = request.send().promise.await?;
        let response = response.get()?;
        if response.get_ok() {
            Ok(())
        } else {
            Err(err(capnp_text(response.get_error())?))
        }
    }

    async fn nearby(&mut self, collection: &str) -> Result<(), DynError> {
        let mut request = self.client.nearby_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        payload.set_lat(52.52);
        payload.set_lon(13.405);
        payload.set_meters(2_000.0);
        fill_capnp_search_options(payload.reborrow().init_options());
        let response = request.send().promise.await?;
        ensure_capnp_search_ok(response.get()?.get_resp()?)
    }

    async fn within(&mut self, collection: &str) -> Result<(), DynError> {
        let mut request = self.client.within_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        fill_capnp_bounds_area(payload.reborrow().init_area());
        fill_capnp_search_options(payload.reborrow().init_options());
        let response = request.send().promise.await?;
        ensure_capnp_search_ok(response.get()?.get_resp()?)
    }

    async fn intersects(&mut self, collection: &str) -> Result<(), DynError> {
        let mut request = self.client.intersects_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        fill_capnp_bounds_area(payload.reborrow().init_area());
        fill_capnp_search_options(payload.reborrow().init_options());
        let response = request.send().promise.await?;
        ensure_capnp_search_ok(response.get()?.get_resp()?)
    }

    async fn scan(&mut self, collection: &str) -> Result<(), DynError> {
        let mut request = self.client.scan_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        let mut options = payload.reborrow().init_options();
        fill_capnp_search_options(options.reborrow());
        let mut filters = options.reborrow().init_where(1);
        fill_capnp_range_filter(filters.reborrow().get(0), "speed", 32.0, 96.0);
        let response = request.send().promise.await?;
        ensure_capnp_search_ok(response.get()?.get_resp()?)
    }

    async fn search(&mut self, collection: &str) -> Result<(), DynError> {
        let mut request = self.client.search_request();
        let mut payload = request.get().init_req();
        payload.set_collection(collection);
        let mut options = payload.reborrow().init_options();
        fill_capnp_search_options(options.reborrow());
        options.set_match("msg-*");
        let mut filters = options.reborrow().init_where(1);
        fill_capnp_range_filter(filters.reborrow().get(0), "tag_code", 1.0, 1.0);
        let response = request.send().promise.await?;
        ensure_capnp_search_ok(response.get()?.get_resp()?)
    }

    async fn set_nearby_channel(&mut self, name: &str, collection: &str) -> Result<(), DynError> {
        let mut request = self.client.setchan_request();
        let mut payload = request.get().init_req();
        payload.set_name(name);
        let mut nearby = payload.reborrow().init_search().init_nearby();
        nearby.set_collection(collection);
        nearby.set_lat(52.52);
        nearby.set_lon(13.405);
        nearby.set_meters(2_500.0);
        fill_capnp_search_options(nearby.reborrow().init_options());
        let mut detect = payload.reborrow().init_detect(1);
        detect.set(0, capnp_api::DetectType::Enter);
        let mut commands = payload.reborrow().init_commands(1);
        commands.set(0, "set");
        let response = request.send().promise.await?;
        ensure_capnp_ok(response.get()?.get_resp()?)
    }
}

fn fill_capnp_point(builder: capnp_api::geo_object::Builder<'_>, lat: f64, lon: f64) {
    let mut point = builder.init_point();
    point.set_lat(lat);
    point.set_lon(lon);
}

fn fill_capnp_point_fields(
    mut fields: capnp::struct_list::Builder<'_, capnp_api::field_entry::Owned>,
    index: usize,
) {
    fill_capnp_number_field(fields.reborrow().get(0), "speed", (index % 120) as f64);
    fill_capnp_number_field(fields.reborrow().get(1), "tag_code", tag_code(index) as f64);
}

fn fill_capnp_number_field(mut field: capnp_api::field_entry::Builder<'_>, name: &str, value: f64) {
    field.set_name(name);
    field.set_number(value);
}

fn fill_capnp_search_options(mut options: capnp_api::search_options::Builder<'_>) {
    options.set_limit(100);
    options.set_nofields(true);
    options.set_include_count(false);
    options.set_asc(true);
    options.set_output(capnp_api::OutputFormat::Ids);
}

fn fill_capnp_range_filter(
    mut filter: capnp_api::where_filter::Builder<'_>,
    field: &str,
    min: f64,
    max: f64,
) {
    filter.set_field(field);
    filter.set_min(min);
    filter.set_max(max);
}

fn fill_capnp_bounds_area(area: capnp_api::area_spec::Builder<'_>) {
    let mut bounds = area.init_bounds();
    bounds.set_min_lat(52.50);
    bounds.set_min_lon(13.39);
    bounds.set_max_lat(52.56);
    bounds.set_max_lon(13.45);
}

fn ensure_capnp_ok(response: capnp_api::ok_response::Reader<'_>) -> Result<(), DynError> {
    if response.get_ok() {
        Ok(())
    } else {
        Err(err(capnp_text(response.get_error())?))
    }
}

fn ensure_capnp_search_ok(
    response: capnp_api::search_response::Reader<'_>,
) -> Result<(), DynError> {
    if response.get_ok() {
        Ok(())
    } else {
        Err(err(capnp_text(response.get_error())?))
    }
}

fn capnp_text(value: capnp::Result<capnp::text::Reader<'_>>) -> Result<String, DynError> {
    Ok(value?.to_string()?)
}

struct Tile38Client {
    reader: BufReader<TcpStream>,
}

impl Tile38Client {
    async fn connect(port: u16) -> Result<Self, DynError> {
        let stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream.set_nodelay(true)?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    async fn command(&mut self, args: Vec<String>) -> Result<RespValue, DynError> {
        let frame = encode_resp_command(&args);
        self.reader.get_mut().write_all(&frame).await?;
        self.reader.get_mut().flush().await?;
        let response = read_resp_value(&mut self.reader).await?;
        match response {
            RespValue::Error(message) => Err(err(format!("Tile38 command failed: {message}"))),
            other => Ok(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RespValue {
    Simple(String),
    Error(String),
    Integer(i64),
    Bulk(Option<Vec<u8>>),
    Array(Option<Vec<RespValue>>),
}

fn encode_resp_command(args: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
    for arg in args {
        out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        out.extend_from_slice(arg.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn read_resp_value<'a, R>(
    reader: &'a mut R,
) -> Pin<Box<dyn Future<Output = Result<RespValue, DynError>> + Send + 'a>>
where
    R: AsyncBufRead + Unpin + Send + 'a,
{
    Box::pin(async move {
        let line = read_resp_line(reader).await?;
        let Some((&prefix, rest)) = line.split_first() else {
            return Err(err("empty RESP line"));
        };
        match prefix {
            b'+' => Ok(RespValue::Simple(resp_string(rest)?)),
            b'-' => Ok(RespValue::Error(resp_string(rest)?)),
            b':' => Ok(RespValue::Integer(parse_resp_i64(rest)?)),
            b'$' => {
                let len = parse_resp_i64(rest)?;
                if len < 0 {
                    return Ok(RespValue::Bulk(None));
                }
                let len = usize::try_from(len)
                    .map_err(|_| err(format!("invalid RESP bulk length: {len}")))?;
                let mut bytes = vec![0; len];
                reader.read_exact(&mut bytes).await?;
                read_resp_crlf(reader).await?;
                Ok(RespValue::Bulk(Some(bytes)))
            }
            b'*' => {
                let len = parse_resp_i64(rest)?;
                if len < 0 {
                    return Ok(RespValue::Array(None));
                }
                let len = usize::try_from(len)
                    .map_err(|_| err(format!("invalid RESP array length: {len}")))?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(read_resp_value(reader).await?);
                }
                Ok(RespValue::Array(Some(values)))
            }
            other => Err(err(format!("unsupported RESP prefix: {}", other as char))),
        }
    })
}

async fn read_resp_line<R>(reader: &mut R) -> Result<Vec<u8>, DynError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let bytes = reader.read_until(b'\n', &mut line).await?;
    if bytes == 0 {
        return Err(err("unexpected EOF while reading RESP line"));
    }
    if line.len() < 2 || !line.ends_with(b"\r\n") {
        return Err(err("malformed RESP line"));
    }
    line.truncate(line.len() - 2);
    Ok(line)
}

async fn read_resp_crlf<R>(reader: &mut R) -> Result<(), DynError>
where
    R: AsyncBufRead + Unpin,
{
    let mut crlf = [0_u8; 2];
    reader.read_exact(&mut crlf).await?;
    if crlf != *b"\r\n" {
        return Err(err("malformed RESP bulk terminator"));
    }
    Ok(())
}

fn parse_resp_i64(bytes: &[u8]) -> Result<i64, DynError> {
    resp_string(bytes)?.parse::<i64>().map_err(|_| {
        err(format!(
            "invalid RESP integer: {}",
            String::from_utf8_lossy(bytes)
        ))
    })
}

fn resp_string(bytes: &[u8]) -> Result<String, DynError> {
    Ok(std::str::from_utf8(bytes)?.to_owned())
}

fn point_fields(index: usize) -> serde_json::Value {
    serde_json::json!([
        {
            "name": "speed",
            "value": {
                "type": "number",
                "value": (index % 120) as f64
            }
        },
        {
            "name": "tag",
            "value": {
                "type": "text",
                "value": if index.is_multiple_of(2) { "keep" } else { "drop" }
            }
        },
        {
            "name": "tag_code",
            "value": {
                "type": "number",
                "value": tag_code(index) as f64
            }
        }
    ])
}

fn tag_code(index: usize) -> u8 {
    if index.is_multiple_of(2) { 1 } else { 0 }
}

fn build_client() -> Result<reqwest::Client, DynError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {DEFAULT_TOKEN}"))?,
    );
    Ok(reqwest::Client::builder()
        .default_headers(headers)
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(30))
        .build()?)
}

fn write_config(
    path: &Path,
    profile: StorageProfile,
    ports: ServerPorts,
    aof_path: &Path,
    webhook_queue_path: &Path,
    aof_overrides: AofOverrideArgs,
    capnp_enabled: bool,
) -> Result<(), DynError> {
    let mut config = RuntimeConfig {
        listen_addr: format!("127.0.0.1:{}", ports.http),
        capnp_enabled,
        capnp_listen_addr: format!("127.0.0.1:{}", ports.capnp),
        storage: match profile {
            StorageProfile::Memory => StorageMode::Memory,
            StorageProfile::Aof => StorageMode::Aof {
                path: aof_path.to_path_buf(),
            },
        },
        webhook_queue_path: Some(webhook_queue_path.to_path_buf()),
        auth: latlng_auth::AuthConfig {
            bearer_token: Some(DEFAULT_TOKEN.to_owned()),
            ..latlng_auth::AuthConfig::default()
        },
        ..RuntimeConfig::default()
    };
    if profile == StorageProfile::Aof {
        if let Some(value) = aof_overrides.writer_queue_limit {
            config.aof_writer_queue_limit = value.max(1);
        }
        if let Some(value) = aof_overrides.group_commit_delay_ms {
            config.aof_group_commit_delay_ms = value;
        }
        if let Some(value) = aof_overrides.group_commit_max_requests {
            config.aof_group_commit_max_requests = value.max(1);
        }
    }
    save_to_path(&config, path)?;
    Ok(())
}

fn free_port() -> Result<u16, DynError> {
    let listener = StdTcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn repo_root() -> Result<PathBuf, DynError> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| err("failed to determine repo root"))
}

fn default_server_binary(repo_root: &Path) -> Result<PathBuf, DynError> {
    let release = repo_root.join("target/release/latlng-server");
    if release.exists() {
        return Ok(release);
    }
    let debug = repo_root.join("target/debug/latlng-server");
    if debug.exists() {
        return Ok(debug);
    }
    Err(err(
        "could not find target/release/latlng-server or target/debug/latlng-server; build the server first",
    ))
}

fn read_base_url_from_config(config_path: &Path) -> Result<String, DynError> {
    let raw = fs::read_to_string(config_path)?;
    let value = serde_json::from_str::<serde_json::Value>(&raw)?;
    let listen_addr = value["listen_addr"]
        .as_str()
        .ok_or_else(|| err("missing listen_addr in benchmark config"))?;
    Ok(format!("http://{listen_addr}"))
}

fn read_capnp_addr_from_config(config_path: &Path) -> Result<String, DynError> {
    let raw = fs::read_to_string(config_path)?;
    let value = serde_json::from_str::<serde_json::Value>(&raw)?;
    value["capnp_listen_addr"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| err("missing capnp_listen_addr in benchmark config"))
}

fn read_aof_path_from_config(config_path: &Path) -> Result<Option<PathBuf>, DynError> {
    let raw = fs::read_to_string(config_path)?;
    let value = serde_json::from_str::<serde_json::Value>(&raw)?;
    match value["storage"]["type"].as_str() {
        Some("aof") => Ok(value["storage"]["path"].as_str().map(PathBuf::from)),
        _ => Ok(None),
    }
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn percentile_ms(sorted_micros: &[u64], percentile: f64) -> f64 {
    if sorted_micros.is_empty() {
        return 0.0;
    }
    let rank = ((sorted_micros.len() - 1) as f64 * percentile).round() as usize;
    sorted_micros[rank] as f64 / 1_000.0
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), DynError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

fn print_result(result: &BenchmarkResult) {
    if let Some(steady) = result.steady_state.as_ref() {
        println!(
            "result engine={} transport={} scenario={} profile={} concurrency={} requests={} errors={} ops/sec={:.2} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3}",
            result.engine.as_str(),
            result.transport.as_str(),
            result.scenario,
            result.profile,
            result.concurrency.unwrap_or_default(),
            steady.request_count,
            steady.error_count,
            steady.ops_per_sec,
            steady.latency_ms.mean,
            steady.latency_ms.p50,
            steady.latency_ms.p95,
            steady.latency_ms.p99
        );
    }
    if let Some(startup) = result.startup.as_ref() {
        println!(
            "result engine={} transport={} scenario={} profile={} records={} replay_ms={:.3} log_bytes={}",
            result.engine.as_str(),
            result.transport.as_str(),
            result.scenario,
            result.profile,
            startup.record_count,
            startup.replay_duration_ms,
            startup.log_bytes
        );
    }
}

fn compare_command(args: CompareArgs) -> Result<(), DynError> {
    let baseline = read_benchmark_file(&args.baseline)?;
    let candidate = read_benchmark_file(&args.candidate)?;
    let baseline_map = result_map(&baseline.results);
    let candidate_map = result_map(&candidate.results);

    let mut entries = Vec::new();
    for (key, baseline_result) in &baseline_map {
        let Some(candidate_result) = candidate_map.get(key) else {
            continue;
        };
        entries.push(ComparisonEntry {
            scenario: key.scenario.clone(),
            profile: key.profile.clone(),
            concurrency: key.concurrency,
            baseline_engine: baseline_result.engine.as_str().to_owned(),
            candidate_engine: candidate_result.engine.as_str().to_owned(),
            baseline_transport: baseline_result.transport.as_str().to_owned(),
            candidate_transport: candidate_result.transport.as_str().to_owned(),
            steady_state: compare_steady_state(
                baseline_result.steady_state.as_ref(),
                candidate_result.steady_state.as_ref(),
            ),
            startup: compare_startup(
                baseline_result.startup.as_ref(),
                candidate_result.startup.as_ref(),
            ),
        });
    }
    entries.sort_by(|left, right| {
        (
            left.scenario.as_str(),
            left.profile.as_str(),
            left.concurrency,
        )
            .cmp(&(
                right.scenario.as_str(),
                right.profile.as_str(),
                right.concurrency,
            ))
    });

    for entry in &entries {
        if let Some(steady) = entry.steady_state.as_ref() {
            println!(
                "compare baseline_engine={} baseline_transport={} candidate_engine={} candidate_transport={} scenario={} profile={} concurrency={} ops/sec {:+.2}% p95 {:+.2}% p99 {:+.2}%",
                entry.baseline_engine,
                entry.baseline_transport,
                entry.candidate_engine,
                entry.candidate_transport,
                entry.scenario,
                entry.profile,
                entry.concurrency.unwrap_or_default(),
                steady.ops_per_sec_delta_pct.unwrap_or(0.0),
                steady.p95_delta_pct.unwrap_or(0.0),
                steady.p99_delta_pct.unwrap_or(0.0),
            );
        }
        if let Some(startup) = entry.startup.as_ref() {
            println!(
                "compare baseline_engine={} baseline_transport={} candidate_engine={} candidate_transport={} scenario={} profile={} replay_ms {:+.2}% ({:+.3} ms)",
                entry.baseline_engine,
                entry.baseline_transport,
                entry.candidate_engine,
                entry.candidate_transport,
                entry.scenario,
                entry.profile,
                startup.replay_delta_pct.unwrap_or(0.0),
                startup.replay_delta_ms
            );
        }
    }

    let comparison = ComparisonFile {
        schema_version: 1,
        generated_at: Utc::now().to_rfc3339(),
        baseline_file: args.baseline.display().to_string(),
        candidate_file: args.candidate.display().to_string(),
        entries,
    };
    if let Some(path) = args.output.as_ref() {
        write_json(path, &comparison)?;
        println!("wrote comparison to {}", path.display());
    }
    Ok(())
}

fn read_benchmark_file(path: &Path) -> Result<BenchmarkFile, DynError> {
    let raw = fs::read_to_string(path)
        .map_err(|error| err(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_str(&raw)
        .map_err(|error| err(format!("failed to parse {}: {error}", path.display())))
}

fn result_map(results: &[BenchmarkResult]) -> BTreeMap<ScenarioKey, BenchmarkResult> {
    results
        .iter()
        .cloned()
        .map(|result| {
            (
                ScenarioKey {
                    scenario: result.scenario.clone(),
                    profile: result.profile.clone(),
                    concurrency: result.concurrency,
                },
                result,
            )
        })
        .collect()
}

fn compare_steady_state(
    baseline: Option<&SteadyStateMetrics>,
    candidate: Option<&SteadyStateMetrics>,
) -> Option<SteadyStateComparison> {
    let (Some(baseline), Some(candidate)) = (baseline, candidate) else {
        return None;
    };
    Some(SteadyStateComparison {
        baseline_ops_per_sec: baseline.ops_per_sec,
        candidate_ops_per_sec: candidate.ops_per_sec,
        ops_per_sec_delta: candidate.ops_per_sec - baseline.ops_per_sec,
        ops_per_sec_delta_pct: percent_delta(candidate.ops_per_sec, baseline.ops_per_sec),
        baseline_p95_ms: baseline.latency_ms.p95,
        candidate_p95_ms: candidate.latency_ms.p95,
        p95_delta_ms: candidate.latency_ms.p95 - baseline.latency_ms.p95,
        p95_delta_pct: percent_delta(candidate.latency_ms.p95, baseline.latency_ms.p95),
        baseline_p99_ms: baseline.latency_ms.p99,
        candidate_p99_ms: candidate.latency_ms.p99,
        p99_delta_ms: candidate.latency_ms.p99 - baseline.latency_ms.p99,
        p99_delta_pct: percent_delta(candidate.latency_ms.p99, baseline.latency_ms.p99),
    })
}

fn compare_startup(
    baseline: Option<&StartupMetrics>,
    candidate: Option<&StartupMetrics>,
) -> Option<StartupComparison> {
    let (Some(baseline), Some(candidate)) = (baseline, candidate) else {
        return None;
    };
    Some(StartupComparison {
        baseline_replay_ms: baseline.replay_duration_ms,
        candidate_replay_ms: candidate.replay_duration_ms,
        replay_delta_ms: candidate.replay_duration_ms - baseline.replay_duration_ms,
        replay_delta_pct: percent_delta(candidate.replay_duration_ms, baseline.replay_duration_ms),
    })
}

fn percent_delta(candidate: f64, baseline: f64) -> Option<f64> {
    if baseline == 0.0 {
        None
    } else {
        Some(((candidate - baseline) / baseline) * 100.0)
    }
}

fn parse_concurrency_list(value: &str) -> Result<Vec<usize>, DynError> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<usize>()
                .map(|value| value.max(1))
                .map_err(|_| err(format!("invalid concurrency value: {entry}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        return Err(err("concurrency list must not be empty"));
    }
    Ok(values)
}

fn parse_u64_flag(value: &str, flag: &str) -> Result<u64, DynError> {
    value
        .parse::<u64>()
        .map_err(|_| err(format!("invalid value for {flag}: {value}")))
}

fn parse_usize_flag(value: &str, flag: &str) -> Result<usize, DynError> {
    value
        .parse::<usize>()
        .map_err(|_| err(format!("invalid value for {flag}: {value}")))
}

fn expect_value<'a>(
    args: &'a [String],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, DynError> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| err(format!("missing value for {flag}")))
}

fn trim_logs(logs: &str) -> String {
    const LIMIT: usize = 2_000;
    if logs.len() <= LIMIT {
        logs.to_owned()
    } else {
        format!("...{}", &logs[logs.len() - LIMIT..])
    }
}

fn git_commit() -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--short")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn err(message: impl Into<String>) -> DynError {
    Box::new(io::Error::other(message.into()))
}

fn print_help() {
    println!(
        "\
latlng-server-benchmark

Usage:
  latlng-server-benchmark run [options]
  latlng-server-benchmark compare <baseline.json> <candidate.json> [--output <path>]

Run options:
  --engine <latlng|tile38>
  --latlng-transport <http|capnp>
  --preset <standard|write-heavy|query-heavy|aof-tuning|geofence-heavy>
  --scenario <all|set_point_write|fenced_set_point_write|get_object_read|nearby_query|within_query|intersects_query|scan_query|search_query|mixed_read_write|startup_replay>
  --profile <memory|aof|both>
  --concurrency <n>
  --concurrency-list <n1,n2,...>
  --warmup-secs <n>
  --measure-secs <n>
  --seed-objects <n>
  --startup-records <n>
  --aof-writer-queue-limit <n>
  --aof-group-commit-delay-ms <n>
  --aof-group-commit-max-requests <n>
  --server-bin <path>
  --tile38-server-bin <path>
  --tile38-appendonly <yes|no>
  --output <path>

Compare options:
  --output <path>
"
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::io::{AsyncWriteExt, BufReader};

    use super::{
        BenchmarkEngine, BenchmarkFile, BenchmarkResult, LatLngTransport, LatencyStats, RespValue,
        RunArgs, StartupMetrics, SteadyStateMetrics, Tile38AppendOnly, encode_resp_command,
        percent_delta, percentile_ms, read_resp_value, result_map,
    };

    #[test]
    fn percentile_ms_uses_sorted_input() {
        let values = vec![1_000, 2_000, 3_000, 4_000, 5_000];
        assert_eq!(percentile_ms(&values, 0.50), 3.0);
        assert_eq!(percentile_ms(&values, 0.95), 5.0);
    }

    #[test]
    fn percent_delta_handles_zero_baseline() {
        assert_eq!(percent_delta(10.0, 0.0), None);
        assert_eq!(percent_delta(12.0, 10.0), Some(20.0));
    }

    #[test]
    fn result_map_uses_scenario_profile_and_concurrency() {
        let first = BenchmarkResult {
            engine: BenchmarkEngine::LatLng,
            transport: LatLngTransport::Http,
            scenario: "get_object_read".to_owned(),
            profile: "memory".to_owned(),
            concurrency: Some(8),
            warmup_secs: 1,
            measure_secs: 1,
            seed_objects: 100,
            steady_state: Some(SteadyStateMetrics {
                request_count: 10,
                error_count: 0,
                ops_per_sec: 10.0,
                latency_ms: LatencyStats {
                    mean: 1.0,
                    p50: 1.0,
                    p95: 2.0,
                    p99: 3.0,
                },
            }),
            startup: None,
        };
        let second = BenchmarkResult {
            engine: BenchmarkEngine::Tile38,
            transport: LatLngTransport::Resp,
            scenario: "startup_replay".to_owned(),
            profile: "aof".to_owned(),
            concurrency: None,
            warmup_secs: 0,
            measure_secs: 0,
            seed_objects: 100,
            steady_state: None,
            startup: Some(StartupMetrics {
                replay_duration_ms: 10.0,
                record_count: 100,
                log_bytes: 1_024,
            }),
        };
        let map = result_map(&[first, second]);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn run_args_parse_tile38_options() {
        let args = strings(&[
            "--engine",
            "tile38",
            "--tile38-server-bin",
            "/opt/bin/tile38-server",
            "--tile38-appendonly",
            "yes",
        ]);
        let parsed = RunArgs::parse(&args).unwrap();
        assert_eq!(parsed.engine, BenchmarkEngine::Tile38);
        assert_eq!(
            parsed.tile38_server_bin,
            PathBuf::from("/opt/bin/tile38-server")
        );
        assert_eq!(parsed.tile38_appendonly, Tile38AppendOnly::Yes);
    }

    #[test]
    fn run_args_parse_latlng_transport() {
        let args = strings(&["--latlng-transport", "capnp"]);
        let parsed = RunArgs::parse(&args).unwrap();
        assert_eq!(parsed.latlng_transport, LatLngTransport::Capnp);
    }

    #[test]
    fn run_args_default_to_latlng_and_tile38_appendonly_no() {
        let parsed = RunArgs::parse(&[]).unwrap();
        assert_eq!(parsed.engine, BenchmarkEngine::LatLng);
        assert_eq!(parsed.latlng_transport, LatLngTransport::Http);
        assert_eq!(parsed.tile38_server_bin, PathBuf::from("tile38-server"));
        assert_eq!(parsed.tile38_appendonly, Tile38AppendOnly::No);
    }

    #[test]
    fn old_benchmark_results_default_to_latlng_engine() {
        let raw = r#"
        {
          "schema_version": 1,
          "tool_version": "0.0.0",
          "generated_at": "2026-05-10T00:00:00Z",
          "git_commit": null,
          "results": [{
            "scenario": "get_object_read",
            "profile": "memory",
            "concurrency": 1,
            "warmup_secs": 0,
            "measure_secs": 1,
            "seed_objects": 10,
            "steady_state": null,
            "startup": null
          }]
        }
        "#;
        let parsed: BenchmarkFile = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.results[0].engine, BenchmarkEngine::LatLng);
        assert_eq!(parsed.results[0].transport, LatLngTransport::Http);
    }

    #[test]
    fn resp_command_encoding_uses_array_bulk_strings() {
        let encoded = encode_resp_command(&strings(&["SET", "bench", "id-1"]));
        assert_eq!(encoded, b"*3\r\n$3\r\nSET\r\n$5\r\nbench\r\n$4\r\nid-1\r\n");
    }

    #[tokio::test]
    async fn resp_parser_reads_core_response_types() {
        let (mut writer, reader) = tokio::io::duplex(256);
        writer
            .write_all(b"+OK\r\n:42\r\n$3\r\nhey\r\n*2\r\n+OK\r\n$5\r\nworld\r\n-ERR no\r\n")
            .await
            .unwrap();
        drop(writer);

        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_resp_value(&mut reader).await.unwrap(),
            RespValue::Simple("OK".to_owned())
        );
        assert_eq!(
            read_resp_value(&mut reader).await.unwrap(),
            RespValue::Integer(42)
        );
        assert_eq!(
            read_resp_value(&mut reader).await.unwrap(),
            RespValue::Bulk(Some(b"hey".to_vec()))
        );
        assert_eq!(
            read_resp_value(&mut reader).await.unwrap(),
            RespValue::Array(Some(vec![
                RespValue::Simple("OK".to_owned()),
                RespValue::Bulk(Some(b"world".to_vec()))
            ]))
        );
        assert_eq!(
            read_resp_value(&mut reader).await.unwrap(),
            RespValue::Error("ERR no".to_owned())
        );
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }
}
