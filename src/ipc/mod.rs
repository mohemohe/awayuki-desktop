// Runtime uses the generated client contract; Rust metadata is compiled by the
// generator binary and its own completeness tests only.
pub mod account;
pub mod auth;
pub mod compose;
// Runtime consumes the settings registry while command metadata is consumed
// by the generator binary and completeness tests.
#[allow(dead_code)]
pub mod contract;
pub mod dto;
pub mod error;
pub mod maintenance;
pub mod media;
pub mod runtime;
pub mod settings;
pub mod sidecar;
pub mod timeline;
