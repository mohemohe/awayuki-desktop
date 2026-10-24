//! Opt-in, isolated, debug-only exercise of the real WKWebView watchdog.
//! This module is excluded entirely from release builds.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewBuilder, WebviewUrl,
};

#[path = "webview_recovery_smoke_storage.rs"]
mod storage;

const INJECT_AFTER: Duration = Duration::from_secs(25);
const DEADLINE: Duration = Duration::from_secs(210);
const SIDECAR_LABEL: &str = "recovery-smoke-sidecar";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Frames,
    Finite,
    Infinite,
}

impl Mode {
    fn expected_stall(self) -> &'static str {
        match self {
            Self::Frames => "animation-frame",
            Self::Finite | Self::Infinite => "javascript",
        }
    }

    fn script(self) -> &'static str {
        match self {
            Self::Frames => "window.requestAnimationFrame = () => 0;",
            Self::Finite => {
                "(() => { const end = performance.now() + 70000; while (performance.now() < end) {} })();"
            }
            Self::Infinite => "(() => { for (;;) {} })();",
        }
    }
}

pub(super) struct Config {
    mode: Mode,
    report: File,
    started: Instant,
}

struct Smoke {
    state: Mutex<RunState>,
}

struct RunState {
    config: Config,
    last_frame: Option<u64>,
    main_finished: bool,
    injected_ms: Option<u64>,
    reload_requested_ms: Option<u64>,
    reload_requests: u32,
    recreation_requests: u32,
    stall_reason: Option<&'static str>,
    reload_started: bool,
    reload_finished: bool,
    recovered_progress: bool,
    fixture_window: Option<usize>,
    fixture_sidecar: Option<usize>,
    verification_started: bool,
    window_preserved: Option<bool>,
    sidecar_preserved: Option<bool>,
    main_below_sidecar: Option<bool>,
    completed: bool,
}

impl RunState {
    fn navigation_started(&mut self) {
        self.main_finished = false;
        self.last_frame = None;
        self.recovered_progress = false;
        self.reload_finished = false;
        self.reload_started = self.reload_requests > 0;
    }

    fn navigation_finished(&mut self) {
        self.main_finished = true;
        if self.reload_started {
            self.reload_finished = true;
            // Both samples proving progress must belong to the finished
            // replacement document, never the previous document or its load.
            self.last_frame = None;
        }
    }

    fn observe_frame(&mut self, frame: u64) -> bool {
        let advanced = self
            .last_frame
            .replace(frame)
            .is_some_and(|old| old != frame);
        if advanced && self.reload_started && self.reload_finished {
            self.recovered_progress = true;
        }
        advanced
    }

    fn passed(&self) -> bool {
        matches!((self.injected_ms, self.reload_requested_ms), (Some(injected), Some(requested)) if requested >= injected)
            && self.reload_requests == 1
            && self.recreation_requests <= 1
            && self.stall_reason == Some(self.config.mode.expected_stall())
            && self.reload_finished
            && self.recovered_progress
    }

    fn preservation_passed(&self) -> bool {
        self.window_preserved == Some(true)
            && self.sidecar_preserved == Some(true)
            && self.main_below_sidecar == Some(true)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    version: u8,
    mode: Mode,
    status: &'a str,
    reason: &'a str,
    elapsed_ms: u64,
    injected_ms: Option<u64>,
    reload_requested_ms: Option<u64>,
    reload_requests: u32,
    recreation_requests: u32,
    stall_reason: Option<&'static str>,
    new_document_loaded: bool,
    recovered_frame_progress: bool,
    window_preserved: Option<bool>,
    sidecar_preserved: Option<bool>,
    main_below_sidecar: Option<bool>,
}

/// Runs before Tauri opens any database. Every invocation requires a new,
/// explicitly selected fixture directory; an existing debug_data is rejected.
pub(super) fn prepare() -> Option<Config> {
    let mode = match std::env::var("AWAYUKI_WEBVIEW_RECOVERY_SMOKE") {
        Err(std::env::VarError::NotPresent) => return None,
        Ok(value) if value == "frames" => Mode::Frames,
        Ok(value) if value == "finite" => Mode::Finite,
        Ok(value) if value == "infinite" => Mode::Infinite,
        _ => configuration_failed("mode must be frames, finite, or infinite"),
    };
    let root = std::env::var_os("AWAYUKI_WEBVIEW_RECOVERY_SMOKE_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| configuration_failed("explicit fixture CWD is required"));
    let current = std::env::current_dir()
        .unwrap_or_else(|_| configuration_failed("cannot resolve fixture CWD"));
    let data =
        storage::prepare(&root, &current).unwrap_or_else(|error| configuration_failed(&error));
    let report = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("webview-recovery-smoke.json"))
        .unwrap_or_else(|_| configuration_failed("report must not already exist"));
    std::env::set_current_dir(&data)
        .unwrap_or_else(|_| configuration_failed("cannot enter isolated debug_data"));
    tracing::info!(target: "awayuki::webview", "Webview recovery smoke using fresh isolated debug_data");
    Some(Config {
        mode,
        report,
        started: Instant::now(),
    })
}

fn configuration_failed(message: &str) -> ! {
    eprintln!("Webview recovery smoke configuration rejected: {message}");
    std::process::exit(2);
}

pub(super) fn install(app: &tauri::App, config: Option<Config>) {
    let Some(config) = config else { return };
    app.manage(Smoke {
        state: Mutex::new(RunState {
            config,
            last_frame: None,
            main_finished: false,
            injected_ms: None,
            reload_requested_ms: None,
            reload_requests: 0,
            recreation_requests: 0,
            stall_reason: None,
            reload_started: false,
            reload_finished: false,
            recovered_progress: false,
            fixture_window: None,
            fixture_sidecar: None,
            verification_started: false,
            window_preserved: None,
            sidecar_preserved: None,
            main_below_sidecar: None,
            completed: false,
        }),
    });
    // The deadline is independent of WebContent, the native event loop, and the
    // Tokio executor. Even an infinite JS fault cannot leave automation hanging.
    let handle = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(DEADLINE);
        finish(&handle, false, "native deadline expired");
    });
    install_sidecar_fixture(app.handle());
    tracing::info!(target: "awayuki::webview", deadline_seconds = DEADLINE.as_secs(), "Webview recovery smoke armed");
}

fn install_sidecar_fixture(app: &AppHandle) {
    let Some(window) = app.get_window("main") else {
        finish(app, false, "fixture main window missing");
        return;
    };
    let Ok(native_window) = window.ns_window().map(|window| window as usize) else {
        finish(app, false, "fixture main native window unavailable");
        return;
    };
    let builder = WebviewBuilder::new(
        SIDECAR_LABEL,
        WebviewUrl::External("about:blank".parse().expect("valid fixture URL")),
    )
    .focused(false);
    let Ok(sidecar) = window.add_child(
        builder,
        LogicalPosition::new(8.0, 8.0),
        LogicalSize::new(24.0, 24.0),
    ) else {
        finish(app, false, "fixture sidecar creation failed");
        return;
    };
    let handle = app.clone();
    if sidecar
        .with_webview(move |platform| {
            let platform = super::webview_health::NativeWebviewHandles::new(platform);
            let sidecar = platform.inner() as usize;
            if native_window == 0 || sidecar == 0 || platform.ns_window() as usize != native_window
            {
                finish(&handle, false, "fixture native identity unavailable");
                return;
            }
            let Some(smoke) = handle.try_state::<Smoke>() else {
                return;
            };
            if let Ok(mut state) = smoke.state.lock() {
                state.fixture_window = Some(native_window);
                state.fixture_sidecar = Some(sidecar);
            };
        })
        .is_err()
    {
        finish(app, false, "fixture native inspection dispatch failed");
    }
}

pub(super) fn navigation(webview: &Webview, finished: bool) {
    if webview.label() != "main" {
        return;
    }
    let app = webview.app_handle();
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    let Ok(mut state) = smoke.state.lock() else {
        return;
    };
    if finished {
        state.navigation_finished();
    } else {
        state.navigation_started();
    }
    let passed = state.passed();
    drop(state);
    if passed {
        verify_preservation(app);
    }
}

pub(super) fn probe_completed(app: &AppHandle, response: Option<(u8, bool, u64)>, _resumed: bool) {
    let Some((1, true, frame)) = response else {
        return;
    };
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    let Ok(mut state) = smoke.state.lock() else {
        return;
    };
    // The watchdog only forwards accepted probes. Its one-shot `resumed`
    // flag may already have fired in the old document when a finite fault
    // ends before navigation, so require fresh post-load frame samples here.
    let advanced = state.observe_frame(frame);
    let passed = state.passed();
    let inject = advanced
        && state.main_finished
        && state.fixture_window.is_some()
        && state.fixture_sidecar.is_some()
        && state.injected_ms.is_none()
        && state.config.started.elapsed() >= INJECT_AFTER;
    if inject {
        state.injected_ms = Some(elapsed_ms(&state));
    }
    drop(state);
    if passed {
        verify_preservation(app);
    } else if inject {
        let handle = app.clone();
        if app
            .run_on_main_thread(move || inject_fault(&handle))
            .is_err()
        {
            finish(app, false, "fault dispatch failed");
        }
    }
}

fn inject_fault(app: &AppHandle) {
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    let mode = match smoke.state.lock() {
        Ok(state) => state.config.mode,
        Err(_) => return,
    };
    // The fixture is a new empty database/login document. Give that document a
    // safe null checkpoint without adding test-only commands to the frontend.
    let checkpoint = app.state::<crate::application::frontend_recovery::FrontendRecoveryState>();
    let allowed = checkpoint.restore().and_then(|restored| {
        checkpoint.checkpoint(serde_json::Value::Null, false, &restored.generation)
    });
    if allowed.is_err() {
        finish(app, false, "could not prepare fixture checkpoint");
        return;
    }
    tracing::info!(target: "awayuki::webview", "Webview recovery smoke injecting fault into healthy main document");
    if app
        .get_webview("main")
        .is_none_or(|webview| webview.eval(mode.script()).is_err())
    {
        finish(app, false, "fault evaluation dispatch failed");
    }
}

pub(super) fn recovery_requested(app: &AppHandle, reason: &'static str) {
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    if let Ok(mut state) = smoke.state.lock() {
        state.reload_requests += 1;
        state.reload_requested_ms = Some(elapsed_ms(&state));
        state.stall_reason = Some(reason);
    };
}

pub(super) fn recreation_requested(app: &AppHandle) {
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    if let Ok(mut state) = smoke.state.lock() {
        state.recreation_requests += 1;
    };
}

fn verify_preservation(app: &AppHandle) {
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    let Ok(mut state) = smoke.state.lock() else {
        return;
    };
    if state.verification_started || state.completed {
        return;
    }
    state.verification_started = true;
    drop(state);
    let handle = app.clone();
    if app
        .run_on_main_thread(move || inspect_preservation(&handle))
        .is_err()
    {
        finish(app, false, "preservation inspection dispatch failed");
    }
}

fn inspect_preservation(app: &AppHandle) {
    let Some(main) = app.get_webview("main") else {
        finish(app, false, "recovered main webview missing");
        return;
    };
    let Some(sidecar) = app.get_webview(SIDECAR_LABEL) else {
        finish(app, false, "fixture sidecar lost during recovery");
        return;
    };
    let handle = app.clone();
    if main
        .with_webview(move |main| {
            let main = super::webview_health::NativeWebviewHandles::new(main);
            let main_view = main.inner() as usize;
            let main_window = main.ns_window() as usize;
            let callback_handle = handle.clone();
            if sidecar
                .with_webview(move |sidecar| {
                    let sidecar = super::webview_health::NativeWebviewHandles::new(sidecar);
                    let Some(smoke) = callback_handle.try_state::<Smoke>() else {
                        return;
                    };
                    let Ok(mut state) = smoke.state.lock() else {
                        return;
                    };
                    state.window_preserved = Some(
                        main_window != 0
                            && state.fixture_window == Some(main_window)
                            && sidecar.ns_window() as usize == main_window,
                    );
                    state.sidecar_preserved = Some(
                        !sidecar.inner().is_null()
                            && state.fixture_sidecar == Some(sidecar.inner() as usize),
                    );
                    state.main_below_sidecar =
                        Some(main_is_below_sidecar(main_view, sidecar.inner()));
                    let passed = state.passed() && state.preservation_passed();
                    drop(state);
                    finish(
                        &callback_handle,
                        passed,
                        if passed {
                            "fresh document resumed JavaScript and animation frames; native window and sidecar preserved"
                        } else {
                            "native window, sidecar, or view ordering was not preserved"
                        },
                    );
                })
                .is_err()
            {
                finish(&handle, false, "sidecar native inspection dispatch failed");
            }
        })
        .is_err()
    {
        finish(app, false, "main native inspection dispatch failed");
    }
}

fn main_is_below_sidecar(main: usize, sidecar: *mut std::ffi::c_void) -> bool {
    use objc::{msg_send, runtime::Object, sel, sel_impl};

    if main == 0 || sidecar.is_null() {
        return false;
    }
    // with_webview runs this on the AppKit thread. Inspect only public NSView
    // hierarchy; a missing view or different parent must never pass the check.
    unsafe {
        let parent: *mut Object = msg_send![sidecar as *mut Object, superview];
        if parent.is_null() {
            return false;
        }
        let subviews: *mut Object = msg_send![parent, subviews];
        if subviews.is_null() {
            return false;
        }
        let count: usize = msg_send![subviews, count];
        let mut main_index = None;
        let mut sidecar_index = None;
        for index in 0..count {
            let view: *mut Object = msg_send![subviews, objectAtIndex: index];
            if view as usize == main {
                main_index = Some(index);
            }
            if view.cast::<std::ffi::c_void>() == sidecar {
                sidecar_index = Some(index);
            }
        }
        matches!((main_index, sidecar_index), (Some(main), Some(sidecar)) if main < sidecar)
    }
}

fn elapsed_ms(state: &RunState) -> u64 {
    state
        .config
        .started
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn finish(app: &AppHandle, passed: bool, reason: &'static str) {
    let Some(smoke) = app.try_state::<Smoke>() else {
        return;
    };
    let Ok(mut state) = smoke.state.lock() else {
        eprintln!("Webview recovery smoke state unavailable");
        std::process::exit(1);
    };
    if state.completed {
        return;
    }
    state.completed = true;
    let report = Report {
        version: 1,
        mode: state.config.mode,
        status: if passed { "passed" } else { "failed" },
        reason,
        elapsed_ms: elapsed_ms(&state),
        injected_ms: state.injected_ms,
        reload_requested_ms: state.reload_requested_ms,
        reload_requests: state.reload_requests,
        recreation_requests: state.recreation_requests,
        stall_reason: state.stall_reason,
        new_document_loaded: state.reload_finished,
        recovered_frame_progress: state.recovered_progress,
        window_preserved: state.window_preserved,
        sidecar_preserved: state.sidecar_preserved,
        main_below_sidecar: state.main_below_sidecar,
    };
    let written = serde_json::to_writer_pretty(&mut state.config.report, &report)
        .and_then(|()| {
            state
                .config
                .report
                .write_all(b"\n")
                .map_err(serde_json::Error::io)
        })
        .and_then(|()| {
            state
                .config
                .report
                .sync_all()
                .map_err(serde_json::Error::io)
        });
    tracing::info!(target: "awayuki::webview", passed, reason, report_written = written.is_ok(), "Webview recovery smoke finished");
    // Only this isolated debug process exits; no WebKit process is addressed or
    // killed directly. Immediate exit also bounds teardown of an infinite fault.
    std::process::exit(if passed && written.is_ok() { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_state() -> RunState {
        RunState {
            config: Config {
                mode: Mode::Finite,
                report: OpenOptions::new().write(true).open("/dev/null").unwrap(),
                started: Instant::now(),
            },
            last_frame: Some(1),
            main_finished: true,
            injected_ms: Some(25_000),
            reload_requested_ms: Some(70_000),
            reload_requests: 1,
            recreation_requests: 0,
            stall_reason: Some("javascript"),
            reload_started: false,
            reload_finished: false,
            recovered_progress: false,
            fixture_window: Some(1),
            fixture_sidecar: Some(2),
            verification_started: false,
            window_preserved: None,
            sidecar_preserved: None,
            main_below_sidecar: None,
            completed: false,
        }
    }

    #[test]
    fn old_document_progress_cannot_pass_after_the_replacement_loads() {
        let mut state = run_state();
        assert!(state.observe_frame(2));
        assert!(!state.recovered_progress);
        state.navigation_started();
        state.navigation_finished();
        assert!(!state.passed());
        assert!(!state.observe_frame(0));
        assert!(!state.passed());
        assert!(state.observe_frame(1));
        assert!(state.passed());
    }

    #[test]
    fn frames_during_navigation_do_not_count_as_post_load_progress() {
        let mut state = run_state();
        state.navigation_started();
        state.observe_frame(0);
        state.observe_frame(1);
        assert!(!state.recovered_progress);
        state.navigation_finished();
        assert!(!state.observe_frame(2));
        assert!(!state.passed());
        assert!(state.observe_frame(3));
        assert!(state.passed());
    }

    #[test]
    fn navigation_discards_prior_document_recovery_evidence() {
        let mut state = run_state();
        state.navigation_started();
        state.navigation_finished();
        state.observe_frame(0);
        state.observe_frame(1);
        assert!(state.passed());
        state.navigation_started();
        assert!(!state.reload_finished);
        assert!(!state.recovered_progress);
        state.navigation_finished();
        assert!(!state.passed());
    }

    #[test]
    fn recovery_accepts_at_most_one_recreation_after_exactly_one_reload() {
        let mut state = run_state();
        state.navigation_started();
        state.navigation_finished();
        state.observe_frame(0);
        state.observe_frame(1);
        assert!(state.passed());
        state.recreation_requests = 1;
        assert!(state.passed());
        state.recreation_requests = 2;
        assert!(!state.passed());
        state.recreation_requests = 1;
        state.reload_requests = 2;
        assert!(!state.passed());
        state.reload_requests = 0;
        assert!(!state.passed());
    }

    #[test]
    fn native_preservation_requires_every_positive_observation() {
        let mut state = run_state();
        assert!(!state.preservation_passed());
        state.window_preserved = Some(true);
        state.sidecar_preserved = Some(true);
        assert!(!state.preservation_passed());
        state.main_below_sidecar = Some(false);
        assert!(!state.preservation_passed());
        state.main_below_sidecar = Some(true);
        assert!(state.preservation_passed());
        state.sidecar_preserved = Some(false);
        assert!(!state.preservation_passed());
        state.sidecar_preserved = Some(true);
        state.window_preserved = Some(false);
        assert!(!state.preservation_passed());
    }
}
