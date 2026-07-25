//! Thin desktop entry facade.
//!
//! IPC handlers and runtime composition live under `ipc`, application
//! orchestration under `application`, and SQL in `db::queries`.

pub fn run() {
    crate::ipc::runtime::run();
}
