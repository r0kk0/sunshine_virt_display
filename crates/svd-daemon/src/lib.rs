//! svd-daemon library crate.
//!
//! Exposes internal modules so that integration tests (`tests/`) can
//! link against them without spawning the daemon binary.

pub mod config;
pub mod error;
pub mod ipc;
