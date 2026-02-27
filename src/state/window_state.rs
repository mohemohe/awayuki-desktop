use std::path::PathBuf;

use gpui::{point, px, size, Bounds, WindowBounds};
use serde::{Deserialize, Serialize};

use crate::constants::APP_NAME;

const WINDOW_STATE_FILENAME: &str = "window_state.json";

#[derive(Serialize, Deserialize)]
struct SavedWindowState {
    /// "windowed", "maximized", or "fullscreen"
    state: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn get_data_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        return PathBuf::from(".");
    }

    let candidates = [
        dirs::data_dir().map(|d| d.join(APP_NAME)),
        dirs::home_dir().map(|d| d.join(format!(".{}", APP_NAME))),
    ];

    for candidate in &candidates {
        if let Some(dir) = candidate {
            if std::fs::create_dir_all(dir).is_ok() {
                return dir.clone();
            }
        }
    }

    PathBuf::from(".")
}

fn state_file_path() -> PathBuf {
    get_data_dir().join(WINDOW_STATE_FILENAME)
}

pub fn load_window_bounds() -> Option<WindowBounds> {
    let path = state_file_path();
    let data = std::fs::read_to_string(&path).ok()?;
    let saved: SavedWindowState = serde_json::from_str(&data).ok()?;

    let bounds = Bounds {
        origin: point(px(saved.x), px(saved.y)),
        size: size(px(saved.width), px(saved.height)),
    };

    let window_bounds = match saved.state.as_str() {
        "maximized" => WindowBounds::Maximized(bounds),
        "fullscreen" => WindowBounds::Fullscreen(bounds),
        _ => WindowBounds::Windowed(bounds),
    };

    Some(window_bounds)
}

pub fn save_window_bounds(window_bounds: WindowBounds) {
    let (state, bounds) = match window_bounds {
        WindowBounds::Windowed(b) => ("windowed", b),
        WindowBounds::Maximized(b) => ("maximized", b),
        WindowBounds::Fullscreen(b) => ("fullscreen", b),
    };

    let saved = SavedWindowState {
        state: state.to_string(),
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
    };

    let path = state_file_path();
    match serde_json::to_string_pretty(&saved) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!("Failed to save window state: {}", e);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to serialize window state: {}", e);
        }
    }
}
