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
mod plugin_runtime_limits;
mod plugins;
mod services;
mod state;
mod tauri_commands;
mod updater;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::constants::{APP_IDENTIFIER, APP_NAME, APP_VERSION};
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
fn register_windows_notification_identity() -> Result<(), String> {
    use windows_registry::CURRENT_USER;

    let cache_root = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let icon_path = ensure_windows_notification_icon_at(&cache_root)
        .map_err(|error| format!("Failed to install notification icon: {error}"))?;
    let key = CURRENT_USER
        .create(format!(r"SOFTWARE\Classes\AppUserModelId\{APP_IDENTIFIER}"))
        .map_err(|error| error.to_string())?;
    key.set_string("DisplayName", "Awayuki")
        .map_err(|error| error.to_string())?;
    key.set_string("IconBackgroundColor", "0")
        .map_err(|error| error.to_string())?;
    key.set_string("IconUri", icon_path.to_string_lossy())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn ensure_windows_notification_icon_at(
    cache_root: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icons/AppIcon.png");

    let directory = cache_root.join(APP_NAME);
    std::fs::create_dir_all(&directory)?;
    let icon_path = directory.join(format!("notification-icon-{APP_VERSION}.png"));
    let needs_write = match std::fs::read(&icon_path) {
        Ok(existing) => existing != ICON_BYTES,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    if needs_write {
        std::fs::write(&icon_path, ICON_BYTES)?;
    }
    Ok(icon_path)
}

#[cfg(test)]
mod notification_identity_tests {
    use super::*;

    #[test]
    fn installs_the_product_icon_and_repairs_a_stale_cache_file() {
        let root = std::env::temp_dir().join(format!(
            "awayuki-notification-icon-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        let icon_path = ensure_windows_notification_icon_at(&root).unwrap();
        assert_eq!(
            std::fs::read(&icon_path).unwrap(),
            include_bytes!("../assets/icons/AppIcon.png")
        );

        std::fs::write(&icon_path, b"stale").unwrap();
        assert_eq!(
            ensure_windows_notification_icon_at(&root).unwrap(),
            icon_path
        );
        assert_eq!(
            std::fs::read(&icon_path).unwrap(),
            include_bytes!("../assets/icons/AppIcon.png")
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
