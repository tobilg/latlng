#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use latlng_storage::{
    CompactionResult, StorageBackend, StorageEntry, StorageError, StorageResult,
    assert_strictly_increasing, replay_entries,
};
use serde::{Deserialize, Serialize};

pub use latlng_storage as storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AofOptions {
    pub writer_queue_limit: usize,
    pub max_group_commit_delay_ms: u64,
    pub max_requests_per_commit_cycle: usize,
    #[cfg(test)]
    pub fail_write_and_sync_stage: Option<u8>,
}

impl Default for AofOptions {
    fn default() -> Self {
        Self {
            writer_queue_limit: 4_096,
            max_group_commit_delay_ms: 1,
            max_requests_per_commit_cycle: 128,
            #[cfg(test)]
            fail_write_and_sync_stage: None,
        }
    }
}

pub struct AofBackend {
    path: PathBuf,
    options: AofOptions,
    committed: Arc<Mutex<CommittedState>>,
    send_gate: Mutex<()>,
    writer: Mutex<WriterHandle>,
}

impl std::fmt::Debug for AofBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AofBackend")
            .field("path", &self.path)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct CommittedState {
    entries: Vec<StorageEntry>,
    closed: bool,
}

#[derive(Debug)]
struct WriterHandle {
    sender: SyncSender<WriterCommand>,
    join: Option<JoinHandle<()>>,
    closed: bool,
}

#[derive(Debug)]
struct WriterRuntime {
    writer: BufWriter<File>,
    durable_len: u64,
    #[cfg(test)]
    fail_write_and_sync_stage: Option<u8>,
}

#[derive(Debug)]
enum WriterCommand {
    Append(AppendRequest),
    Snapshot {
        entries: Vec<StorageEntry>,
        response: ResponseSender<()>,
    },
    Compact {
        response: ResponseSender<CompactionResult>,
    },
    Close {
        response: ResponseSender<()>,
    },
}

#[derive(Debug)]
struct AppendRequest {
    frame: AofFrame,
    entries: Vec<StorageEntry>,
    response: ResponseSender<()>,
}

type ResponseSender<T> = mpsc::Sender<StorageResult<T>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AofFrame {
    Entry(StorageEntry),
    Batch(Vec<StorageEntry>),
    Group(Vec<AofFrame>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AofIntegrityReport {
    pub path: PathBuf,
    pub entry_count: u64,
    pub first_sequence: Option<u64>,
    pub last_sequence: u64,
    pub durable_prefix_bytes: u64,
    pub total_bytes: u64,
    pub truncated_tail: bool,
    pub checksum_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AofBackupReport {
    pub backup_path: PathBuf,
    pub source_path: PathBuf,
    pub entry_count: u64,
    pub last_sequence: u64,
    pub checksum_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AofRestoreReport {
    pub backup_path: PathBuf,
    pub restored_path: PathBuf,
    pub entry_count: u64,
    pub last_sequence: u64,
    pub checksum_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AofBackupFile {
    format: String,
    version: String,
    created_unix_seconds: u64,
    source_path: PathBuf,
    entry_count: u64,
    first_sequence: Option<u64>,
    last_sequence: u64,
    checksum_hex: String,
    entries: Vec<StorageEntry>,
}

const AOF_BACKUP_FORMAT: &str = "latlng-aof-backup-v1";

impl AofBackend {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        Self::open_with_options(path, AofOptions::default())
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: AofOptions) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let options = sanitize_options(options);
        let entries = load_entries(&path)?;
        let file = reopen_writer_file(&path)?;
        let durable_len = current_size(&path)?;
        let committed = Arc::new(Mutex::new(CommittedState {
            entries,
            closed: false,
        }));
        let (sender, receiver) = mpsc::sync_channel(options.writer_queue_limit);
        let writer_state = Arc::clone(&committed);
        let thread_path = path.clone();
        let join = thread::Builder::new()
            .name("latlng-aof-writer".to_owned())
            .spawn(move || {
                run_writer_loop(
                    thread_path,
                    WriterRuntime {
                        writer: file,
                        durable_len,
                        #[cfg(test)]
                        fail_write_and_sync_stage: options.fail_write_and_sync_stage,
                    },
                    options,
                    writer_state,
                    receiver,
                )
            })
            .map_err(|error| StorageError::Message(error.to_string()))?;

        Ok(Self {
            path,
            options,
            committed,
            send_gate: Mutex::new(()),
            writer: Mutex::new(WriterHandle {
                sender,
                join: Some(join),
                closed: false,
            }),
        })
    }

    fn submit<T>(&self, make: impl FnOnce(ResponseSender<T>) -> WriterCommand) -> StorageResult<T> {
        let _gate = self
            .send_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sender = {
            let writer = self
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if writer.closed {
                return Err(StorageError::Closed);
            }
            writer.sender.clone()
        };
        let (response_tx, response_rx) = mpsc::channel();
        sender
            .send(make(response_tx))
            .map_err(|_| StorageError::Closed)?;
        drop(_gate);
        response_rx.recv().map_err(|_| StorageError::Closed)?
    }
}

impl StorageBackend for AofBackend {
    fn append(&self, entry: &StorageEntry) -> StorageResult<()> {
        let entry = entry.clone();
        self.submit(move |response| {
            WriterCommand::Append(AppendRequest {
                frame: AofFrame::Entry(entry.clone()),
                entries: vec![entry],
                response,
            })
        })
    }

    fn append_batch(&self, entries: &[StorageEntry]) -> StorageResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        assert_strictly_increasing(entries)?;
        let entries = entries.to_vec();
        self.submit(move |response| {
            WriterCommand::Append(AppendRequest {
                frame: AofFrame::Batch(entries.clone()),
                entries,
                response,
            })
        })
    }

    fn replay(
        &self,
        after_seq: u64,
        callback: &mut dyn FnMut(StorageEntry) -> StorageResult<()>,
    ) -> StorageResult<u64> {
        let entries = self
            .committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .clone();
        replay_entries(&entries, after_seq, callback)
    }

    fn snapshot(&self, entries: Vec<StorageEntry>) -> StorageResult<()> {
        assert_strictly_increasing(&entries)?;
        self.submit(move |response| WriterCommand::Snapshot { entries, response })
    }

    fn compact(&self) -> StorageResult<CompactionResult> {
        self.submit(move |response| WriterCommand::Compact { response })
    }

    fn last_sequence(&self) -> StorageResult<u64> {
        Ok(self
            .committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(0))
    }

    fn checksum(&self, from: u64, to: u64) -> StorageResult<[u8; 16]> {
        let entries = self
            .committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .iter()
            .filter(|entry| entry.sequence >= from && entry.sequence <= to)
            .cloned()
            .collect::<Vec<_>>();
        Ok(StorageEntry::checksum(&entries))
    }

    fn close(&self) -> StorageResult<()> {
        let response = {
            let _gate = self
                .send_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let sender = {
                let mut writer = self
                    .writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if writer.closed {
                    return Ok(());
                }
                writer.closed = true;
                writer.sender.clone()
            };
            let (response_tx, response_rx) = mpsc::channel();
            sender
                .send(WriterCommand::Close {
                    response: response_tx,
                })
                .map_err(|_| StorageError::Closed)?;
            drop(_gate);
            response_rx.recv().map_err(|_| StorageError::Closed)?
        };

        let join = {
            self.writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .join
                .take()
        };
        if let Some(join) = join {
            join.join()
                .map_err(|_| StorageError::Message("aof writer thread panicked".to_owned()))?;
        }
        response
    }
}

fn run_writer_loop(
    path: PathBuf,
    runtime: WriterRuntime,
    options: AofOptions,
    committed: Arc<Mutex<CommittedState>>,
    receiver: Receiver<WriterCommand>,
) {
    let mut runtime = runtime;
    let mut pending = VecDeque::new();

    loop {
        let command = if let Some(command) = pending.pop_front() {
            command
        } else {
            match receiver.recv() {
                Ok(command) => command,
                Err(_) => break,
            }
        };

        match command {
            WriterCommand::Append(first) => {
                let requests = collect_append_requests(first, &receiver, &mut pending, options);
                let result = commit_append_group(&path, &mut runtime, &committed, &requests);
                respond_append_requests(requests, result);
            }
            WriterCommand::Snapshot { entries, response } => {
                let result = execute_snapshot(&path, &mut runtime, &committed, entries);
                let _ = response.send(result);
            }
            WriterCommand::Compact { response } => {
                let result = execute_compact(&path, &mut runtime, &committed);
                let _ = response.send(result);
            }
            WriterCommand::Close { response } => {
                let result = execute_close(&mut runtime, &committed);
                let _ = response.send(result);
                break;
            }
        }
    }
}

fn collect_append_requests(
    first: AppendRequest,
    receiver: &Receiver<WriterCommand>,
    pending: &mut VecDeque<WriterCommand>,
    options: AofOptions,
) -> Vec<AppendRequest> {
    let mut requests = vec![first];
    let max_requests = options.max_requests_per_commit_cycle.max(1);
    let delay = Duration::from_millis(options.max_group_commit_delay_ms);
    let deadline = Instant::now() + delay;

    while requests.len() < max_requests {
        let next = if let Some(command) = pending.pop_front() {
            Some(command)
        } else if delay.is_zero() {
            receiver.try_recv().ok()
        } else {
            let now = Instant::now();
            if now >= deadline {
                None
            } else {
                match receiver.recv_timeout(deadline.saturating_duration_since(now)) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => None,
                }
            }
        };

        match next {
            Some(WriterCommand::Append(request)) => requests.push(request),
            Some(other) => {
                pending.push_front(other);
                break;
            }
            None => break,
        }
    }

    requests
}

fn commit_append_group(
    path: &Path,
    runtime: &mut WriterRuntime,
    committed: &Arc<Mutex<CommittedState>>,
    requests: &[AppendRequest],
) -> StorageResult<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let last_entry = {
        let state = committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return Err(StorageError::Closed);
        }
        state.entries.last().cloned()
    };

    let mut last = last_entry;
    for request in requests {
        validate_batch_against_last(last.as_ref(), &request.entries)?;
        last = request.entries.last().cloned();
    }

    let frame = if requests.len() == 1 {
        requests[0].frame.clone()
    } else {
        AofFrame::Group(
            requests
                .iter()
                .map(|request| request.frame.clone())
                .collect(),
        )
    };

    let start_len = runtime.durable_len;
    if let Err(error) = write_and_sync_frame(runtime, &frame) {
        let _ = rollback_file(path, start_len);
        if let Ok(reopened) = reopen_writer_runtime(path) {
            *runtime = reopened;
        }
        return Err(error);
    }
    runtime.durable_len = start_len.saturating_add(encoded_frame_len(&frame));

    let mut state = committed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for request in requests {
        state.entries.extend(request.entries.iter().cloned());
    }
    Ok(())
}

fn execute_snapshot(
    path: &Path,
    runtime: &mut WriterRuntime,
    committed: &Arc<Mutex<CommittedState>>,
    entries: Vec<StorageEntry>,
) -> StorageResult<()> {
    assert_strictly_increasing(&entries)?;
    rewrite(path, &entries)?;
    *runtime = reopen_writer_runtime(path)?;
    let mut state = committed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.closed {
        return Err(StorageError::Closed);
    }
    state.entries = entries;
    Ok(())
}

fn execute_compact(
    path: &Path,
    runtime: &mut WriterRuntime,
    committed: &Arc<Mutex<CommittedState>>,
) -> StorageResult<CompactionResult> {
    let entries = committed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entries
        .clone();
    let before_bytes = current_size(path)?;
    rewrite(path, &entries)?;
    *runtime = reopen_writer_runtime(path)?;
    let after_bytes = current_size(path)?;
    Ok(CompactionResult {
        before_entries: entries.len() as u64,
        after_entries: entries.len() as u64,
        before_bytes,
        after_bytes,
    })
}

fn execute_close(
    runtime: &mut WriterRuntime,
    committed: &Arc<Mutex<CommittedState>>,
) -> StorageResult<()> {
    runtime.writer.flush()?;
    runtime.writer.get_ref().sync_all()?;
    runtime.durable_len = runtime.writer.get_ref().metadata()?.len();
    committed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .closed = true;
    Ok(())
}

fn respond_append_requests(requests: Vec<AppendRequest>, result: StorageResult<()>) {
    for request in requests {
        let _ = request.response.send(result.clone());
    }
}

fn sanitize_options(options: AofOptions) -> AofOptions {
    AofOptions {
        writer_queue_limit: options.writer_queue_limit.max(1),
        max_group_commit_delay_ms: options.max_group_commit_delay_ms,
        max_requests_per_commit_cycle: options.max_requests_per_commit_cycle.max(1),
        #[cfg(test)]
        fail_write_and_sync_stage: options.fail_write_and_sync_stage,
    }
}

fn reopen_writer_file(path: &Path) -> StorageResult<BufWriter<File>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    Ok(BufWriter::new(file))
}

fn reopen_writer_runtime(path: &Path) -> StorageResult<WriterRuntime> {
    let writer = reopen_writer_file(path)?;
    let durable_len = writer.get_ref().metadata()?.len();
    Ok(WriterRuntime {
        writer,
        durable_len,
        #[cfg(test)]
        fail_write_and_sync_stage: None,
    })
}

fn write_and_sync_frame(runtime: &mut WriterRuntime, frame: &AofFrame) -> StorageResult<()> {
    write_frame(&mut runtime.writer, frame)?;
    #[cfg(test)]
    maybe_fail_write_and_sync(runtime, 1)?;
    runtime.writer.flush()?;
    #[cfg(test)]
    maybe_fail_write_and_sync(runtime, 2)?;
    runtime.writer.get_ref().sync_all()?;
    #[cfg(test)]
    maybe_fail_write_and_sync(runtime, 3)?;
    Ok(())
}

fn rollback_file(path: &Path, len: u64) -> StorageResult<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(len)?;
    file.sync_all()?;
    Ok(())
}

fn rewrite(path: &Path, entries: &[StorageEntry]) -> StorageResult<()> {
    let tmp_path = path.with_extension("rewrite");
    {
        let file = File::create(&tmp_path)?;
        let mut writer = BufWriter::new(file);
        for entry in entries {
            write_frame(&mut writer, &AofFrame::Entry(entry.clone()))?;
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn current_size(path: &Path) -> StorageResult<u64> {
    Ok(std::fs::metadata(path)?.len())
}

pub fn verify_aof(path: impl AsRef<Path>) -> StorageResult<AofIntegrityReport> {
    let (_, report) = load_entries_with_report(path.as_ref())?;
    Ok(report)
}

pub fn backup_aof(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> StorageResult<AofBackupReport> {
    let source = source.as_ref();
    let destination = destination.as_ref();
    let (entries, report) = load_entries_with_report(source)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let backup = AofBackupFile {
        format: AOF_BACKUP_FORMAT.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        created_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        source_path: source.to_path_buf(),
        entry_count: report.entry_count,
        first_sequence: report.first_sequence,
        last_sequence: report.last_sequence,
        checksum_hex: report.checksum_hex.clone(),
        entries,
    };
    let file = File::create(destination)?;
    serde_json::to_writer_pretty(file, &backup)
        .map_err(|error| StorageError::Codec(error.to_string()))?;
    Ok(AofBackupReport {
        backup_path: destination.to_path_buf(),
        source_path: source.to_path_buf(),
        entry_count: report.entry_count,
        last_sequence: report.last_sequence,
        checksum_hex: report.checksum_hex,
    })
}

pub fn restore_aof(
    backup_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    force: bool,
) -> StorageResult<AofRestoreReport> {
    let backup_path = backup_path.as_ref();
    let destination = destination.as_ref();
    if destination.exists() && !force {
        return Err(StorageError::Message(format!(
            "refusing to overwrite existing AOF: {}",
            destination.display()
        )));
    }
    let file = File::open(backup_path)?;
    let backup: AofBackupFile =
        serde_json::from_reader(file).map_err(|error| StorageError::Codec(error.to_string()))?;
    if backup.format != AOF_BACKUP_FORMAT {
        return Err(StorageError::Codec(format!(
            "unsupported AOF backup format: {}",
            backup.format
        )));
    }
    assert_strictly_increasing(&backup.entries)?;
    let checksum_hex = checksum_hex(&StorageEntry::checksum(&backup.entries));
    if checksum_hex != backup.checksum_hex {
        return Err(StorageError::Codec(
            "AOF backup checksum does not match entries".to_owned(),
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rewrite(destination, &backup.entries)?;
    let restored = verify_aof(destination)?;
    if restored.checksum_hex != backup.checksum_hex || restored.entry_count != backup.entry_count {
        return Err(StorageError::Codec(
            "restored AOF failed post-write validation".to_owned(),
        ));
    }
    Ok(AofRestoreReport {
        backup_path: backup_path.to_path_buf(),
        restored_path: destination.to_path_buf(),
        entry_count: restored.entry_count,
        last_sequence: restored.last_sequence,
        checksum_hex: restored.checksum_hex,
    })
}

fn validate_batch_against_last(
    last: Option<&StorageEntry>,
    entries: &[StorageEntry],
) -> StorageResult<()> {
    if entries.is_empty() {
        return Ok(());
    }
    assert_strictly_increasing(entries)?;
    if let Some(last) = last
        && let Some(first) = entries.first()
        && first.sequence <= last.sequence
    {
        return Err(StorageError::SequenceRegression {
            expected: last.sequence + 1,
            actual: first.sequence,
        });
    }
    Ok(())
}

fn encoded_frame_len(frame: &AofFrame) -> u64 {
    8 + bincode::serialize(frame)
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
fn maybe_fail_write_and_sync(runtime: &mut WriterRuntime, stage: u8) -> StorageResult<()> {
    if runtime.fail_write_and_sync_stage == Some(stage) {
        runtime.fail_write_and_sync_stage = None;
        return Err(StorageError::Io(format!(
            "injected write_and_sync failure at stage {stage}"
        )));
    }
    Ok(())
}

fn write_frame(writer: &mut BufWriter<File>, frame: &AofFrame) -> StorageResult<()> {
    let payload =
        bincode::serialize(frame).map_err(|error| StorageError::Codec(error.to_string()))?;
    writer.write_all(&(payload.len() as u64).to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(())
}

fn load_entries(path: &Path) -> StorageResult<Vec<StorageEntry>> {
    Ok(load_entries_with_report(path)?.0)
}

fn load_entries_with_report(path: &Path) -> StorageResult<(Vec<StorageEntry>, AofIntegrityReport)> {
    if !path.exists() {
        return Ok((
            Vec::new(),
            AofIntegrityReport {
                path: path.to_path_buf(),
                entry_count: 0,
                first_sequence: None,
                last_sequence: 0,
                durable_prefix_bytes: 0,
                total_bytes: 0,
                truncated_tail: false,
                checksum_hex: checksum_hex(&StorageEntry::checksum(&[])),
            },
        ));
    }

    let file = File::open(path)?;
    let total_bytes = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut offset = 0_u64;
    let mut durable_prefix_bytes = 0_u64;
    let mut truncated_tail = false;

    'frames: loop {
        let frame_start = offset;
        let mut len_buf = [0_u8; 8];
        let mut len_read = 0_usize;
        while len_read < len_buf.len() {
            let read = reader.read(&mut len_buf[len_read..])?;
            if read == 0 {
                if len_read > 0 {
                    truncated_tail = true;
                    durable_prefix_bytes = frame_start;
                }
                break 'frames;
            }
            len_read += read;
        }
        offset += len_buf.len() as u64;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut payload = vec![0_u8; len];
        let mut payload_read = 0_usize;
        while payload_read < len {
            let read = reader.read(&mut payload[payload_read..])?;
            if read == 0 {
                truncated_tail = true;
                durable_prefix_bytes = frame_start;
                break;
            }
            payload_read += read;
        }
        if truncated_tail {
            break;
        }
        offset += len as u64;
        let frame = bincode::deserialize::<AofFrame>(&payload)
            .map_err(|error| StorageError::Codec(error.to_string()))?;
        apply_frame_to_entries(&mut entries, frame)?;
        durable_prefix_bytes = offset;
    }

    let checksum_hex = checksum_hex(&StorageEntry::checksum(&entries));
    let report = AofIntegrityReport {
        path: path.to_path_buf(),
        entry_count: entries.len() as u64,
        first_sequence: entries.first().map(|entry| entry.sequence),
        last_sequence: entries.last().map(|entry| entry.sequence).unwrap_or(0),
        durable_prefix_bytes,
        total_bytes,
        truncated_tail,
        checksum_hex,
    };
    Ok((entries, report))
}

fn checksum_hex(checksum: &[u8; 16]) -> String {
    checksum
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn apply_frame_to_entries(entries: &mut Vec<StorageEntry>, frame: AofFrame) -> StorageResult<()> {
    match frame {
        AofFrame::Entry(entry) => {
            validate_batch_against_last(entries.last(), std::slice::from_ref(&entry))?;
            entries.push(entry);
        }
        AofFrame::Batch(batch) => {
            if batch.is_empty() {
                return Err(StorageError::Codec("empty batch frame".to_owned()));
            }
            validate_batch_against_last(entries.last(), &batch)?;
            entries.extend(batch);
        }
        AofFrame::Group(frames) => {
            if frames.is_empty() {
                return Err(StorageError::Codec("empty group frame".to_owned()));
            }
            for frame in frames {
                apply_frame_to_entries(entries, frame)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    use bytes::Bytes;
    use latlng_storage::{StorageBackend, StorageEntry, StorageError};
    use tempfile::tempdir;

    use super::{
        AofBackend, AofFrame, AofOptions, backup_aof, encoded_frame_len, restore_aof, verify_aof,
    };

    fn entry(sequence: u64, command: &'static [u8]) -> StorageEntry {
        StorageEntry {
            sequence,
            timestamp_ns: sequence,
            command: Bytes::from_static(command),
        }
    }

    fn load_frames(path: &std::path::Path) -> Vec<AofFrame> {
        let mut file = OpenOptions::new().read(true).open(path).unwrap();
        let mut frames = Vec::new();
        loop {
            let mut len_buf = [0_u8; 8];
            match file.read_exact(&mut len_buf) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => panic!("unexpected frame read error: {error}"),
            }
            let len = u64::from_le_bytes(len_buf) as usize;
            let mut payload = vec![0_u8; len];
            file.read_exact(&mut payload).unwrap();
            frames.push(bincode::deserialize(&payload).unwrap());
        }
        frames
    }

    #[test]
    fn aof_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set")).unwrap();
        backend.close().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 1);
        let mut seen = 0;
        reopened
            .replay(0, &mut |entry| {
                seen = entry.sequence;
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, 1);
    }

    #[test]
    fn aof_roundtrip_preserves_atomic_batch_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend
            .append_batch(&[entry(2, b"set b"), entry(3, b"set c")])
            .unwrap();
        backend.close().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        let mut seen = Vec::new();
        reopened
            .replay(0, &mut |entry| {
                seen.push(entry.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, vec![1, 2, 3]);
    }

    #[test]
    fn concurrent_appends_can_share_one_group_commit_frame() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = Arc::new(
            AofBackend::open_with_options(
                &path,
                AofOptions {
                    writer_queue_limit: 8,
                    max_group_commit_delay_ms: 20,
                    max_requests_per_commit_cycle: 8,
                    ..AofOptions::default()
                },
            )
            .unwrap(),
        );
        let barrier = Arc::new(Barrier::new(2));

        let first_backend = Arc::clone(&backend);
        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            first_backend.append(&entry(1, b"set a")).unwrap();
        });
        let second_backend = Arc::clone(&backend);
        let second_barrier = Arc::clone(&barrier);
        let second = thread::spawn(move || {
            second_barrier.wait();
            thread::sleep(Duration::from_millis(1));
            second_backend.append(&entry(2, b"set b")).unwrap();
        });

        first.join().unwrap();
        second.join().unwrap();
        backend.close().unwrap();

        let frames = load_frames(&path);
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            AofFrame::Group(group) => {
                assert_eq!(group.len(), 2);
            }
            other => panic!("expected grouped frame, got {other:?}"),
        }
    }

    #[test]
    fn aof_ignores_truncated_tail_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set")).unwrap();
        backend.close().unwrap();

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&16_u64.to_le_bytes()).unwrap();
        file.write_all(&[1, 2, 3, 4]).unwrap();
        file.flush().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 1);
        let mut seen = Vec::new();
        reopened
            .replay(0, &mut |entry| {
                seen.push(entry.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, vec![1]);

        let report = verify_aof(&path).unwrap();
        assert_eq!(report.entry_count, 1);
        assert_eq!(report.last_sequence, 1);
        assert!(report.truncated_tail);
        assert!(report.durable_prefix_bytes < report.total_bytes);
    }

    #[test]
    fn aof_ignores_truncated_tail_batch_as_one_unit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set")).unwrap();
        backend
            .append_batch(&[entry(2, b"set b"), entry(3, b"set c")])
            .unwrap();
        backend.close().unwrap();

        let truncate_len = encoded_frame_len(&AofFrame::Batch(vec![
            entry(2, b"set b"),
            entry(3, b"set c"),
        ]));
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        let full_len = file.metadata().unwrap().len();
        file.set_len(full_len - truncate_len + 4).unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        let mut seen = Vec::new();
        reopened
            .replay(0, &mut |entry| {
                seen.push(entry.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, vec![1]);
    }

    #[test]
    fn aof_rejects_corrupt_payloads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.write_all(&4_u64.to_le_bytes()).unwrap();
        file.write_all(&[0xde, 0xad, 0xbe, 0xef]).unwrap();
        file.flush().unwrap();

        match AofBackend::open(&path) {
            Err(StorageError::Codec(_)) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn verify_aof_reports_valid_prefix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend.append(&entry(2, b"set b")).unwrap();
        backend.close().unwrap();

        let report = verify_aof(&path).unwrap();
        assert_eq!(report.entry_count, 2);
        assert_eq!(report.first_sequence, Some(1));
        assert_eq!(report.last_sequence, 2);
        assert_eq!(report.durable_prefix_bytes, report.total_bytes);
        assert!(!report.truncated_tail);
    }

    #[test]
    fn backup_and_restore_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backup = dir.path().join("backup.json");
        let restored = dir.path().join("restored.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend.append(&entry(2, b"set b")).unwrap();
        backend.close().unwrap();

        let backup_report = backup_aof(&path, &backup).unwrap();
        assert_eq!(backup_report.entry_count, 2);

        let restore_report = restore_aof(&backup, &restored, false).unwrap();
        assert_eq!(restore_report.entry_count, 2);
        assert_eq!(restore_report.checksum_hex, backup_report.checksum_hex);

        let reopened = AofBackend::open(&restored).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 2);
    }

    #[test]
    fn restore_refuses_to_overwrite_without_force() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backup = dir.path().join("backup.json");
        let restored = dir.path().join("restored.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend.close().unwrap();
        backup_aof(&path, &backup).unwrap();
        std::fs::write(&restored, b"existing").unwrap();

        let error = restore_aof(&backup, &restored, false).unwrap_err();
        assert!(matches!(error, StorageError::Message(_)));

        restore_aof(&backup, &restored, true).unwrap();
        assert_eq!(verify_aof(&restored).unwrap().last_sequence, 1);
    }

    #[test]
    fn snapshot_rewrites_aof_contents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend.append(&entry(2, b"set b")).unwrap();

        backend.snapshot(vec![entry(2, b"set b")]).unwrap();
        backend.close().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        let mut sequences = Vec::new();
        reopened
            .replay(0, &mut |entry| {
                sequences.push(entry.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(sequences, vec![2]);
        assert_eq!(reopened.last_sequence().unwrap(), 2);
    }

    #[test]
    fn compact_waits_for_queued_appends() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = Arc::new(
            AofBackend::open_with_options(
                &path,
                AofOptions {
                    writer_queue_limit: 8,
                    max_group_commit_delay_ms: 50,
                    max_requests_per_commit_cycle: 8,
                    ..AofOptions::default()
                },
            )
            .unwrap(),
        );

        let appender = {
            let backend = Arc::clone(&backend);
            thread::spawn(move || {
                backend.append(&entry(1, b"set a")).unwrap();
            })
        };
        thread::sleep(Duration::from_millis(5));
        let result = backend.compact().unwrap();
        appender.join().unwrap();
        backend.close().unwrap();

        assert!(result.after_entries >= 1);
        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 1);
    }

    #[test]
    fn close_flushes_queued_writes_before_returning() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = Arc::new(
            AofBackend::open_with_options(
                &path,
                AofOptions {
                    writer_queue_limit: 8,
                    max_group_commit_delay_ms: 50,
                    max_requests_per_commit_cycle: 8,
                    ..AofOptions::default()
                },
            )
            .unwrap(),
        );

        let appender = {
            let backend = Arc::clone(&backend);
            thread::spawn(move || {
                backend.append(&entry(1, b"set a")).unwrap();
            })
        };
        thread::sleep(Duration::from_millis(5));
        backend.close().unwrap();
        appender.join().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 1);
    }

    #[test]
    fn queue_backpressure_does_not_deadlock() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = Arc::new(
            AofBackend::open_with_options(
                &path,
                AofOptions {
                    writer_queue_limit: 1,
                    max_group_commit_delay_ms: 5,
                    max_requests_per_commit_cycle: 2,
                    ..AofOptions::default()
                },
            )
            .unwrap(),
        );

        let mut workers = Vec::new();
        for sequence in 1..=3 {
            let backend = Arc::clone(&backend);
            workers.push(thread::spawn(move || {
                thread::sleep(Duration::from_millis(sequence - 1));
                backend
                    .append(&entry(sequence, b"set load"))
                    .expect("append should succeed under backpressure");
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        backend.close().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 3);
    }

    #[test]
    fn failed_group_append_rolls_back_to_last_durable_prefix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend.close().unwrap();

        let backend = AofBackend::open_with_options(
            &path,
            AofOptions {
                fail_write_and_sync_stage: Some(2),
                ..AofOptions::default()
            },
        )
        .unwrap();
        let error = backend
            .append_batch(&[entry(2, b"set b"), entry(3, b"set c")])
            .unwrap_err();
        assert!(matches!(error, StorageError::Io(_)));
        backend.close().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        let mut seen = Vec::new();
        reopened
            .replay(0, &mut |entry| {
                seen.push(entry.sequence);
                Ok(())
            })
            .unwrap();
        assert_eq!(seen, vec![1]);
    }

    #[test]
    fn snapshot_and_compact_preserve_committed_prefix_after_failed_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = AofBackend::open(&path).unwrap();
        backend.append(&entry(1, b"set a")).unwrap();
        backend.close().unwrap();

        let backend = AofBackend::open_with_options(
            &path,
            AofOptions {
                fail_write_and_sync_stage: Some(2),
                ..AofOptions::default()
            },
        )
        .unwrap();
        let _ = backend.append(&entry(2, b"set b"));
        backend.snapshot(vec![entry(1, b"set a")]).unwrap();
        let result = backend.compact().unwrap();
        backend.close().unwrap();

        assert_eq!(result.after_entries, 1);
        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 1);
    }

    #[test]
    fn concurrent_failed_group_append_preserves_last_durable_prefix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("appendonly.aof");
        let backend = Arc::new(
            AofBackend::open_with_options(
                &path,
                AofOptions {
                    writer_queue_limit: 8,
                    max_group_commit_delay_ms: 20,
                    max_requests_per_commit_cycle: 8,
                    fail_write_and_sync_stage: Some(2),
                },
            )
            .unwrap(),
        );
        let barrier = Arc::new(Barrier::new(2));

        let first_backend = Arc::clone(&backend);
        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            first_backend.append(&entry(1, b"set a"))
        });
        let second_backend = Arc::clone(&backend);
        let second_barrier = Arc::clone(&barrier);
        let second = thread::spawn(move || {
            second_barrier.wait();
            thread::sleep(Duration::from_millis(1));
            second_backend.append(&entry(2, b"set b"))
        });

        let first_result = first.join().unwrap();
        let second_result = second.join().unwrap();
        assert!(matches!(first_result, Err(StorageError::Io(_))));
        assert!(matches!(second_result, Err(StorageError::Io(_))));
        backend.close().unwrap();

        let reopened = AofBackend::open(&path).unwrap();
        assert_eq!(reopened.last_sequence().unwrap(), 0);
    }
}
