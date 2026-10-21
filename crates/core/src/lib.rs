//! Twicecat core: a headless Matrix client engine.

// The SDK's timeline types nest deeply; without this rustc fails with
// "queries overflow the depth limit" (same as the SDK's `examples/timeline`).
#![recursion_limit = "256"]

pub mod api;
#[cfg(target_os = "linux")]
mod appimage_update;
mod audio_session;
mod avatars;
mod call;
mod call_settings;
mod call_sounds;
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
mod ringtone;
mod room_list;
mod rooms;
pub mod secrets;
mod security;
mod session;
mod spaces;
mod sync;
mod threads;
mod timeline;
mod updates;
mod upload;
mod verification;
mod video_settings;

#[cfg(target_os = "linux")]
pub use appimage_update::prepare_tls as prepare_appimage_tls;
pub use cpu_level::require_cpu_level;
pub use engine::{Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE};
