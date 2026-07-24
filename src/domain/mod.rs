//! Protocol-independent application domain types.
//!
//! Types in this module deliberately do not depend on Tauri, SQLx, or any
//! protocol adapter.  Adapters expose their functionality through capability
//! snapshots and commands carry explicit identities from the IPC boundary.

pub mod adapter_error;
pub mod capability;
pub mod identity;
pub mod protocol;
