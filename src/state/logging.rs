use std::fs::{self, File, OpenOptions};
use std::io::{self, LineWriter, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::reload;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Registry;

use crate::state::debug_settings::LogLevel;
use crate::state::paths;

static LOG_FILE: OnceLock<Mutex<Option<LineWriter<File>>>> = OnceLock::new();
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

fn cell() -> &'static Mutex<Option<LineWriter<File>>> {
    LOG_FILE.get_or_init(|| Mutex::new(None))
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
        tracing::error!("Failed to reload log filter: {}", e);
    }
}

pub fn log_file_path() -> PathBuf {
    paths::log_file_path()
}

pub fn enable() -> io::Result<()> {
    let path = log_file_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut guard = cell().lock().unwrap();
    *guard = Some(LineWriter::new(file));
    Ok(())
}

pub fn disable() {
    let mut guard = cell().lock().unwrap();
    if let Some(ref mut writer) = *guard {
        let _ = writer.flush();
    }
    *guard = None;
}

/// Open the log file in the user's preferred application.
///
/// Creates an empty file first if it does not yet exist so the OS open call
/// has something to launch.
pub fn open_in_default_app() -> io::Result<()> {
    let path = log_file_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
    }
    open::that(&path).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
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
        let mut guard = cell().lock().unwrap();
        if let Some(ref mut writer) = *guard {
            writer.write(buf)
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = cell().lock().unwrap();
        if let Some(ref mut writer) = *guard {
            writer.flush()
        } else {
            Ok(())
        }
    }
}
