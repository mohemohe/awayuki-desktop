use std::path::{Path, PathBuf};

use crate::constants::{APP_NAME, DB_FILENAME, LOG_FILENAME};

const PORTABLE_MARKER_FILENAME: &str = "PORTABLE";

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
    app_storage_dir().join(DB_FILENAME)
}

pub fn log_file_path() -> PathBuf {
    app_storage_dir().join(LOG_FILENAME)
}

fn app_storage_dir() -> PathBuf {
    portable_data_dir().unwrap_or_else(data_dir)
}

fn portable_data_dir() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    portable_data_dir_for_executable(&executable)
}

fn portable_data_dir_for_executable(executable: &Path) -> Option<PathBuf> {
    let executable_dir = executable.parent()?;
    if executable_dir.join(PORTABLE_MARKER_FILENAME).is_file() {
        Some(executable_dir.to_path_buf())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("awayuki-paths-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn portable_data_dir_uses_executable_dir_when_marker_exists() {
        let dir = unique_temp_dir("portable");
        let executable = dir.join("awayuki");
        std::fs::write(dir.join(PORTABLE_MARKER_FILENAME), "").expect("write marker");

        assert_eq!(
            portable_data_dir_for_executable(&executable),
            Some(dir.clone())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn portable_data_dir_ignores_executable_dir_without_marker() {
        let dir = unique_temp_dir("non-portable");
        let executable = dir.join("awayuki");

        assert_eq!(portable_data_dir_for_executable(&executable), None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn portable_data_dir_does_not_use_app_bundle_parent_marker() {
        let dir = unique_temp_dir("macos-bundle-parent");
        let app_parent = dir.join("Applications");
        let executable_dir = app_parent
            .join("Awayuki.app")
            .join("Contents")
            .join("MacOS");
        std::fs::create_dir_all(&executable_dir).expect("create executable dir");
        std::fs::write(app_parent.join(PORTABLE_MARKER_FILENAME), "").expect("write marker");

        assert_eq!(
            portable_data_dir_for_executable(&executable_dir.join("awayuki")),
            None
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
