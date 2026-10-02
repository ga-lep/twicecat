//! Tessera core: a headless Matrix client engine.

// The SDK's timeline types nest deeply; without this rustc fails with
// "queries overflow the depth limit" (same as the SDK's `examples/timeline`).
#![recursion_limit = "256"]

pub mod api;
mod avatars;
mod client;
mod engine;
pub mod logging;
mod oauth;
pub mod paths;
mod room_list;
pub mod secrets;
mod session;
mod sync;
mod timeline;

pub use engine::{Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE};
