//! Tessera desktop app: the Iced shell on top of `tessera-core`.
//!
//! The app talks to the core only through its command and event API (see
//! `tessera_core::api`); it never depends on the Matrix SDK.

pub mod bridge;
pub mod room_list;
pub mod state;
pub mod theme;

pub use bridge::{Bridge, BridgeEvent, EventsSlot};
pub use state::{App, CommandSink, Message, Screen, Startup};
pub const TITLE: &str = "Tessera";
