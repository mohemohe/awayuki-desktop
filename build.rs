fn main() {
    // `tauri::generate_context!()` embeds `frontendDist` in the executable.
    // Watching only the top-level directory misses content-only changes below
    // `assets/`, so register every generated asset with Cargo.
    watch_frontend_dist(std::path::Path::new("frontend/dist"));
    tauri_build::build();

    println!("cargo:rerun-if-env-changed=VERSION");
    let version =
        std::env::var("VERSION").unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=APP_VERSION={version}");

    #[cfg(target_os = "windows")]
    windows_winsparkle::ensure_x64_and_deploy_dll();

    #[cfg(target_os = "macos")]
    {
        add_sparkle_runtime_rpaths();
        add_swift_runtime_rpaths();
    }
}

fn watch_frontend_dist(path: &std::path::Path) {
    println!("cargo:rerun-if-changed={}", path.display());

    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let child = entry.path();
        if child.is_dir() {
            watch_frontend_dist(&child);
        } else {
            println!("cargo:rerun-if-changed={}", child.display());
        }
    }
}

#[cfg(target_os = "macos")]
fn add_sparkle_runtime_rpaths() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    let home = std::env::var("HOME").unwrap_or_default();
    let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{home}/.cargo"));
    let checkouts = std::path::Path::new(&cargo_home)
        .join("git")
        .join("checkouts");
    let Ok(entries) = std::fs::read_dir(checkouts) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("sparkle-updater-")
        {
            continue;
        }
        let Ok(revisions) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for revision in revisions.flatten() {
            let sparkle_sys = revision.path().join("sparkle-sys");
            if sparkle_sys.join("Sparkle.framework").exists() {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sparkle_sys.display());
                return;
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn add_swift_runtime_rpaths() {
    // FoundationModels pulls Swift Concurrency in as an @rpath dependency.
    // Prefer the system Swift runtime to avoid loading a second copy from the
    // bundle; keep the toolchain path as a local cargo-run fallback.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    let output = match std::process::Command::new("xcrun")
        .args(["--find", "swift-stdlib-tool"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return,
    };

    let tool = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let Some(toolchain_usr) = std::path::Path::new(&tool)
        .parent()
        .and_then(|path| path.parent())
    else {
        return;
    };

    for relative in ["lib/swift-5.5/macosx", "lib/swift/macosx"] {
        let candidate = toolchain_usr.join(relative);
        if candidate.join("libswift_Concurrency.dylib").exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", candidate.display());
            return;
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_winsparkle {
    use std::path::{Path, PathBuf};

    const WINSPARKLE_VERSION: &str = "0.9.2";
    const X86_LIB_MAX_SIZE: u64 = 50_000;

    pub fn ensure_x64_and_deploy_dll() {
        let sys_dir = match find_winsparkle_sys_dir() {
            Some(directory) => directory,
            None => {
                println!(
                    "cargo:warning=winsparkle-sys checkout not found; \
                     run `cargo fetch` first if WinSparkle linking fails"
                );
                return;
            }
        };

        let import_library = sys_dir.join("WinSparkle.lib");
        let dll = sys_dir.join("WinSparkle.dll");
        if !is_x64_library(&import_library) {
            install_x64(&sys_dir);
        }
        copy_dll_to_target(&dll);
    }

    fn find_winsparkle_sys_dir() -> Option<PathBuf> {
        let cargo_home = std::env::var("CARGO_HOME").ok().or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .map(|home| format!("{home}\\.cargo"))
        })?;
        let entries =
            std::fs::read_dir(Path::new(&cargo_home).join("git").join("checkouts")).ok()?;
        for entry in entries.flatten() {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("sparkle-updater-")
            {
                continue;
            }
            for revision in std::fs::read_dir(entry.path()).ok()?.flatten() {
                let candidate = revision.path().join("winsparkle-sys");
                if candidate.join("WinSparkle.lib").exists() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    fn is_x64_library(path: &Path) -> bool {
        std::fs::metadata(path)
            .map(|metadata| metadata.len() > X86_LIB_MAX_SIZE)
            .unwrap_or(false)
    }

    fn install_x64(sys_dir: &Path) {
        let cache = std::env::temp_dir().join(format!("awayuki-winsparkle-{WINSPARKLE_VERSION}"));
        std::fs::create_dir_all(&cache).expect("create WinSparkle cache directory");
        let zip = cache.join(format!("WinSparkle-{WINSPARKLE_VERSION}.zip"));
        let extract = cache.join("extract");
        let release = extract
            .join(format!("WinSparkle-{WINSPARKLE_VERSION}"))
            .join("x64")
            .join("Release");
        if !release.join("WinSparkle.lib").exists() || !release.join("WinSparkle.dll").exists() {
            download_and_extract(&zip, &extract);
        }
        std::fs::copy(
            release.join("WinSparkle.lib"),
            sys_dir.join("WinSparkle.lib"),
        )
        .expect("copy x64 WinSparkle.lib");
        std::fs::copy(
            release.join("WinSparkle.dll"),
            sys_dir.join("WinSparkle.dll"),
        )
        .expect("copy x64 WinSparkle.dll");
    }

    fn download_and_extract(zip: &Path, extract: &Path) {
        let url = format!(
            "https://github.com/vslavik/winsparkle/releases/download/v{0}/WinSparkle-{0}.zip",
            WINSPARKLE_VERSION
        );
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; \
             if (-not (Test-Path -LiteralPath {zip})) {{ \
                 Invoke-WebRequest -Uri '{url}' -OutFile {zip} \
             }}; \
             if (Test-Path -LiteralPath {extract}) {{ \
                 Remove-Item -Recurse -Force -LiteralPath {extract} \
             }}; \
             Expand-Archive -LiteralPath {zip} -DestinationPath {extract}",
            zip = powershell_literal(zip),
            extract = powershell_literal(extract),
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .expect("invoke PowerShell to download WinSparkle");
        assert!(
            status.success(),
            "failed to download and extract WinSparkle x64"
        );
    }

    fn copy_dll_to_target(dll: &Path) {
        if !dll.exists() {
            return;
        }
        let Ok(out_dir) = std::env::var("OUT_DIR") else {
            return;
        };
        let Some(target_subdir) = Path::new(&out_dir).ancestors().nth(3) else {
            return;
        };
        let destination = target_subdir.join("WinSparkle.dll");
        if let Err(error) = std::fs::copy(dll, &destination) {
            println!(
                "cargo:warning=failed to copy WinSparkle.dll to {}: {}",
                destination.display(),
                error
            );
        }
    }

    fn powershell_literal(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "''"))
    }
}
