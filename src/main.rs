#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod application;
mod auth;
mod bluesky;
mod constants;
mod db;
mod domain;
mod ipc;
mod mastodon;
mod misskey;
mod observability;
mod services;
mod state;
mod tauri_commands;
mod updater;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::constants::{APP_IDENTIFIER, APP_NAME};
use crate::state::logging::{self, LogFileMakeWriter};

fn main() {
    // Apply the private creation mask before Tauri, SQLite, or the optional
    // file logger can create runtime files.
    crate::state::storage_security::harden_process_creation_mask();

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("awayuki=info,webview=info"));
    let (filter_layer, reload_handle) = reload::Layer::new(env_filter);
    logging::set_filter_handle(reload_handle);

    let stderr_layer = fmt::layer().with_writer(std::io::stderr);
    let file_layer = fmt::layer().with_ansi(false).with_writer(LogFileMakeWriter);

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    tracing::info!("{} starting with Tauri...", APP_NAME);

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = notify_rust::set_application(APP_IDENTIFIER) {
            tracing::warn!("Failed to set notification application: {}", e);
        }
    }

    #[cfg(target_os = "windows")]
    if let Err(error) = register_windows_notification_identity() {
        tracing::warn!("Failed to register Awayuki notification identity: {error}");
    }

    tauri_commands::run();
}

#[cfg(target_os = "windows")]
fn register_windows_notification_identity() -> windows_registry::Result<()> {
    use windows_registry::CURRENT_USER;

    let key = CURRENT_USER.create(format!(r"SOFTWARE\Classes\AppUserModelId\{APP_IDENTIFIER}"))?;
    key.set_string("DisplayName", "Awayuki")?;
    key.set_string("IconBackgroundColor", "0")?;
    if let Ok(executable) = std::env::current_exe() {
        key.set_string("IconUri", executable.to_string_lossy())?;
    }
    Ok(())
}
