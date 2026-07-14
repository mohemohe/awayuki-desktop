//! Application use cases and process-level coordination.
//!
//! This layer owns orchestration and is intentionally independent from Tauri
//! command argument types. IPC handlers should validate/translate input and
//! then call one application operation.

pub mod account;
pub mod auth;
pub mod compose;
pub mod desktop;
pub mod maintenance;
pub mod media;
pub mod notification;
pub mod preferences;
pub mod runtime;
pub mod settings;
pub mod sidecar_policy;
pub mod startup_gate;
pub mod status;
pub mod timeline;
pub mod timeline_hydration;
pub mod timeline_view;
pub mod translation;
pub mod window_persistence;
