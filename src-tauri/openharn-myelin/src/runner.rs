//! Sidecar runner facade.
//!
//! The ordered async turn loop lives in `runner/turn_loop.rs`; this module
//! keeps the crate-level API stable while making the loop independently
//! reviewable and testable.

mod turn_loop;

pub(crate) use turn_loop::run_loop;
