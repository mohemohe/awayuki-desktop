//! Native window-state restore and debounced persistence.
//!
//! Functional account and credential state remains exclusively in the portable
//! SQLite database; this module stores only the non-functional window geometry
//! row in that same database.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{WebviewWindow, WindowEvent};

use super::{settings, Database, WINDOW_STATE_SAVE_DEBOUNCE_MS, WINDOW_STATE_SETTING_KEY};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedWindowState {
    /// "windowed", "maximized", or "fullscreen".
    state: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct RawSavedWindowState {
    state: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub(crate) async fn restore_window_state(window: &WebviewWindow, database: &Database) {
    let Some(state) = load_saved_window_state(database).await else {
        return;
    };

    if !is_window_state_usable(window, &state) {
        tracing::warn!("Ignoring unusable saved window state: {:?}", state);
        return;
    }

    if let Err(error) = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
        x: state.x,
        y: state.y,
    })) {
        tracing::warn!("Failed to restore window position: {}", error);
    }

    if let Err(error) = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
        width: state.width,
        height: state.height,
    })) {
        tracing::warn!("Failed to restore window size: {}", error);
    }

    match state.state.as_str() {
        "maximized" => {
            if let Err(error) = window.maximize() {
                tracing::warn!("Failed to restore maximized window state: {}", error);
            }
        }
        "fullscreen" => {
            if let Err(error) = window.set_fullscreen(true) {
                tracing::warn!("Failed to restore fullscreen window state: {}", error);
            }
        }
        _ => {}
    }
}

async fn load_saved_window_state(database: &Database) -> Option<SavedWindowState> {
    match settings::get_setting(database.reader(), WINDOW_STATE_SETTING_KEY).await {
        Ok(Some(json)) => parse_saved_window_state(&json, "app_settings.window_state"),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!("Failed to load window state: {}", error);
            None
        }
    }
}

fn parse_saved_window_state(json: &str, source: &str) -> Option<SavedWindowState> {
    let raw = match serde_json::from_str::<RawSavedWindowState>(json) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!("Failed to parse window state from {}: {}", source, error);
            return None;
        }
    };

    if !raw.x.is_finite() || !raw.y.is_finite() || !raw.width.is_finite() || !raw.height.is_finite()
    {
        tracing::warn!("Ignoring non-finite window state from {}", source);
        return None;
    }

    Some(SavedWindowState {
        state: raw.state,
        x: raw.x.round() as i32,
        y: raw.y.round() as i32,
        width: raw.width.round().max(0.0) as u32,
        height: raw.height.round().max(0.0) as u32,
    })
}

pub(crate) fn install_window_state_persistence(window: WebviewWindow, database: Arc<Database>) {
    let (save_signal, save_events) =
        crate::application::window_persistence::window_persistence_channel();
    let worker_window = window.clone();
    let worker_database = database.clone();

    tauri::async_runtime::spawn(
        crate::application::window_persistence::run_window_persistence_worker(
            save_events,
            Duration::from_millis(WINDOW_STATE_SAVE_DEBOUNCE_MS),
            move || {
                let worker_window = worker_window.clone();
                let worker_database = worker_database.clone();
                async move {
                    if let Err(error) = persist_window_state(&worker_window, &worker_database).await
                    {
                        tracing::warn!(error = %error, "Failed to persist window state");
                    }
                }
            },
        ),
    );

    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            save_signal.changed();
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            save_signal.flush();
        }
        _ => {}
    });
}

async fn persist_window_state(
    window: &WebviewWindow,
    database: &Database,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let is_fullscreen = window.is_fullscreen()?;
    let is_maximized = window.is_maximized()?;
    let position = window.outer_position()?;
    let size = window.outer_size()?;
    let state = SavedWindowState {
        state: if is_fullscreen {
            "fullscreen"
        } else if is_maximized {
            "maximized"
        } else {
            "windowed"
        }
        .to_string(),
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let json = serde_json::to_string(&state)?;
    settings::set_setting(database.writer(), WINDOW_STATE_SETTING_KEY, &json).await?;
    Ok(())
}

fn is_window_state_usable(window: &WebviewWindow, state: &SavedWindowState) -> bool {
    if state.width < 320 || state.height < 240 {
        return false;
    }

    let Ok(monitors) = window.available_monitors() else {
        return true;
    };
    if monitors.is_empty() {
        return true;
    }

    let window_left = state.x;
    let window_top = state.y;
    let window_right = state.x.saturating_add(state.width as i32);
    let window_bottom = state.y.saturating_add(state.height as i32);

    monitors.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        let monitor_left = position.x;
        let monitor_top = position.y;
        let monitor_right = position.x.saturating_add(size.width as i32);
        let monitor_bottom = position.y.saturating_add(size.height as i32);

        window_left < monitor_right
            && window_right > monitor_left
            && window_top < monitor_bottom
            && window_bottom > monitor_top
    })
}
