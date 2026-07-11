//! Application use cases and process-level coordination.
//!
//! This layer owns orchestration and is intentionally independent from Tauri
//! command argument types. IPC handlers should validate/translate input and
//! then call one application operation.

pub mod desktop;
pub mod sidecar_policy;
pub mod startup_gate;
