//! Tessera core: a headless Matrix client engine.

// The SDK's timeline types nest deeply; without this rustc fails with
// "queries overflow the depth limit" (same as the SDK's `examples/timeline`).
#![recursion_limit = "256"]

pub mod api;
pub mod paths;
pub mod secrets;
mod session;
