use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::state::paths::StorageLocation;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Tighten the process creation mask before any runtime files are opened.
///
/// SQLite creates WAL and SHM sidecars itself, so setting a restrictive umask
/// at process start is the only way to avoid a world-readable interval on
/// Unix. Awayuki never needs to create group- or world-readable files.
pub fn harden_process_creation_mask() {
    #[cfg(unix)]
    // SAFETY: `umask` is process-global, so this is called once at the very
    // beginning of `main`, before Tauri starts worker threads or opens files.
    unsafe {
        libc::umask(0o077);
    }
}

/// Prepare an application storage location and return a warning when the
/// parent directory is intentionally borrowed (debug/portable/fallback).
pub fn prepare_storage(location: &StorageLocation) -> io::Result<Option<&'static str>> {
    // Never chmod a checkout or executable directory owned by another use
    // case. Files inside it are still forced private and callers surface the
    // mode-specific warning to the user-facing diagnostic log.
    if location.kind.owns_directory() {
        create_private_directory(&location.directory)?;
    } else {
        fs::create_dir_all(&location.directory)?;
    }

    Ok(location.kind.warning())
}

pub fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(PRIVATE_DIRECTORY_MODE);
        builder.create(path)?;
        set_private_directory_permissions(path)?;
    }

    #[cfg(not(unix))]
    {
        // On Windows, newly created entries inherit the ACL of the selected
        // per-user directory. Portable/fallback locations are treated as
        // borrowed and reported by `prepare_storage`; no insecure ACL rewrite
        // is guessed here.
        fs::create_dir_all(path)?;
    }

    Ok(())
}

pub fn open_private_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = non_empty_parent(path) {
        // The caller may intentionally use a borrowed debug/portable parent.
        // `prepare_storage` hardens app-owned directories and leaves borrowed
        // directory ACLs untouched.
        fs::create_dir_all(parent)?;
    }

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    apply_private_creation_mode(&mut options);
    let file = options.open(path)?;
    set_private_file_permissions(path)?;
    Ok(file)
}

pub fn create_private_file_if_missing(path: &Path) -> io::Result<()> {
    if let Some(parent) = non_empty_parent(path) {
        fs::create_dir_all(parent)?;
    }

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    apply_private_creation_mode(&mut options);
    let _file = options.open(path)?;
    set_private_file_permissions(path)
}

/// Repair permissions for the primary SQLite file and any sidecars that
/// already exist. A restrictive process umask protects sidecars created later.
pub fn harden_sqlite_files(database_path: &Path) -> io::Result<()> {
    for path in sqlite_file_set(database_path) {
        if path.exists() {
            set_private_file_permissions(&path)?;
        }
    }
    Ok(())
}

pub fn set_private_file_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
    }

    #[cfg(windows)]
    {
        if let Err(error) = set_windows_current_user_only_acl(path) {
            if windows_acl_is_unsupported(&error) {
                tracing::warn!(
                    file = %path.file_name().unwrap_or_default().to_string_lossy(),
                    error = %error,
                    "filesystem does not support private Windows ACLs; continuing without ACL repair"
                );
            } else {
                return Err(error);
            }
        }
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = path;
    }

    Ok(())
}

/// Replace a file's inherited Windows DACL with one protected allow ACE for
/// the current process user. Using the process token instead of a copied
/// file's previous owner avoids locking the active user out of a portable DB.
/// Parent-directory ACLs are never changed.
#[cfg(windows)]
fn set_windows_current_user_only_acl(path: &Path) -> io::Result<()> {
    if windows_acl_is_current_user_only(path)? {
        return Ok(());
    }

    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE,
        SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let current_user = windows_current_user_sid()?;

    let trustee = TRUSTEE_W {
        pMultipleTrustee: null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        ptstrName: current_user.sid.cast(),
    };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: 0,
        Trustee: trustee,
    };
    let mut acl: *mut ACL = null_mut();
    // Passing no old ACL intentionally creates a DACL containing only the
    // owner's entry; this avoids carrying inherited Everyone/Users entries.
    // SAFETY: all pointers refer to initialized values for the duration of the
    // call. The resulting ACL is a LocalAlloc allocation.
    let status = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    struct AclAllocation(*mut ACL);
    impl Drop for AclAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: SetEntriesInAclW returns a LocalAlloc allocation.
                unsafe {
                    LocalFree(self.0.cast());
                }
            }
        }
    }
    let _acl = AclAllocation(acl);

    // SAFETY: the ACL and path are valid until this synchronous call returns.
    let status = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }

    if windows_acl_is_current_user_only(path)? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows file ACL could not be restricted to the current user",
        ))
    }
}

#[cfg(windows)]
fn windows_acl_is_current_user_only(path: &Path) -> io::Result<bool> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    struct LocalAllocation(HLOCAL);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: GetNamedSecurityInfoW transfers this LocalAlloc
                // security descriptor to the caller.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: output pointers and NUL-terminated path satisfy the API contract.
    let status = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = LocalAllocation(descriptor.cast());
    if descriptor.is_null() || dacl.is_null() {
        return Ok(false);
    }

    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: descriptor remains owned by `_descriptor` until function exit.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Ok(false);
    }

    let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
    // SAFETY: dacl belongs to the live descriptor and `info` has the required size.
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || info.AceCount != 1
    {
        return Ok(false);
    }

    let mut raw_ace = null_mut();
    // SAFETY: the ACL reports one ACE at index zero.
    if unsafe { GetAce(dacl, 0, &mut raw_ace) } == 0 || raw_ace.is_null() {
        return Ok(false);
    }
    // `ACCESS_ALLOWED_ACE_TYPE` is zero. Avoid another Windows feature solely
    // for that constant while still validating the concrete ACE representation.
    let ace = unsafe { &*(raw_ace.cast::<ACCESS_ALLOWED_ACE>()) };
    if ace.Header.AceType != 0 || ace.Mask & FILE_ALL_ACCESS != FILE_ALL_ACCESS {
        return Ok(false);
    }
    let ace_sid: PSID = (&ace.SidStart as *const u32).cast_mut().cast();
    let current_user = windows_current_user_sid()?;
    // SAFETY: SidStart is the variable-length SID embedded in this allow ACE.
    Ok(unsafe { EqualSid(current_user.sid, ace_sid) } != 0)
}

#[cfg(windows)]
struct WindowsCurrentUserSid {
    _buffer: Vec<usize>,
    sid: windows_sys::Win32::Security::PSID,
}

#[cfg(windows)]
fn windows_current_user_sid() -> io::Result<WindowsCurrentUserSid> {
    use std::mem::size_of;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: OpenProcessToken returned this owned handle.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    let mut raw_token: HANDLE = null_mut();
    // SAFETY: output handle is initialized and closed by TokenHandle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = TokenHandle(raw_token);
    let mut required_bytes = 0u32;
    // The first call intentionally obtains the required buffer size.
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required_bytes);
    }
    if required_bytes == 0 {
        return Err(io::Error::last_os_error());
    }

    let words = (required_bytes as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0usize; words];
    // SAFETY: usize storage supplies sufficient alignment and byte capacity for
    // TOKEN_USER plus its variable-length SID.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required_bytes,
            &mut required_bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    if token_user.User.Sid.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows process token has no user SID",
        ));
    }
    Ok(WindowsCurrentUserSid {
        sid: token_user.User.Sid,
        _buffer: buffer,
    })
}

#[cfg(windows)]
fn windows_acl_is_unsupported(error: &io::Error) -> bool {
    error
        .raw_os_error()
        .is_some_and(|code| is_unsupported_windows_acl_error_code(code as u32))
}

#[cfg(any(windows, test))]
fn is_unsupported_windows_acl_error_code(code: u32) -> bool {
    // WinError.h: ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED, and
    // ERROR_CALL_NOT_IMPLEMENTED. FAT/exFAT drivers commonly return the first
    // two when persistent security descriptors are unavailable.
    matches!(code, 1 | 50 | 120)
}

fn set_private_directory_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

fn apply_private_creation_mode(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_FILE_MODE);
    }

    #[cfg(not(unix))]
    {
        let _ = options;
    }
}

fn sqlite_file_set(database_path: &Path) -> [PathBuf; 3] {
    let display = database_path.as_os_str().to_string_lossy();
    [
        database_path.to_path_buf(),
        PathBuf::from(format!("{display}-wal")),
        PathBuf::from(format!("{display}-shm")),
    ]
}

fn non_empty_parent(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn temporary_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "awayuki-storage-security-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn storage_kind_controls_whether_parent_is_owned() {
        use crate::state::paths::StorageKind;

        assert!(StorageKind::PerUser.owns_directory());
        assert!(!StorageKind::Portable.owns_directory());
        assert!(StorageKind::Portable.warning().is_some());
    }

    #[test]
    fn private_file_api_creates_an_appendable_file_on_every_platform() {
        use std::io::Write;

        let directory = temporary_directory("portable-api");
        fs::create_dir_all(&directory).expect("create directory");
        let path = directory.join("awayuki.log");
        let mut file = open_private_append(&path).expect("open private file");
        file.write_all(b"diagnostic\n").expect("append diagnostic");
        drop(file);

        assert_eq!(fs::read(&path).expect("read file"), b"diagnostic\n");
        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn only_acl_unsupported_filesystem_errors_are_treated_as_nonfatal() {
        assert!(is_unsupported_windows_acl_error_code(1));
        assert!(is_unsupported_windows_acl_error_code(50));
        assert!(is_unsupported_windows_acl_error_code(120));
        assert!(!is_unsupported_windows_acl_error_code(5));
        assert!(!is_unsupported_windows_acl_error_code(87));
    }

    #[cfg(unix)]
    #[test]
    fn creates_and_repairs_private_directories_and_files() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temporary_directory("permissions");
        create_private_directory(&directory).expect("create directory");
        let file = directory.join("awayuki.db");
        fs::write(&file, b"fixture").expect("write fixture");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))
            .expect("make fixture permissive");

        set_private_file_permissions(&file).expect("repair file permissions");

        assert_eq!(
            fs::metadata(&directory)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        assert_eq!(
            fs::metadata(&file)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            PRIVATE_FILE_MODE
        );

        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn portable_storage_does_not_chmod_the_borrowed_executable_directory() {
        use crate::state::paths::StorageKind;
        use std::os::unix::fs::PermissionsExt;

        let directory = temporary_directory("borrowed");
        fs::create_dir_all(&directory).expect("create directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
            .expect("set borrowed directory permissions");
        let location = StorageLocation {
            directory: directory.clone(),
            kind: StorageKind::Portable,
        };

        let warning = prepare_storage(&location).expect("prepare portable storage");

        assert!(warning.is_some());
        assert_eq!(
            fs::metadata(&directory)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_sidecars_are_repaired_together() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temporary_directory("sqlite");
        fs::create_dir_all(&directory).expect("create directory");
        let database = directory.join("awayuki.db");
        for path in sqlite_file_set(&database) {
            fs::write(&path, b"").expect("create sqlite fixture");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
                .expect("make fixture permissive");
        }

        harden_sqlite_files(&database).expect("repair sqlite files");

        for path in sqlite_file_set(&database) {
            assert_eq!(
                fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
                PRIVATE_FILE_MODE
            );
        }

        fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn sqlite_database_and_sidecars_receive_current_user_only_protected_acls() {
        let directory = temporary_directory("sqlite-windows-acl");
        fs::create_dir_all(&directory).expect("create directory");
        let database = directory.join("awayuki.db");
        for path in sqlite_file_set(&database) {
            fs::write(path, b"").expect("create sqlite fixture");
        }

        harden_sqlite_files(&database).expect("repair SQLite ACLs");

        for path in sqlite_file_set(&database) {
            assert!(
                windows_acl_is_current_user_only(&path).expect("inspect Windows ACL"),
                "{} must have one protected current-user ACE",
                path.display()
            );
        }

        fs::remove_dir_all(directory).expect("remove fixture");
    }
}
