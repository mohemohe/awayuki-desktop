fn main() {
    #[cfg(target_os = "macos")]
    {
        // Production: find Sparkle.framework inside the app bundle
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

        // Development: find Sparkle.framework in cargo git checkouts for `cargo run`
        let home = std::env::var("HOME").unwrap_or_default();
        let cargo_home =
            std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{}/.cargo", home));
        let git_checkouts = format!("{}/git/checkouts", cargo_home);

        if let Ok(entries) = std::fs::read_dir(&git_checkouts) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("sparkle-updater") {
                    if let Ok(sub_entries) = std::fs::read_dir(entry.path()) {
                        for sub in sub_entries.flatten() {
                            let fw_dir = sub.path().join("sparkle-sys");
                            if fw_dir.join("Sparkle.framework").exists() {
                                println!(
                                    "cargo:rustc-link-arg=-Wl,-rpath,{}",
                                    fw_dir.display()
                                );
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
        res.set("ProductName", "Awayuki");
        res.set("FileDescription", "A lightweight Mastodon client");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows resources: {}", e);
        }
    }
}
