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

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::constants::APP_NAME;
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
        if let Err(e) = notify_rust::set_application("dev.mohemohe.awayuki.desktop") {
            tracing::warn!("Failed to set notification application: {}", e);
        }
    }

    tauri_commands::run();
}
