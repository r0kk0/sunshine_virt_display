//! svd-daemon library crate.
//!
//! Exposes internal modules so that integration tests (`tests/`) can
//! link against them without spawning the daemon binary.

pub mod config;
pub mod error;
pub mod handler;
pub mod ipc;
pub mod sleep;
pub mod strategy;
pub mod watcher;
