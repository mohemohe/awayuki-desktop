use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugSettings {
    #[serde(default)]
    pub logging_enabled: bool,
    #[serde(default)]
    pub log_level: LogLevel,
}
