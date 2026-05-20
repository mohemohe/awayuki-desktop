use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub const ALL: [LogLevel; 5] = [
        LogLevel::Error,
        LogLevel::Warn,
        LogLevel::Info,
        LogLevel::Debug,
        LogLevel::Trace,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            LogLevel::Error => "Error",
            LogLevel::Warn => "Warn",
            LogLevel::Info => "Info",
            LogLevel::Debug => "Debug",
            LogLevel::Trace => "Trace",
        }
    }

    /// EnvFilter directive for the awayuki crate at this level.
    pub fn directive(&self) -> &'static str {
        match self {
            LogLevel::Error => "awayuki=error,webview=error",
            LogLevel::Warn => "awayuki=warn,webview=warn",
            LogLevel::Info => "awayuki=info,webview=info",
            LogLevel::Debug => "awayuki=debug,webview=debug",
            LogLevel::Trace => "awayuki=trace,webview=trace",
        }
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSettings {
    #[serde(default)]
    pub logging_enabled: bool,
    #[serde(default)]
    pub log_level: LogLevel,
}

impl Default for DebugSettings {
    fn default() -> Self {
        Self {
            logging_enabled: false,
            log_level: LogLevel::default(),
        }
    }
}
