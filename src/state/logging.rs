use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

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
const LOG_GENERATIONS: usize = 3;

static LOG_CONTROL: OnceLock<LogControl> = OnceLock::new();
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static SECRET_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
static BEARER_TOKEN: OnceLock<Regex> = OnceLock::new();
static AUTHORIZATION_HEADER: OnceLock<Regex> = OnceLock::new();

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

struct RotatingLog {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    bytes_written: u64,
}

impl RotatingLog {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            writer: None,
            bytes_written: 0,
        }
    }

    fn write_record(&mut self, record: &[u8]) -> io::Result<()> {
        self.ensure_open()?;
        if self.bytes_written.saturating_add(record.len() as u64) > MAX_LOG_FILE_BYTES {
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
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush();
        self.writer = None;

        let oldest = rotated_path(&self.path, LOG_GENERATIONS);
        if oldest.exists() {
            fs::remove_file(oldest)?;
        }
        for generation in (1..LOG_GENERATIONS).rev() {
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
    let without_authorization = authorization.replace_all(&text, "Authorization: [redacted]");
    let without_bearer = bearer.replace_all(&without_authorization, "Bearer [redacted]");
    assignment
        .replace_all(&without_bearer, "$1$2[redacted]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_assignments_and_bearer_tokens() {
        let input = b"access_token=hunter2 code=oauth-code Authorization: Bearer abc.def";
        let output = redact_log_record(input);

        assert!(!output.contains("hunter2"));
        assert!(!output.contains("oauth-code"));
        assert!(!output.contains("abc.def"));
        assert!(output.contains("access_token=[redacted]"));
        assert!(output.contains("Authorization: [redacted]"));
    }

    #[test]
    fn rotated_names_are_stable() {
        assert_eq!(
            rotated_path(Path::new("/tmp/awayuki.log"), 2),
            PathBuf::from("/tmp/awayuki.log.2")
        );
    }
}
