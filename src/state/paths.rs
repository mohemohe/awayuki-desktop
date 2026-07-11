use std::path::{Path, PathBuf};

use crate::constants::{APP_NAME, DB_FILENAME, LOG_FILENAME};

const PORTABLE_MARKER_FILENAME: &str = "PORTABLE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// The normal per-user application data directory. Awayuki owns this
    /// directory and can safely restrict its permissions.
    PerUser,
    /// Debug builds retain their historical current-working-directory layout.
    DebugWorkingDirectory,
    /// Portable mode deliberately borrows the executable directory.
    Portable,
    /// Last-resort release fallback when no OS user directory is available.
    FallbackWorkingDirectory,
}

impl StorageKind {
    pub fn owns_directory(self) -> bool {
        matches!(self, Self::PerUser)
    }

    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::PerUser => None,
            Self::DebugWorkingDirectory => Some(
                "debug storage uses the current working directory; database and log files are private, but the parent directory may be shared",
            ),
            Self::Portable => Some(
                "portable storage inherits the executable directory ACL; do not place logged-in data on shared or untrusted media",
            ),
            Self::FallbackWorkingDirectory => Some(
                "OS user data directories are unavailable; storage fell back to the current working directory",
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLocation {
    pub directory: PathBuf,
    pub kind: StorageKind,
}

pub fn db_path() -> PathBuf {
    storage_location().directory.join(DB_FILENAME)
}

pub fn log_file_path() -> PathBuf {
    storage_location().directory.join(LOG_FILENAME)
}

pub fn storage_location() -> StorageLocation {
    if let Some(directory) = portable_data_dir() {
        return StorageLocation {
            directory,
            kind: StorageKind::Portable,
        };
    }

    if cfg!(debug_assertions) {
        return StorageLocation {
            directory: PathBuf::from("."),
            kind: StorageKind::DebugWorkingDirectory,
        };
    }

    let candidates = [
        dirs::data_dir().map(|directory| directory.join(APP_NAME)),
        dirs::home_dir().map(|directory| directory.join(format!(".{}", APP_NAME))),
    ];

    for directory in candidates.into_iter().flatten() {
        if std::fs::create_dir_all(&directory).is_ok() {
            return StorageLocation {
                directory,
                kind: StorageKind::PerUser,
            };
        }
    }

    StorageLocation {
        directory: PathBuf::from("."),
        kind: StorageKind::FallbackWorkingDirectory,
    }
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
