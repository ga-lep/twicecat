//! Our own PipeWire client on Linux (plan decision 12): streams with the
//! call role and a 10 ms quantum at 48 kHz, devices from the registry (no
//! other applications' streams), and device changes from registry events
//! instead of polling. cpal remains for systems without a PipeWire daemon.

mod graph;
mod registry;
mod streams;
mod thread;

use std::path::Path;

pub(crate) use registry::{Watcher, list};
pub(crate) use streams::{Streams, open};
pub(crate) use thread::{PwThread, make_current_realtime, spawn as spawn_thread};

/// Whether a PipeWire daemon's socket exists for this session.
pub(crate) fn available() -> bool {
    let socket = |dir: &str| Path::new(dir).join("pipewire-0").exists();
    let remote = std::env::var("PIPEWIRE_REMOTE").ok();
    if let Some(remote) = remote.as_deref()
        && remote.starts_with('/')
    {
        return Path::new(remote).exists();
    }
    ["PIPEWIRE_RUNTIME_DIR", "XDG_RUNTIME_DIR"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .any(|dir| socket(&dir))
}
