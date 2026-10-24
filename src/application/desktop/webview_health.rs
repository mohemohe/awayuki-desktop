//! Detect a live-but-unresponsive main WKWebView, which does not invoke the
//! WebContent termination hook. Native polling remains independent of page JS.
//!
//! A completed rAF proves rendering callbacks progress, not that pixels reached
//! the display. Never infer a renderer failure from a hidden page's paused rAF.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use tauri::{AppHandle, Manager};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const GRACE_PERIOD: Duration = Duration::from_secs(15);
const MISSED_CHECK_INTERVAL: Duration = Duration::from_secs(15);
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(45);
const RECOVERY_COOLDOWN: Duration = Duration::from_secs(120);
const RECREATION_TIMEOUT: Duration = Duration::from_secs(60);

// One pending rAF per document, even when rendering stops but JS still runs.
// No timers, animation loops, account data, or page contents are collected.
const PROBE_SCRIPT: &str = r#"(() => {
  const key = '__AWAYUKI_WEBVIEW_HEALTH_PROBE__';
  const state = window[key] ||= { frame: 0, pending: false };
  const visible = document.visibilityState === 'visible';
  if (visible && !state.pending) {
    state.pending = true;
    requestAnimationFrame(() => { state.frame += 1; state.pending = false; });
  }
  return { version: 1, visible, frame: state.frame };
})()"#;

#[derive(Clone)]
struct WebviewHealth(Arc<Mutex<HealthState>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StallReason {
    JavaScript,
    AnimationFrame,
}

impl StallReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::JavaScript => "javascript",
            Self::AnimationFrame => "animation-frame",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Probe(u64),
    MissedCheck { reason: StallReason, checks: u8 },
    Recover(StallReason),
    Recreate,
}

#[derive(Debug)]
struct Progress {
    since: Instant,
    missed_checks: u8,
}

impl Progress {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            missed_checks: 0,
        }
    }
    fn reset(&mut self, now: Instant) {
        *self = Self::new(now);
    }
    fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.since)
    }
    fn missed_check(&mut self, now: Instant) -> Option<u8> {
        let checks = (self.elapsed(now).as_secs() / MISSED_CHECK_INTERVAL.as_secs()).min(3) as u8;
        if checks > self.missed_checks {
            self.missed_checks = checks;
            Some(checks)
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct PendingProbe {
    id: u64,
}

#[derive(Debug)]
struct FrameProgress {
    frame: u64,
    progress: Progress,
}

#[derive(Debug)]
struct HealthState {
    next_id: u64,
    pending: Option<PendingProbe>,
    javascript: Option<Progress>,
    frame: Option<FrameProgress>,
    next_probe_at: Instant,
    last_tick_wall: SystemTime,
    was_visible: bool,
    cooldown_until: Option<Instant>,
    awaiting_recovery_progress: bool,
    recreate_after: Option<Instant>,
    stopped: bool,
}

impl HealthState {
    fn new(now: Instant, wall: SystemTime) -> Self {
        Self {
            next_id: 0,
            pending: None,
            javascript: None,
            frame: None,
            next_probe_at: now + GRACE_PERIOD,
            last_tick_wall: wall,
            was_visible: false,
            cooldown_until: None,
            awaiting_recovery_progress: false,
            recreate_after: None,
            stopped: false,
        }
    }

    fn navigation(&mut self, now: Instant) {
        // A callback from the replaced document must never acknowledge a newer
        // probe. IDs are monotonic across navigation, visibility, and recovery.
        self.pending = None;
        self.javascript = None;
        self.frame = None;
        self.next_probe_at = now + GRACE_PERIOD;
    }

    fn pause(&mut self, now: Instant) {
        self.next_probe_at = now + GRACE_PERIOD;
        if self.recreate_after.is_some() {
            self.recreate_after = Some(now + GRACE_PERIOD + RECREATION_TIMEOUT);
        }
        // Keep the native evaluation outstanding. Repeated hide/show must not
        // accumulate evaluations against a suspended or hung WebContent process.
        if let Some(javascript) = &mut self.javascript {
            javascript.reset(now + GRACE_PERIOD);
        }
        if let Some(frame) = &mut self.frame {
            frame.progress.reset(now + GRACE_PERIOD);
        }
    }

    fn begin_recovery(&mut self, now: Instant) {
        self.cooldown_until = Some(now + RECOVERY_COOLDOWN);
        self.awaiting_recovery_progress = true;
        self.recreate_after = Some(now + RECREATION_TIMEOUT);
        self.navigation(now);
    }

    fn begin_recreation(&mut self, now: Instant) {
        // Consume this incident's one fallback before touching the native view.
        self.recreate_after = None;
        self.navigation(now);
    }

    fn tick(
        &mut self,
        now: Instant,
        wall: SystemTime,
        visible: bool,
        recovery_allowed: bool,
    ) -> Option<Action> {
        let scheduling_gap = wall
            .duration_since(self.last_tick_wall)
            .map_or(true, |gap| gap > GRACE_PERIOD);
        self.last_tick_wall = wall;
        if self.stopped {
            return None;
        }
        if !visible || !self.was_visible || scheduling_gap {
            self.was_visible = visible;
            self.pause(now);
            return None;
        }
        if self.recreate_after.is_some_and(|deadline| now >= deadline) {
            return Some(Action::Recreate);
        }
        let stalled = self
            .javascript
            .as_ref()
            .filter(|javascript| javascript.elapsed(now) >= RECOVERY_TIMEOUT)
            .map(|_| StallReason::JavaScript)
            .or_else(|| {
                self.frame
                    .as_ref()
                    .filter(|frame| frame.progress.elapsed(now) >= RECOVERY_TIMEOUT)
                    .map(|_| StallReason::AnimationFrame)
            });
        let cooldown_complete = self.cooldown_until.is_none_or(|until| now >= until);
        if let Some(reason) = stalled.filter(|_| recovery_allowed && cooldown_complete) {
            return Some(Action::Recover(reason));
        }
        if let Some(checks) = self
            .javascript
            .as_mut()
            .and_then(|javascript| javascript.missed_check(now))
        {
            return Some(Action::MissedCheck {
                reason: StallReason::JavaScript,
                checks,
            });
        }
        if let Some(checks) = self
            .frame
            .as_mut()
            .and_then(|frame| frame.progress.missed_check(now))
        {
            return Some(Action::MissedCheck {
                reason: StallReason::AnimationFrame,
                checks,
            });
        }
        if self.pending.is_none() && now >= self.next_probe_at {
            self.next_id += 1;
            self.pending = Some(PendingProbe { id: self.next_id });
            self.javascript.get_or_insert_with(|| Progress::new(now));
            return Some(Action::Probe(self.next_id));
        }
        None
    }

    fn complete(
        &mut self,
        id: u64,
        now: Instant,
        response: Option<ProbeResponse>,
        frontend_owns_checkpoint: bool,
    ) -> bool {
        if self.pending.as_ref().is_none_or(|pending| pending.id != id) {
            return false;
        }
        self.pending = None;
        self.next_probe_at = now + POLL_INTERVAL;
        if let Some(response) = response.filter(|response| response.version == 1) {
            self.javascript = Some(Progress::new(now));
            if response.visible && self.was_visible {
                if let Some(frame) = &mut self.frame {
                    if frame.frame != response.frame {
                        frame.frame = response.frame;
                        frame.progress.reset(now);
                        // An old document may resume JS before a queued reload
                        // actually navigates. Only a new restore handshake can
                        // return checkpoint ownership and complete recovery.
                        if frontend_owns_checkpoint {
                            self.awaiting_recovery_progress = false;
                            self.recreate_after = None;
                        }
                    }
                } else {
                    self.frame = Some(FrameProgress {
                        frame: response.frame,
                        progress: Progress::new(now),
                    });
                }
            } else {
                // WebKit may suppress rAF while the document is hidden even if the
                // NSWindow is visible (e.g. app hiding / Spaces transitions).
                self.frame = None;
            }
        }
        // An empty/error callback is not a responsiveness acknowledgement.
        // Keep both progress clocks while allowing a bounded retry.
        true
    }
}

#[derive(Deserialize)]
struct ProbeResponse {
    version: u8,
    visible: bool,
    frame: u64,
}

pub(super) fn navigation(app: &AppHandle, label: &str) {
    if label != "main" {
        return;
    }
    if let Some(health) = app.try_state::<WebviewHealth>() {
        if let Ok(mut state) = health.0.lock() {
            state.navigation(Instant::now());
        }
    }
}

pub(super) fn install(app: &tauri::App) {
    let health = WebviewHealth(Arc::new(Mutex::new(HealthState::new(
        Instant::now(),
        SystemTime::now(),
    ))));
    app.manage(health.clone());
    let app_handle = app.handle().clone();
    let scheduled = Arc::new(AtomicBool::new(false));
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if health.0.lock().map_or(true, |state| state.stopped) {
                break;
            }
            // Also bound native event-loop work if the application itself stalls.
            if scheduled.swap(true, Ordering::AcqRel) {
                continue;
            }
            let scheduled_tick = Arc::clone(&scheduled);
            let tick_app = app_handle.clone();
            let tick_health = health.clone();
            if app_handle
                .run_on_main_thread(move || {
                    poll(&tick_app, &tick_health);
                    scheduled_tick.store(false, Ordering::Release);
                })
                .is_err()
            {
                break;
            }
        }
    });
}

fn poll(app: &AppHandle, health: &WebviewHealth) {
    let (Some(webview), Some(window)) = (app.get_webview("main"), app.get_window("main")) else {
        if let Ok(mut state) = health.0.lock() {
            state.stopped = true;
        }
        return;
    };
    let visible = window.is_visible().unwrap_or(false)
        && !window.is_minimized().unwrap_or(true)
        && window_is_on_screen(&window);
    let allowed = crate::application::frontend_recovery::recovery_allowed(app);
    let action = health
        .0
        .lock()
        .ok()
        .and_then(|mut state| state.tick(Instant::now(), SystemTime::now(), visible, allowed));
    match action {
        Some(Action::Probe(id)) => {
            let callback_health = health.clone();
            let callback_app = app.clone();
            if let Err(error) = webview.eval_with_callback(PROBE_SCRIPT, move |result| {
                let response = serde_json::from_str::<ProbeResponse>(&result).ok();
                #[cfg(debug_assertions)]
                let smoke_response = response.as_ref().map(|response| (response.version, response.visible, response.frame));
                let valid = response.as_ref().is_some_and(|response| response.version == 1);
                let frontend_owns_checkpoint = crate::application::frontend_recovery::frontend_owns_checkpoint(&callback_app);
                let (accepted, resumed) = callback_health.0.lock().map(|mut state| {
                    let was_recovering = state.awaiting_recovery_progress;
                    let accepted = state.complete(id, Instant::now(), response, frontend_owns_checkpoint);
                    (accepted, was_recovering && !state.awaiting_recovery_progress)
                }).unwrap_or((false, false));
                if resumed {
                    tracing::info!(target: "awayuki::webview", "Main webview JavaScript and animation-frame progress resumed after recovery");
                }
                if accepted && !valid {
                    tracing::debug!(target: "awayuki::webview", "Main webview health evaluation returned no valid response");
                }
                #[cfg(debug_assertions)]
                if accepted {
                    super::webview_recovery_smoke::probe_completed(&callback_app, smoke_response, resumed);
                }
            }) {
                if let Ok(mut state) = health.0.lock() { state.complete(id, Instant::now(), None, false); }
                tracing::warn!(target: "awayuki::webview", %error, "Failed to dispatch main webview health evaluation");
            }
        }
        Some(Action::MissedCheck { reason, checks }) => {
            tracing::warn!(target: "awayuki::webview", reason = reason.as_str(), missed_checks = checks,
                recovery_allowed = allowed, "Main webview health check delayed");
        }
        Some(Action::Recover(reason)) => {
            // Reserve the checkpoint immediately before navigation. A newer
            // unsafe draft update can close the gate after the tick sampled it.
            if !crate::application::frontend_recovery::try_begin_recovery(app) {
                tracing::warn!(target: "awayuki::webview", "Main webview recovery deferred by newer frontend state");
                return;
            }
            if let Ok(mut state) = health.0.lock() {
                state.begin_recovery(Instant::now());
            }
            tracing::error!(target: "awayuki::webview", reason = reason.as_str(), timeout_seconds = RECOVERY_TIMEOUT.as_secs(),
                "Main webview unresponsive; requesting native reload with preserved frontend state");
            #[cfg(debug_assertions)]
            super::webview_recovery_smoke::recovery_requested(app, reason.as_str());
            // Public WKWebView.reload() also asks WebKit to check the Network /
            // GPU processes. JS location.reload() cannot run in a hung process.
            if let Err(error) = webview.reload() {
                tracing::error!(target: "awayuki::webview", %error, "Failed to request main webview recovery");
            }
        }
        Some(Action::Recreate) => {
            // A new document that has begun restoring owns the draft again.
            // Never use an older reload reservation to destroy that document.
            if !crate::application::frontend_recovery::try_begin_recreation(app) {
                if let Ok(mut state) = health.0.lock() {
                    state.recreate_after = None;
                }
                tracing::warn!(target: "awayuki::webview", "Main webview recreation deferred after frontend took checkpoint ownership");
                return;
            }
            if let Ok(mut state) = health.0.lock() {
                state.begin_recreation(Instant::now());
            }
            tracing::error!(target: "awayuki::webview", timeout_seconds = RECREATION_TIMEOUT.as_secs(),
                "Main webview reload did not recover; recreating only the main webview once");
            #[cfg(debug_assertions)]
            super::webview_recovery_smoke::recreation_requested(app);
            if let Err(error) = recreate_main_webview(app, &webview, &window) {
                tracing::error!(target: "awayuki::webview", %error, "Main webview recreation failed; no further automatic recreation scheduled");
            }
        }
        None => {}
    }
}

/// Tauri-runtime-wry 2.11.2 transfers three +1 Cocoa handles through
/// Retained::into_raw in WithWebview, and PlatformWebview has no destructor.
/// Balance those references so observing the old view cannot keep its hung
/// process alive after close. Re-check this ownership when upgrading Tauri.
pub(super) struct NativeWebviewHandles(tauri::webview::PlatformWebview);

impl NativeWebviewHandles {
    pub(super) fn new(native: tauri::webview::PlatformWebview) -> Self {
        Self(native)
    }
}

impl std::ops::Deref for NativeWebviewHandles {
    type Target = tauri::webview::PlatformWebview;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for NativeWebviewHandles {
    fn drop(&mut self) {
        use objc::runtime::Object;
        use objc::{msg_send, sel, sel_impl};
        for raw in [self.inner(), self.controller(), self.ns_window()] {
            unsafe {
                let _: () = msg_send![raw as *mut Object, release];
            }
        }
    }
}

fn recreate_main_webview(
    app: &AppHandle,
    old: &tauri::Webview,
    window: &tauri::Window,
) -> Result<(), String> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == "main")
        .ok_or("main webview configuration is unavailable")?
        .clone();
    let size = window.inner_size().map_err(|error| error.to_string())?;
    let window_focused = window.is_focused().unwrap_or(false);
    let main_focused = Arc::new(AtomicBool::new(false));
    let old_main_focused = Arc::clone(&main_focused);
    let media_uploads = Arc::clone(&app.state::<super::RuntimeState>().media_uploads);
    // poll() runs on the native main thread. Wry executes these operations
    // inline in that case, so old delegates are disabled before its label is
    // reused. Otherwise a delayed old page event could resolve the new `main`.
    old.with_webview(move |native| unsafe {
        let native = NativeWebviewHandles::new(native);
        let view = native.inner() as *mut Object;
        let window: *mut Object = msg_send![view, window];
        let responder: *mut Object = msg_send![window, firstResponder];
        let is_view: objc::runtime::BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
        let contained: objc::runtime::BOOL = if is_view == objc::runtime::YES {
            msg_send![responder, isDescendantOf: view]
        } else {
            objc::runtime::NO
        };
        old_main_focused.store(
            window_focused && contained == objc::runtime::YES,
            Ordering::Release,
        );
        let nil: *mut Object = std::ptr::null_mut();
        let _: () = msg_send![view, setNavigationDelegate: nil];
        let _: () = msg_send![view, setUIDelegate: nil];
    })
    .map_err(|error| error.to_string())?;
    old.close().map_err(|error| error.to_string())?;
    // Closing a Webview removes only that view. The existing NSWindow,
    // content-view container, window listeners, and sidecars remain alive.
    let replacement = window
        .add_child(
            tauri::WebviewBuilder::from_config(&config)
                .auto_resize()
                .focused(false),
            tauri::LogicalPosition::new(0.0, 0.0),
            size,
        )
        .map_err(|error| error.to_string())?;
    replacement
        .with_webview(|native| unsafe {
            let native = NativeWebviewHandles::new(native);
            let view = native.inner() as *mut Object;
            let parent: *mut Object = msg_send![view, superview];
            let nil: *mut Object = std::ptr::null_mut();
            // AppKit inserts a new child above existing siblings by default.
            // Put the opaque application view below the preserved sidecars.
            let _: () = msg_send![parent, addSubview: view positioned: -1isize relativeTo: nil];
        })
        .map_err(|error| error.to_string())?;
    super::install_drop_path_registration(&replacement, media_uploads);
    if main_focused.load(Ordering::Acquire) {
        replacement.set_focus().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn window_is_on_screen(window: &tauri::Window) -> bool {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    let Ok(window) = window.ns_window() else {
        return false;
    };
    // poll() always runs on the main thread. occlusionState is public AppKit
    // API; its Visible bit is 1 << 1. Focus is deliberately not required.
    unsafe {
        let occlusion: usize = msg_send![window as *mut Object, occlusionState];
        occlusion & (1 << 1) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clock {
        now: Instant,
        wall: SystemTime,
        state: HealthState,
    }
    impl Clock {
        fn new() -> Self {
            let now = Instant::now();
            let wall = SystemTime::now();
            Self {
                now,
                wall,
                state: HealthState::new(now, wall),
            }
        }
        fn tick(&mut self, seconds: u64, visible: bool, allowed: bool) -> Option<Action> {
            self.now += Duration::from_secs(seconds);
            self.wall += Duration::from_secs(seconds);
            let action = self.state.tick(self.now, self.wall, visible, allowed);
            if matches!(action, Some(Action::Recover(_))) {
                self.state.begin_recovery(self.now);
            } else if matches!(action, Some(Action::Recreate)) {
                self.state.begin_recreation(self.now);
            }
            action
        }
        fn probe(&mut self) -> u64 {
            self.tick(0, true, true);
            match self.tick(15, true, true) {
                Some(Action::Probe(id)) => id,
                other => panic!("{other:?}"),
            }
        }
        fn reply(&mut self, id: u64, frame: u64) {
            assert!(self.state.complete(
                id,
                self.now,
                Some(ProbeResponse {
                    version: 1,
                    visible: true,
                    frame
                }),
                true,
            ));
        }
    }

    #[test]
    fn one_outstanding_evaluation_recovers_only_after_45_seconds() {
        let mut c = Clock::new();
        c.probe();
        assert_eq!(c.tick(5, true, true), None);
        assert_eq!(
            c.tick(10, true, true),
            Some(Action::MissedCheck {
                reason: StallReason::JavaScript,
                checks: 1
            })
        );
        assert_eq!(
            c.tick(15, true, true),
            Some(Action::MissedCheck {
                reason: StallReason::JavaScript,
                checks: 2
            })
        );
        assert_eq!(
            c.tick(15, true, true),
            Some(Action::Recover(StallReason::JavaScript))
        );
    }

    #[test]
    fn unsafe_unsaved_state_defers_recovery_until_checkpoint_allows_it() {
        let mut c = Clock::new();
        c.probe();
        for _ in 0..3 {
            assert!(!matches!(c.tick(15, true, false), Some(Action::Recover(_))));
        }
        assert!(c.state.pending.is_some());
        assert_eq!(
            c.tick(5, true, true),
            Some(Action::Recover(StallReason::JavaScript))
        );
    }

    #[test]
    fn invalid_evaluation_replies_do_not_manufacture_javascript_progress() {
        let mut c = Clock::new();
        let id = c.probe();
        c.state.complete(id, c.now, None, false);
        let mut recovered = false;
        for _ in 0..12 {
            match c.tick(5, true, true) {
                Some(Action::Probe(id)) => {
                    c.state.complete(id, c.now, None, false);
                }
                Some(Action::Recover(reason)) => {
                    assert_eq!(reason, StallReason::JavaScript);
                    recovered = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(recovered);
    }

    #[test]
    fn rejected_recovery_reservation_preserves_the_outstanding_probe() {
        let mut c = Clock::new();
        let id = c.probe();
        c.tick(15, true, true);
        c.tick(15, true, true);
        c.now += Duration::from_secs(15);
        c.wall += Duration::from_secs(15);
        assert_eq!(
            c.state.tick(c.now, c.wall, true, true),
            Some(Action::Recover(StallReason::JavaScript))
        );
        // The checkpoint gate closed between the tick and native reload. No
        // begin_recovery occurs: no abandoned callback or spurious cooldown.
        assert_eq!(c.state.pending.as_ref().unwrap().id, id);
        assert!(c.state.cooldown_until.is_none());
        assert!(!c.state.awaiting_recovery_progress);
        c.reply(id, 1);
    }

    #[test]
    fn valid_reply_resets_a_previous_evaluation_failure() {
        let mut c = Clock::new();
        let id = c.probe();
        c.state.complete(id, c.now, None, false);
        c.tick(15, true, true);
        let Some(Action::Probe(id)) = c.tick(5, true, true) else {
            panic!()
        };
        c.reply(id, 1);
        for frame in 2..20 {
            let Some(Action::Probe(id)) = c.tick(5, true, true) else {
                panic!()
            };
            c.reply(id, frame);
        }
    }

    #[test]
    fn hidden_then_visible_rearms_the_same_probe_with_grace() {
        let mut c = Clock::new();
        let id = c.probe();
        c.tick(15, true, true);
        c.tick(15, false, true);
        c.tick(15, false, true);
        c.tick(5, true, true);
        assert_eq!(c.state.pending.as_ref().unwrap().id, id);
        for _ in 0..3 {
            assert!(!matches!(c.tick(15, true, true), Some(Action::Recover(_))));
        }
        assert_eq!(
            c.tick(15, true, true),
            Some(Action::Recover(StallReason::JavaScript))
        );
    }

    #[test]
    fn sleep_or_wall_clock_discontinuity_is_not_a_renderer_timeout() {
        let mut c = Clock::new();
        let id = c.probe();
        c.tick(15, true, true);
        assert_eq!(c.tick(600, true, true), None);
        assert_eq!(c.state.pending.as_ref().unwrap().id, id);
        c.wall -= Duration::from_secs(3600);
        assert_eq!(c.tick(5, true, true), None);
        assert_eq!(c.tick(15, true, true), None);
    }

    #[test]
    fn stale_callback_after_navigation_cannot_acknowledge_new_probe() {
        let mut c = Clock::new();
        let old = c.probe();
        c.state.navigation(c.now);
        let Some(Action::Probe(current)) = c.tick(15, true, true) else {
            panic!()
        };
        assert_ne!(old, current);
        assert!(!c.state.complete(old, c.now, None, false));
        assert_eq!(c.state.pending.as_ref().unwrap().id, current);
        c.reply(current, 1);
    }

    #[test]
    fn responsive_javascript_with_stalled_animation_frames_is_detected() {
        let mut c = Clock::new();
        let id = c.probe();
        c.reply(id, 0);
        let mut recovered = false;
        for _ in 0..10 {
            match c.tick(5, true, true) {
                Some(Action::Probe(id)) => c.reply(id, 0),
                Some(Action::Recover(reason)) => {
                    assert_eq!(reason, StallReason::AnimationFrame);
                    recovered = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(recovered);
    }

    #[test]
    fn frame_progress_keeps_visible_idle_window_healthy() {
        let mut c = Clock::new();
        let id = c.probe();
        c.reply(id, 0);
        for frame in 1..100 {
            let Some(Action::Probe(id)) = c.tick(5, true, true) else {
                panic!()
            };
            c.reply(id, frame);
        }
    }

    #[test]
    fn hidden_document_does_not_require_animation_frames() {
        let mut c = Clock::new();
        let id = c.probe();
        c.state.complete(
            id,
            c.now,
            Some(ProbeResponse {
                version: 1,
                visible: false,
                frame: 0,
            }),
            true,
        );
        for _ in 0..20 {
            let Some(Action::Probe(id)) = c.tick(5, true, true) else {
                panic!()
            };
            c.state.complete(
                id,
                c.now,
                Some(ProbeResponse {
                    version: 1,
                    visible: false,
                    frame: 0,
                }),
                true,
            );
        }
    }

    #[test]
    fn recovery_cooldown_survives_page_loads_and_stale_callbacks() {
        let mut c = Clock::new();
        let stale = c.probe();
        for _ in 0..2 {
            c.tick(15, true, true);
        }
        assert!(matches!(c.tick(15, true, true), Some(Action::Recover(_))));
        c.state.navigation(c.now);
        assert!(!c.state.complete(stale, c.now, None, false));
        for _ in 0..7 {
            assert!(!matches!(c.tick(15, true, true), Some(Action::Recover(_))));
        }
        assert!(matches!(c.tick(15, true, true), Some(Action::Recover(_))));
    }

    #[test]
    fn a_hung_reload_gets_one_recreation_after_60_visible_seconds() {
        let mut c = Clock::new();
        c.probe();
        c.tick(15, true, true);
        c.tick(15, true, true);
        assert!(matches!(c.tick(15, true, true), Some(Action::Recover(_))));
        for _ in 0..3 {
            assert!(!matches!(c.tick(15, true, false), Some(Action::Recreate)));
        }
        assert_eq!(c.tick(15, true, false), Some(Action::Recreate));
        assert!(c.state.recreate_after.is_none());
        for _ in 0..20 {
            assert!(!matches!(c.tick(15, true, false), Some(Action::Recreate)));
        }
    }

    #[test]
    fn healthy_frame_progress_cancels_the_pending_recreation() {
        let mut c = Clock::new();
        c.probe();
        c.tick(15, true, true);
        c.tick(15, true, true);
        assert!(matches!(c.tick(15, true, true), Some(Action::Recover(_))));
        let Some(Action::Probe(id)) = c.tick(15, true, false) else {
            panic!()
        };
        c.reply(id, 0);
        let Some(Action::Probe(id)) = c.tick(5, true, false) else {
            panic!()
        };
        c.reply(id, 1);
        assert!(c.state.recreate_after.is_none());
        assert!(!c.state.awaiting_recovery_progress);
        for _ in 0..10 {
            assert!(!matches!(c.tick(15, true, false), Some(Action::Recreate)));
        }
    }

    #[test]
    fn hidden_or_sleeping_window_cannot_spend_the_recreation_deadline() {
        let mut c = Clock::new();
        c.state.begin_recovery(c.now);
        c.tick(0, true, false);
        c.tick(15, true, false);
        c.tick(15, false, false);
        assert_eq!(c.tick(600, false, false), None);
        assert_eq!(c.tick(5, true, false), None);
        for _ in 0..4 {
            assert!(!matches!(c.tick(15, true, false), Some(Action::Recreate)));
        }
        assert_eq!(c.tick(15, true, false), Some(Action::Recreate));
    }

    #[test]
    fn recreation_invalidates_old_probes_and_does_not_arm_another_fallback() {
        let mut c = Clock::new();
        let old = c.probe();
        c.state.begin_recreation(c.now);
        assert!(c.state.recreate_after.is_none());
        assert!(!c.state.complete(old, c.now, None, false));
        let Some(Action::Probe(current)) = c.tick(15, true, false) else {
            panic!()
        };
        assert_ne!(old, current);
        c.reply(current, 0);
    }

    #[test]
    fn old_document_frame_progress_cannot_cancel_a_reserved_fallback() {
        let mut c = Clock::new();
        c.probe();
        c.state.begin_recovery(c.now);
        for frame in 0..3 {
            let Some(Action::Probe(id)) = c.tick(if frame == 0 { 15 } else { 5 }, true, false)
            else {
                panic!()
            };
            assert!(c.state.complete(
                id,
                c.now,
                Some(ProbeResponse {
                    version: 1,
                    visible: true,
                    frame
                }),
                false,
            ));
        }
        assert!(c.state.awaiting_recovery_progress);
        assert!(c.state.recreate_after.is_some());
        for _ in 0..6 {
            c.tick(5, true, false);
        }
        assert_eq!(c.tick(5, true, false), Some(Action::Recreate));
    }
}
