//! Self-build: build a release from a source tree and run the gate (tests + smoke launch).
//! See `docs/self_built.md`. Nothing in the UI drives this yet (later phases), so unused items
//! are expected for now.
#![allow(dead_code)]

pub mod apply;
pub mod build;
pub mod cache;
pub mod clone;
pub mod gate;
pub mod paths;
pub mod preflight;
pub mod ready;
pub mod review;
pub mod run;
pub mod upstream;

/// Seconds the smoke launch keeps the app alive. Mirrors `SMOKE_SECS` in `agentory-helper`; the
/// helper is a separate crate that is never built from the clone, so the value is duplicated.
pub const SMOKE_SECS: u64 = 10;
