#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use sparkle_updater::Updater;

#[cfg(target_os = "macos")]
use tauri::AppHandle;

#[cfg(target_os = "windows")]
const APPCAST_URL: &str = "https://mohemohe.github.io/awayuki-desktop/appcast-windows.xml";

#[cfg(target_os = "macos")]
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24);

// WinSparkle functions that are not exposed by `winsparkle-sys`. The import
// library is linked transitively by `sparkle-updater`; the release package
// places the matching x64 WinSparkle.dll beside awayuki.exe.
#[cfg(target_os = "windows")]
#[link(name = "WinSparkle", kind = "dylib")]
unsafe extern "C" {
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
fn to_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
static UPDATER: OnceLock<Updater> = OnceLock::new();

#[cfg(target_os = "macos")]
unsafe fn macos_has_feed_url() -> bool {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    let bundle: *mut Object = unsafe { msg_send![class!(NSBundle), mainBundle] };
    let info: *mut Object = unsafe { msg_send![bundle, infoDictionary] };
    let key: *mut Object =
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c"SUFeedURL".as_ptr()] };
    let value: *mut Object = unsafe { msg_send![info, objectForKey: key] };
    !value.is_null()
}

#[cfg(target_os = "macos")]
unsafe fn macos_check_for_updates_in_background() -> bool {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    if !unsafe { macos_has_feed_url() } {
        tracing::warn!("SUFeedURL is not configured; Sparkle update check was skipped");
        return false;
    }

    let updater_class = class!(SUUpdater);
    let updater: *mut Object = unsafe { msg_send![updater_class, sharedUpdater] };
    let _: () = unsafe { msg_send![updater, checkForUpdatesInBackground] };
    true
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn init_updater() {
    UPDATER.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            unsafe {
                if macos_check_for_updates_in_background() {
                    tracing::info!("Sparkle launch background update check requested");
                }
            }
            Updater::new()
        }

        #[cfg(target_os = "windows")]
        {
            // WinSparkle otherwise derives these values from VERSIONINFO.
            // Supplying them explicitly keeps update checks working for the
            // portable ZIP.
            unsafe {
                let company = to_wide_null("mohemohe");
                let app = to_wide_null("Awayuki");
                let version = to_wide_null(env!("APP_VERSION"));
                win_sparkle_set_app_details(company.as_ptr(), app.as_ptr(), version.as_ptr());
                win_sparkle_set_automatic_check_for_updates(1);
                win_sparkle_set_update_check_interval(60 * 60 * 24);
            }

            tracing::info!(
                version = env!("APP_VERSION"),
                feed = APPCAST_URL,
                "WinSparkle configured"
            );

            let updater = Updater::new(APPCAST_URL.to_string(), None);
            unsafe {
                win_sparkle_check_update_without_ui();
            }
            tracing::info!("WinSparkle background update check requested");
            updater
        }
    });
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn init_updater() {}

#[cfg(target_os = "macos")]
pub fn schedule_periodic_update_checks(app_handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(UPDATE_CHECK_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            if let Err(error) = app_handle.run_on_main_thread(|| unsafe {
                if macos_check_for_updates_in_background() {
                    tracing::info!("Sparkle periodic background update check requested");
                }
            }) {
                tracing::warn!(%error, "failed to schedule Sparkle update check");
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn schedule_periodic_update_checks(_app_handle: tauri::AppHandle) {}
