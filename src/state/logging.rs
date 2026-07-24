use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::reload;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;

use crate::state::debug_settings::LogLevel;
use crate::state::{paths, storage_security};

const LOG_QUEUE_CAPACITY: usize = 2_048;
const MAX_LOG_RECORD_BYTES: usize = 256 * 1024;
const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_LOG_FILE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const LOG_GENERATIONS: usize = 3;
const VERBOSE_HOT_PATH_SAMPLE_RATE: u64 = 16;

static LOG_CONTROL: OnceLock<LogControl> = OnceLock::new();
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static SECRET_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
static BEARER_TOKEN: OnceLock<Regex> = OnceLock::new();
static AUTHORIZATION_HEADER: OnceLock<Regex> = OnceLock::new();
static SENSITIVE_CONTENT_FIELD: OnceLock<Regex> = OnceLock::new();
static UNIX_LOCAL_PATH: OnceLock<Regex> = OnceLock::new();
static WINDOWS_LOCAL_PATH: OnceLock<Regex> = OnceLock::new();
static HOT_PATH_SAMPLE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct LogControl {
    sender: SyncSender<LogMessage>,
    enabled: AtomicBool,
    dropped: AtomicU64,
}

enum LogMessage {
    Record(Vec<u8>),
    Flush,
}

fn control() -> &'static LogControl {
    LOG_CONTROL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(LOG_QUEUE_CAPACITY);
        let path = log_file_path();
        thread::Builder::new()
            .name("awayuki-log-writer".into())
            .spawn(move || log_worker(receiver, path))
            .expect("failed to start the bounded log writer");
        LogControl {
            sender,
            enabled: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }
    })
}

/// Register the EnvFilter reload handle returned by `reload::Layer::new` so the
/// rest of the app can swap log levels at runtime.
pub fn set_filter_handle(handle: reload::Handle<EnvFilter, Registry>) {
    let _ = FILTER_HANDLE.set(handle);
}

/// Replace the active EnvFilter with one targeting `awayuki` at the given level.
pub fn set_log_level(level: LogLevel) {
    let Some(handle) = FILTER_HANDLE.get() else {
        return;
    };
    let filter = EnvFilter::new(level.directive());
    if let Err(e) = handle.reload(filter) {
        tracing::error!(error = %e, "Failed to reload log filter");
    }
}

pub fn log_file_path() -> PathBuf {
    paths::log_file_path()
}

pub fn enable() -> io::Result<()> {
    let path = log_file_path();
    // Validate and harden the destination synchronously so the caller can
    // surface failures. The worker reopens it lazily on its next record.
    drop(storage_security::open_private_append(&path)?);
    control().enabled.store(true, Ordering::Release);
    Ok(())
}

pub fn disable() {
    let control = control();
    control.enabled.store(false, Ordering::Release);
    let _ = control.sender.try_send(LogMessage::Flush);
}

pub fn dropped_records() -> u64 {
    control().dropped.load(Ordering::Relaxed)
}

/// Open the log file in the user's preferred application.
///
/// Creates an empty file first if it does not yet exist so the OS open call
/// has something to launch.
pub fn open_in_default_app() -> io::Result<()> {
    let path = log_file_path();
    if !path.exists() {
        storage_security::open_private_append(&path)?;
    } else {
        storage_security::set_private_file_permissions(&path)?;
    }
    open::that(&path).map_err(|e| io::Error::other(e.to_string()))
}

#[derive(Clone, Default)]
pub struct LogFileMakeWriter;

impl<'a> MakeWriter<'a> for LogFileMakeWriter {
    type Writer = LogFileGuard;

    fn make_writer(&'a self) -> Self::Writer {
        LogFileGuard
    }
}

pub struct LogFileGuard;

impl Write for LogFileGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let control = control();
        if !control.enabled.load(Ordering::Acquire) {
            return Ok(buf.len());
        }

        let limited = &buf[..buf.len().min(MAX_LOG_RECORD_BYTES)];
        let sample_sequence = if is_verbose_hot_path(limited) {
            HOT_PATH_SAMPLE_COUNTER.fetch_add(1, Ordering::Relaxed)
        } else {
            0
        };
        if !should_keep_verbose_hot_path(limited, sample_sequence) {
            return Ok(buf.len());
        }
        let mut record = redact_log_record(limited).into_bytes();
        if buf.len() > MAX_LOG_RECORD_BYTES {
            record.extend_from_slice(b"...[record truncated]\n");
        }

        match control.sender.try_send(LogMessage::Record(record)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                control.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = control().sender.try_send(LogMessage::Flush);
        Ok(())
    }
}

fn should_keep_verbose_hot_path(record: &[u8], sequence: u64) -> bool {
    !is_verbose_hot_path(record) || sequence.is_multiple_of(VERBOSE_HOT_PATH_SAMPLE_RATE)
}

fn is_verbose_hot_path(record: &[u8]) -> bool {
    let text = String::from_utf8_lossy(record);
    let verbose = text.contains(" DEBUG ") || text.contains(" TRACE ");
    let hot_path = ["stream", "ipc", "tauri-command", "ui-timeline"]
        .iter()
        .any(|marker| text.contains(marker));
    verbose && hot_path
}

struct RotatingLog {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    bytes_written: u64,
    opened_at: Instant,
    max_file_bytes: u64,
    max_file_age: Duration,
    generations: usize,
}

impl RotatingLog {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            writer: None,
            bytes_written: 0,
            opened_at: Instant::now(),
            max_file_bytes: MAX_LOG_FILE_BYTES,
            max_file_age: MAX_LOG_FILE_AGE,
            generations: LOG_GENERATIONS,
        }
    }

    #[cfg(test)]
    fn with_limits(
        path: PathBuf,
        max_file_bytes: u64,
        max_file_age: Duration,
        generations: usize,
    ) -> Self {
        Self {
            path,
            writer: None,
            bytes_written: 0,
            opened_at: Instant::now(),
            max_file_bytes,
            max_file_age,
            generations,
        }
    }

    fn write_record(&mut self, record: &[u8]) -> io::Result<()> {
        self.ensure_open()?;
        if self.bytes_written > 0
            && (self.bytes_written.saturating_add(record.len() as u64) > self.max_file_bytes
                || self.opened_at.elapsed() >= self.max_file_age)
        {
            self.rotate()?;
            self.ensure_open()?;
        }
        if let Some(writer) = &mut self.writer {
            writer.write_all(record)?;
            self.bytes_written = self.bytes_written.saturating_add(record.len() as u64);
        }
        Ok(())
    }

    fn flush(&mut self) {
        if let Some(writer) = &mut self.writer {
            let _ = writer.flush();
        }
    }

    fn ensure_open(&mut self) -> io::Result<()> {
        if self.writer.is_none() {
            let file = storage_security::open_private_append(&self.path)?;
            self.bytes_written = file.metadata()?.len();
            self.writer = Some(BufWriter::with_capacity(64 * 1024, file));
            self.opened_at = Instant::now();
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush();
        self.writer = None;

        let oldest = rotated_path(&self.path, self.generations);
        if oldest.exists() {
            fs::remove_file(oldest)?;
        }
        for generation in (1..self.generations).rev() {
            let source = rotated_path(&self.path, generation);
            if source.exists() {
                fs::rename(source, rotated_path(&self.path, generation + 1))?;
            }
        }
        if self.path.exists() {
            fs::rename(&self.path, rotated_path(&self.path, 1))?;
        }
        self.bytes_written = 0;
        Ok(())
    }
}

fn log_worker(receiver: Receiver<LogMessage>, path: PathBuf) {
    let mut log = RotatingLog::new(path);
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(LogMessage::Record(record)) => {
                if let Err(error) = log.write_record(&record) {
                    eprintln!("Awayuki file logging failed: {error}");
                }
            }
            Ok(LogMessage::Flush) => log.flush(),
            Err(mpsc::RecvTimeoutError::Timeout) => log.flush(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                log.flush();
                break;
            }
        }
    }
}

fn rotated_path(path: &Path, generation: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{generation}"));
    PathBuf::from(name)
}

fn redact_log_record(record: &[u8]) -> String {
    let text = String::from_utf8_lossy(record);
    let assignment = SECRET_ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(access[_-]?token|refresh[_-]?token|password|client[_-]?secret|credential|oauth[_-]?(?:code|state)|code|state)(\s*[:=]\s*)([^\s&,"'}]+)"#,
        )
        .expect("valid secret redaction regex")
    });
    let bearer = BEARER_TOKEN.get_or_init(|| {
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+").expect("valid bearer redaction regex")
    });
    let authorization = AUTHORIZATION_HEADER.get_or_init(|| {
        Regex::new(r"(?i)\bAuthorization\s*[:=]\s*[^\r\n]+")
            .expect("valid authorization header redaction regex")
    });
    let content_field = SENSITIVE_CONTENT_FIELD.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(content|spoiler[_-]?text|notification[_-]?body|post[_-]?body|status[_-]?text)(\s*[:=]\s*)(?:\"(?:\\.|[^\"])*\"|[^\s,}]+)"#,
        )
        .expect("valid content redaction regex")
    });
    let unix_path = UNIX_LOCAL_PATH.get_or_init(|| {
        Regex::new(r#"/(?:Users|home|private|var|tmp)/[^\s"']+"#)
            .expect("valid Unix path redaction regex")
    });
    let windows_path = WINDOWS_LOCAL_PATH.get_or_init(|| {
        Regex::new(r#"(?i)\b[A-Z]:\\(?:Users|Temp|Windows)\\[^\s"']+"#)
            .expect("valid Windows path redaction regex")
    });
    let without_authorization = authorization.replace_all(&text, "Authorization: [redacted]");
    let without_bearer = bearer.replace_all(&without_authorization, "Bearer [redacted]");
    let without_assignments = assignment.replace_all(&without_bearer, "$1$2[redacted]");
    let without_content = content_field.replace_all(&without_assignments, "$1$2[redacted]");
    let without_unix_paths = unix_path.replace_all(&without_content, "[local-path]");
    windows_path
        .replace_all(&without_unix_paths, "[local-path]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::hint::black_box;

    const LOGGING_BENCHMARK_EVENTS: usize = 16_384;

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LoggingBenchmarkSample {
        enabled: bool,
        events: usize,
        retained_records: usize,
        dropped_records: u64,
        producer_p95_ms: f64,
        throughput_events_per_second: f64,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LoggingBenchmarkReport {
        schema_version: u32,
        fixture_id: &'static str,
        sample_rate: u64,
        off: LoggingBenchmarkSample,
        on: LoggingBenchmarkSample,
    }

    #[test]
    fn redacts_secret_assignments_and_bearer_tokens() {
        let input = br#"access_token=hunter2 code=oauth-code Authorization: Bearer abc.def
content=private-post notification_body:"private notice" /Users/alice/Awayuki/awayuki.db C:\Users\alice\awayuki.db"#;
        let output = redact_log_record(input);

        assert!(!output.contains("hunter2"));
        assert!(!output.contains("oauth-code"));
        assert!(!output.contains("abc.def"));
        assert!(!output.contains("private-post"));
        assert!(!output.contains("private notice"));
        assert!(!output.contains("alice"));
        assert!(output.contains("access_token=[redacted]"));
        assert!(output.contains("Authorization: [redacted]"));
        assert!(output.contains("[local-path]"));
    }

    #[test]
    fn rotated_names_are_stable() {
        assert_eq!(
            rotated_path(Path::new("/tmp/awayuki.log"), 2),
            PathBuf::from("/tmp/awayuki.log.2")
        );
    }

    #[test]
    fn rotation_enforces_size_age_and_generation_limits() {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-log-rotation-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("create log fixture directory");
        let path = directory.join("awayuki.log");
        let mut log = RotatingLog::with_limits(path.clone(), 12, Duration::ZERO, 2);
        for index in 0..5 {
            log.write_record(format!("record-{index}\n").as_bytes())
                .expect("write rotating fixture");
        }
        log.flush();

        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
        assert!(rotated_path(&path, 2).exists());
        assert!(!rotated_path(&path, 3).exists());
        fs::remove_dir_all(directory).expect("remove log fixture directory");
    }

    #[test]
    fn verbose_hot_paths_are_sampled_but_warnings_are_not() {
        let debug = b" DEBUG awayuki::ipc stream event";
        assert!(should_keep_verbose_hot_path(debug, 0));
        assert!(!should_keep_verbose_hot_path(debug, 1));
        assert!(should_keep_verbose_hot_path(debug, 16));
        assert!(should_keep_verbose_hot_path(
            b" WARN awayuki::ipc stream failure",
            1
        ));
        assert!(should_keep_verbose_hot_path(
            b" DEBUG awayuki::settings changed",
            1
        ));
    }

    #[test]
    #[ignore = "performance workflow records this timing-sensitive artifact"]
    fn logging_on_off_stream_benchmark() {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-log-benchmark-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("create logging benchmark directory");

        let off = benchmark_stream_logging(false, directory.join("disabled.log"));
        let on = benchmark_stream_logging(true, directory.join("enabled.log"));
        let report = LoggingBenchmarkReport {
            schema_version: 1,
            fixture_id: "synthetic-redacted-stream-v1",
            sample_rate: VERBOSE_HOT_PATH_SAMPLE_RATE,
            off,
            on,
        };
        let json = serde_json::to_string_pretty(&report).expect("serialize logging benchmark");
        println!("{json}");
        if let Ok(output) = std::env::var("AWAYUKI_LOG_BENCHMARK_OUTPUT") {
            let output = PathBuf::from(output);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent).expect("create logging benchmark output directory");
            }
            fs::write(output, format!("{json}\n")).expect("write logging benchmark report");
        }

        assert_eq!(report.on.dropped_records, 0);
        assert!(report.on.producer_p95_ms < 5.0);
        assert!(report.on.throughput_events_per_second >= 100.0);
        fs::remove_dir_all(directory).expect("remove logging benchmark directory");
    }

    fn benchmark_stream_logging(enabled: bool, path: PathBuf) -> LoggingBenchmarkSample {
        let record = b"2026-01-01T00:00:00Z DEBUG awayuki::ipc stream event operation_id=fixture content=private-post access_token=fixture-token\n";
        let (sender, worker) = if enabled {
            let (sender, receiver) = mpsc::sync_channel(LOG_QUEUE_CAPACITY);
            let worker = thread::spawn(move || log_worker(receiver, path));
            (Some(sender), Some(worker))
        } else {
            (None, None)
        };
        let dropped = AtomicU64::new(0);
        let mut retained_records = 0;
        let mut latencies = Vec::with_capacity(LOGGING_BENCHMARK_EVENTS);
        let total_started = Instant::now();

        for sequence in 0..LOGGING_BENCHMARK_EVENTS {
            let started = Instant::now();
            black_box(record.len());
            if let Some(sender) = &sender {
                if should_keep_verbose_hot_path(record, sequence as u64) {
                    retained_records += 1;
                    let redacted = redact_log_record(record).into_bytes();
                    if sender.try_send(LogMessage::Record(redacted)).is_err() {
                        dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            latencies.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        drop(sender);
        if let Some(worker) = worker {
            worker.join().expect("join logging benchmark worker");
        }
        let elapsed = total_started.elapsed().as_secs_f64().max(f64::EPSILON);
        latencies.sort_by(f64::total_cmp);
        let p95_index = ((latencies.len() - 1) as f64 * 0.95).round() as usize;

        LoggingBenchmarkSample {
            enabled,
            events: LOGGING_BENCHMARK_EVENTS,
            retained_records,
            dropped_records: dropped.load(Ordering::Relaxed),
            producer_p95_ms: latencies[p95_index],
            throughput_events_per_second: LOGGING_BENCHMARK_EVENTS as f64 / elapsed,
        }
    }
}
