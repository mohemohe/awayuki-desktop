use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// Reserve a fresh, private storage directory for one debug smoke run.
///
/// The caller supplies the actual current directory and must switch to the
/// returned path before resolving any application storage paths. Keeping that
/// process-wide mutation outside this helper makes validation safe to test in
/// parallel. A pre-existing directory is deliberately never reused, including
/// an empty directory or a symlink left by an earlier run.
pub(super) fn prepare(root: &Path, current: &Path) -> Result<PathBuf, String> {
    if !root.is_absolute() || !current.is_absolute() {
        return Err("recovery smoke root and current directory must be absolute".to_owned());
    }

    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve recovery smoke root: {error}"))?;
    let current = current
        .canonicalize()
        .map_err(|error| format!("cannot resolve recovery smoke current directory: {error}"))?;
    if root != current {
        return Err("recovery smoke root must match the current directory".to_owned());
    }
    if !root.is_dir() {
        return Err("recovery smoke root must be a directory".to_owned());
    }

    let storage = root.join("debug_data");
    // create_dir is atomic and fails for every existing directory entry,
    // including a dangling symlink. Do not replace this with create_dir_all.
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&storage)
        .map_err(|error| format!("recovery smoke requires fresh debug_data storage: {error}"))?;

    let metadata = fs::symlink_metadata(&storage)
        .map_err(|error| format!("cannot inspect recovery smoke storage: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err("recovery smoke storage must be a real directory".to_owned());
    }
    let canonical_storage = storage
        .canonicalize()
        .map_err(|error| format!("cannot resolve recovery smoke storage: {error}"))?;
    if canonical_storage != storage {
        return Err("recovery smoke storage must remain below its selected root".to_owned());
    }
    Ok(canonical_storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "awayuki-recovery-smoke-storage-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir(&path).expect("create isolated test root");
            Self(path.canonicalize().expect("resolve isolated test root"))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn creates_fresh_private_storage_without_changing_current_directory() {
        let root = TestRoot::new();
        let current = std::env::current_dir().expect("current directory");

        let storage = prepare(root.path(), root.path()).expect("prepare fresh storage");

        assert_eq!(storage, root.path().join("debug_data"));
        assert!(storage.is_absolute());
        assert_eq!(fs::read_dir(&storage).expect("read storage").count(), 0);
        assert_eq!(
            fs::metadata(&storage)
                .expect("storage metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(std::env::current_dir().expect("current directory"), current);
    }

    #[test]
    fn accepts_canonical_equivalent_root_and_current_directory() {
        let root = TestRoot::new();
        assert_eq!(
            prepare(&root.path().join("."), root.path()).expect("canonical root"),
            root.path().join("debug_data")
        );
    }

    #[test]
    fn rejects_relative_root_or_current_directory_without_creating_storage() {
        let root = TestRoot::new();
        assert!(prepare(Path::new("."), root.path()).is_err());
        assert!(prepare(root.path(), Path::new(".")).is_err());
        assert!(!root.path().join("debug_data").exists());
    }

    #[test]
    fn rejects_mismatched_current_directory_without_creating_storage() {
        let root = TestRoot::new();
        let other = TestRoot::new();
        assert!(prepare(root.path(), other.path()).is_err());
        assert!(!root.path().join("debug_data").exists());
        assert!(!other.path().join("debug_data").exists());
    }

    #[test]
    fn rejects_existing_storage_and_preserves_its_data() {
        let root = TestRoot::new();
        let storage = root.path().join("debug_data");
        fs::create_dir(&storage).expect("create existing storage");
        let database = storage.join("awayuki.db");
        fs::write(&database, "existing data").expect("existing database");

        assert!(prepare(root.path(), root.path()).is_err());
        assert_eq!(
            fs::read_to_string(database).expect("existing database"),
            "existing data"
        );
    }

    #[test]
    fn rejects_existing_empty_storage() {
        let root = TestRoot::new();
        fs::create_dir(root.path().join("debug_data")).expect("create existing storage");
        assert!(prepare(root.path(), root.path()).is_err());
    }

    #[test]
    fn rejects_existing_file_as_storage() {
        let root = TestRoot::new();
        let storage = root.path().join("debug_data");
        fs::write(&storage, "existing file").expect("create existing file");
        assert!(prepare(root.path(), root.path()).is_err());
        assert_eq!(
            fs::read_to_string(storage).expect("existing file"),
            "existing file"
        );
    }

    #[test]
    fn rejects_existing_symlink_without_touching_its_target() {
        let root = TestRoot::new();
        let target = TestRoot::new();
        let database = target.path().join("awayuki.db");
        fs::write(&database, "user data").expect("create target database");
        symlink(target.path(), root.path().join("debug_data")).expect("create storage symlink");

        assert!(prepare(root.path(), root.path()).is_err());
        assert_eq!(
            fs::read_to_string(database).expect("target database"),
            "user data"
        );
        assert_eq!(fs::read_dir(target.path()).expect("read target").count(), 1);
    }

    #[test]
    fn rejects_dangling_storage_symlink() {
        let root = TestRoot::new();
        let target = root.path().join("missing-target");
        symlink(&target, root.path().join("debug_data")).expect("create dangling symlink");
        assert!(prepare(root.path(), root.path()).is_err());
        assert!(!target.exists());
    }
}
