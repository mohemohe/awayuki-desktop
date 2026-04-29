use std::path::PathBuf;

use crate::constants::{APP_NAME, DB_FILENAME, LOG_FILENAME};

/// Resolve the per-user data directory for awayuki.
///
/// Debug builds use the current working directory so devs can see the files
/// next to the executable. Release builds prefer the OS data dir, falling
/// back to a hidden directory in `$HOME`, and finally to the cwd.
pub fn data_dir() -> PathBuf {
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

pub fn db_path() -> PathBuf {
    data_dir().join(DB_FILENAME)
}

pub fn log_file_path() -> PathBuf {
    data_dir().join(LOG_FILENAME)
}
