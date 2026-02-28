use std::sync::OnceLock;

use sparkle_updater::Updater;

#[cfg(target_os = "windows")]
const APPCAST_URL: &str = "https://mohemohe.github.io/awayuki-desktop/appcast-windows.xml";

static UPDATER: OnceLock<Updater> = OnceLock::new();

pub fn init_updater() {
    UPDATER.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            Updater::new()
        }

        #[cfg(target_os = "windows")]
        {
            Updater::new(APPCAST_URL.to_string(), None)
        }
    });
}

pub fn check_for_updates() {
    if let Some(updater) = UPDATER.get() {
        updater.check_for_updates();
    }
}
