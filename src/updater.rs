#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::OnceLock;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use sparkle_updater::Updater;

#[cfg(target_os = "windows")]
const APPCAST_URL: &str = "https://mohemohe.github.io/awayuki-desktop/appcast-windows.xml";

// WinSparkle FFI declarations not exposed by `winsparkle-sys`. The
// `WinSparkle.lib` import library is already on the link line via the
// `winsparkle-sys` crate (transitive through `sparkle-updater`).
#[cfg(target_os = "windows")]
#[link(name = "WinSparkle", kind = "dylib")]
extern "C" {
    fn win_sparkle_set_app_details(
        company_name: *const u16,
        app_name: *const u16,
        app_version: *const u16,
    );
    fn win_sparkle_set_automatic_check_for_updates(state: i32);
    fn win_sparkle_set_update_check_interval(interval: i32);
    fn win_sparkle_check_update_without_ui();
}

#[cfg(target_os = "windows")]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
static UPDATER: OnceLock<Updater> = OnceLock::new();

#[cfg(any(target_os = "macos", target_os = "windows"))]
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
                let key: *mut Object =
                    msg_send![class!(NSString), stringWithUTF8String: "SUFeedURL\0".as_ptr()];
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
            // WinSparkle reads CompanyName / AppName / FileVersion from the
            // EXE's VERSIONINFO to derive its registry path and to detect the
            // currently-installed version. If any of those are missing or
            // malformed, win_sparkle_init silently fails (the C API catches
            // all exceptions internally) and no notification ever appears.
            // Set them explicitly so behavior does not depend on winres.
            // These calls must precede win_sparkle_init, which Updater::new
            // invokes internally.
            unsafe {
                let company = to_wide_null("mohemohe");
                let app = to_wide_null("Awayuki");
                let version = to_wide_null(env!("APP_VERSION"));

                win_sparkle_set_app_details(company.as_ptr(), app.as_ptr(), version.as_ptr());

                // Defaults are also true / 86400, but be explicit so a stale
                // registry value from a previous run cannot disable checks.
                win_sparkle_set_automatic_check_for_updates(1);
                win_sparkle_set_update_check_interval(60 * 60 * 24);
            }

            tracing::info!(
                "WinSparkle: configured (app=Awayuki, version={}, feed={})",
                env!("APP_VERSION"),
                APPCAST_URL,
            );

            // sparkle-updater's Updater::new calls win_sparkle_set_appcast_url
            // followed by win_sparkle_init.
            let updater = Updater::new(APPCAST_URL.to_string(), None);

            // win_sparkle_init only fires an automatic check if
            // last_check_time + interval <= now, so once a check has been
            // recorded in the registry no further check happens for 24h.
            // Force a background check on every launch; the WinSparkle UI is
            // shown only if a new version is available.
            unsafe {
                win_sparkle_check_update_without_ui();
            }

            tracing::info!("WinSparkle: background update check requested");

            updater
        }
    });
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn init_updater() {
    tracing::info!("Auto-updater not supported on this platform");
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn check_for_updates() {
    if let Some(updater) = UPDATER.get() {
        updater.check_for_updates();
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn check_for_updates() {}
