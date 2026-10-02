//! Tessera core: a headless Matrix client engine.

// The SDK's timeline types nest deeply; without this rustc fails with
// "queries overflow the depth limit" (same as the SDK's `examples/timeline`).
#![recursion_limit = "256"]

pub mod api;
mod audio_session;
mod avatars;
mod call;
mod call_settings;
mod client;
mod drafts;
mod engine;
pub mod logging;
mod oauth;
mod outgoing;
pub mod paths;
mod rich_text;
mod room_list;
pub mod secrets;
mod security;
mod session;
mod sync;
mod timeline;
mod verification;

pub use engine::{Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE};
