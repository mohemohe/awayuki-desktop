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

    #[cfg(target_os = "macos")]
    add_swift_runtime_rpaths();
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
