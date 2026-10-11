//! Twicecat core: a headless Matrix client engine.

// The SDK's timeline types nest deeply; without this rustc fails with
// "queries overflow the depth limit" (same as the SDK's `examples/timeline`).
#![recursion_limit = "256"]

pub mod api;
mod audio_session;
mod avatars;
mod call;
mod call_settings;
mod client;
mod cpu_level;
mod drafts;
mod engine;
mod gpu_enhancement;
pub mod logging;
mod media;
mod media_fetch;
mod notifications;
mod oauth;
mod outgoing;
pub mod paths;
mod rich_text;
mod room_list;
mod rooms;
pub mod secrets;
mod security;
mod session;
mod spaces;
mod sync;
mod threads;
mod timeline;
mod upload;
mod verification;

pub use cpu_level::require_cpu_level;
pub use engine::{Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE};
