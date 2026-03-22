use std::sync::OnceLock;

use sparkle_updater::Updater;

#[cfg(target_os = "windows")]
const APPCAST_URL: &str = "https://mohemohe.github.io/awayuki-desktop/appcast-windows.xml";

static UPDATER: OnceLock<Updater> = OnceLock::new();

pub fn init_updater() {
    UPDATER.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            // GPUI runs this code inside applicationDidFinishLaunching:, so
            // NSApplicationDidFinishLaunchingNotification has already been
            // posted. SUUpdater registers for that notification in its init,
            // but it will never receive it — meaning startUpdateCycle is
            // never called and automatic checks never begin.
            // We therefore call checkForUpdatesInBackground explicitly.
            unsafe {
                use objc::{class, msg_send, runtime::Object, sel, sel_impl};

                // Check if SUFeedURL is configured in Info.plist.
                // When running via `cargo run` (no .app bundle), Info.plist is
                // absent and Sparkle throws an ObjC exception that aborts the
                // process because the extern "C" boundary cannot unwind.
                let bundle: *mut Object = msg_send![class!(NSBundle), mainBundle];
                let info: *mut Object = msg_send![bundle, infoDictionary];
                let key: *mut Object = msg_send![class!(NSString), stringWithUTF8String: "SUFeedURL\0".as_ptr()];
                let value: *mut Object = msg_send![info, objectForKey: key];
                if value.is_null() {
                    tracing::info!("SUFeedURL not configured, skipping Sparkle updater");
                    return Updater::new();
                }

                let cls = class!(SUUpdater);
                let shared: *mut Object = msg_send![cls, sharedUpdater];
                let _: () = msg_send![shared, checkForUpdatesInBackground];
            }
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
