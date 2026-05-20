fn main() {
    tauri_build::build();

    println!("cargo:rerun-if-env-changed=VERSION");
    let version =
        std::env::var("VERSION").unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=APP_VERSION={}", version);

    #[cfg(target_os = "macos")]
    {
        // Production: find Sparkle.framework inside the app bundle
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

        // Development: find Sparkle.framework in cargo git checkouts for `cargo run`
        let home = std::env::var("HOME").unwrap_or_default();
        let cargo_home = std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{}/.cargo", home));
        let git_checkouts = format!("{}/git/checkouts", cargo_home);

        if let Ok(entries) = std::fs::read_dir(&git_checkouts) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("sparkle-updater") {
                    if let Ok(sub_entries) = std::fs::read_dir(entry.path()) {
                        for sub in sub_entries.flatten() {
                            let fw_dir = sub.path().join("sparkle-sys");
                            if fw_dir.join("Sparkle.framework").exists() {
                                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", fw_dir.display());
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("build/AppIcon.ico");
        // CompanyName is required by WinSparkle: it derives its registry path
        // from `HKCU\Software\<CompanyName>\<AppName>`. Without it, WinSparkle
        // init silently fails (the C API catches all exceptions internally).
        res.set("CompanyName", "mohemohe");
        res.set("ProductName", "Awayuki");
        res.set("InternalName", "awayuki");
        res.set("OriginalFilename", "awayuki.exe");
        res.set("FileDescription", "A lightweight Mastodon client");
        res.set("LegalCopyright", "Copyright (c) mohemohe");

        res.set("FileVersion", &version);
        res.set("ProductVersion", &version);

        let parts: Vec<u64> = version.split('.').filter_map(|s| s.parse().ok()).collect();
        let major = parts.first().copied().unwrap_or(0);
        let minor = parts.get(1).copied().unwrap_or(0);
        let patch = parts.get(2).copied().unwrap_or(0);
        let numeric = (major << 48) | (minor << 32) | (patch << 16);
        res.set_version_info(winres::VersionInfo::FILEVERSION, numeric);
        res.set_version_info(winres::VersionInfo::PRODUCTVERSION, numeric);

        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows resources: {}", e);
        }

        // The `winsparkle-sys` crate ships an x86 (32-bit) `WinSparkle.lib`,
        // which produces LNK4272 + LNK2019 against an x64 build. Replace it
        // with the official x64 binaries and copy `WinSparkle.dll` next to
        // the produced exe so `cargo run`/`cargo build --release` work
        // out-of-the-box (release.yml does the equivalent in CI).
        windows_winsparkle::ensure_x64_and_deploy_dll();
    }
}

#[cfg(target_os = "windows")]
mod windows_winsparkle {
    use std::path::{Path, PathBuf};

    const WINSPARKLE_VERSION: &str = "0.9.2";
    const X86_LIB_MAX_SIZE: u64 = 50_000;

    pub fn ensure_x64_and_deploy_dll() {
        let sys_dir = match find_winsparkle_sys_dir() {
            Some(d) => d,
            None => {
                println!(
                    "cargo:warning=winsparkle-sys checkout not found; \
                     run `cargo fetch` first if WinSparkle linking fails"
                );
                return;
            }
        };

        let lib = sys_dir.join("WinSparkle.lib");
        let dll = sys_dir.join("WinSparkle.dll");

        if !is_x64_lib(&lib) {
            install_x64(&sys_dir);
        }

        copy_dll_to_target(&dll);
    }

    fn find_winsparkle_sys_dir() -> Option<PathBuf> {
        let cargo_home = std::env::var("CARGO_HOME").ok().or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .map(|h| format!("{}\\.cargo", h))
        })?;

        let checkouts = Path::new(&cargo_home).join("git").join("checkouts");
        let entries = std::fs::read_dir(&checkouts).ok()?;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("sparkle-updater-") {
                continue;
            }
            if let Ok(subs) = std::fs::read_dir(entry.path()) {
                for sub in subs.flatten() {
                    let candidate = sub.path().join("winsparkle-sys");
                    if candidate.join("WinSparkle.lib").exists() {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    // The bundled x86 lib is ~32 KB; the official x64 lib is ~80 KB. Size is
    // a robust-enough discriminator for this single file.
    fn is_x64_lib(lib: &Path) -> bool {
        std::fs::metadata(lib)
            .map(|m| m.len() > X86_LIB_MAX_SIZE)
            .unwrap_or(false)
    }

    fn install_x64(sys_dir: &Path) {
        println!(
            "cargo:warning=Installing WinSparkle {} (x64) into {}",
            WINSPARKLE_VERSION,
            sys_dir.display()
        );

        let cache = std::env::temp_dir().join(format!("awayuki-winsparkle-{}", WINSPARKLE_VERSION));
        std::fs::create_dir_all(&cache).expect("create WinSparkle cache dir");

        let zip = cache.join(format!("WinSparkle-{}.zip", WINSPARKLE_VERSION));
        let extract = cache.join("extract");
        let extracted_lib = extract
            .join(format!("WinSparkle-{}", WINSPARKLE_VERSION))
            .join("x64")
            .join("Release")
            .join("WinSparkle.lib");
        let extracted_dll = extract
            .join(format!("WinSparkle-{}", WINSPARKLE_VERSION))
            .join("x64")
            .join("Release")
            .join("WinSparkle.dll");

        if !extracted_lib.exists() || !extracted_dll.exists() {
            download_and_extract(&zip, &extract);
        }

        std::fs::copy(&extracted_lib, sys_dir.join("WinSparkle.lib"))
            .expect("copy x64 WinSparkle.lib");
        std::fs::copy(&extracted_dll, sys_dir.join("WinSparkle.dll"))
            .expect("copy x64 WinSparkle.dll");
    }

    fn download_and_extract(zip: &Path, extract: &Path) {
        let url = format!(
            "https://github.com/vslavik/winsparkle/releases/download/v{0}/WinSparkle-{0}.zip",
            WINSPARKLE_VERSION
        );
        let zip_str = path_to_pwsh_string(zip);
        let extract_str = path_to_pwsh_string(extract);

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
            zip = zip_str,
            extract = extract_str,
            url = url,
        );

        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .expect("invoke powershell to download WinSparkle");

        if !status.success() {
            panic!(
                "Failed to download/extract WinSparkle x64 (powershell exit {:?})",
                status.code()
            );
        }
    }

    fn copy_dll_to_target(dll: &Path) {
        if !dll.exists() {
            return;
        }

        let out_dir = match std::env::var("OUT_DIR") {
            Ok(v) => v,
            Err(_) => return,
        };
        let target_subdir = match Path::new(&out_dir).ancestors().nth(3) {
            Some(p) => p.to_path_buf(),
            None => return,
        };

        let dest = target_subdir.join("WinSparkle.dll");
        if let Err(e) = std::fs::copy(dll, &dest) {
            println!(
                "cargo:warning=Failed to copy WinSparkle.dll to {}: {}",
                dest.display(),
                e
            );
        }
    }

    fn path_to_pwsh_string(p: &Path) -> String {
        // Wrap in single quotes so PowerShell treats it literally; escape any
        // existing single quotes by doubling them per PowerShell rules.
        let s = p.display().to_string().replace('\'', "''");
        format!("'{}'", s)
    }
}
