//! Twicecat desktop app: the Iced shell on top of `twicecat-core`.
//!
//! The app talks to the core only through its command and event API (see
//! `twicecat_core::api`); it never depends on the Matrix SDK.

pub mod actions;
pub mod audio_settings;
pub mod avatars;
pub mod blocking;
pub mod bridge;
pub mod calls;
pub mod composer;
pub mod dates;
pub mod dialogs;
pub mod emoji;
pub mod keys;
pub mod links;
pub mod media;
pub mod notifications;
pub mod ptt;
pub mod rich_text;
pub mod room_list;
pub mod rooms;
pub mod screens;
pub mod security;
pub mod sessions;
pub mod settings;
pub mod spaces;
pub mod state;
pub mod switcher;
pub mod text_safety;
pub mod theme;
pub mod threads;
pub mod timeline;
pub mod tray;
pub mod upload;
pub mod verification;

pub use bridge::{Bridge, BridgeEvent, EventsSlot};
pub use state::{App, CommandSink, Message, Screen, Startup};
pub const TITLE: &str = "Twicecat";
