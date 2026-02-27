use std::sync::OnceLock;

use sparkle_updater::Updater;

static UPDATER: OnceLock<Updater> = OnceLock::new();

pub fn init_updater() {
    UPDATER.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            Updater::new()
        }
    });
}

pub fn check_for_updates() {
    if let Some(updater) = UPDATER.get() {
        updater.check_for_updates();
    }
}
