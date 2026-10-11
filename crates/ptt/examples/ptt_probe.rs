//! Prints push-to-talk transitions for a binding, for checking hotkeys,
//! the portal and gamepads on a real desktop (human checkpoint):
//!
//! ```text
//! cargo run -p twicecat-ptt --example ptt_probe -- --key ctrl+shift+KeyT --button left_thumb
//! ```
//!
//! `--keyboard portal|hotkey|window` overrides the detected mechanism. Stop
//! with Ctrl+C. Windows and macOS need an event loop on the main thread for
//! hotkeys, which this probe does not run: use it there for gamepads only.

use std::sync::Arc;
use std::time::Instant;

use twicecat_ptt::{KeyboardBackend, PttConfig, PttService};

fn main() -> Result<(), String> {
    let mut config = PttConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--key" => config.key = Some(value.parse().map_err(|e| format!("{e}"))?),
            "--button" => config.gamepad = Some(value.parse().map_err(|e| format!("{e}"))?),
            "--keyboard" => {
                config.keyboard = match value.as_str() {
                    "portal" => KeyboardBackend::Portal,
                    "hotkey" => KeyboardBackend::Hotkey,
                    "window" => KeyboardBackend::WindowOnly,
                    other => return Err(format!("unknown keyboard mechanism: {other}")),
                }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    println!("keyboard: {:?}", config.keyboard);
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let start = Instant::now();
    let service = PttService::start(
        config,
        runtime.handle(),
        Arc::new(move |event| {
            println!("{:>8.3} s  {event:?}", start.elapsed().as_secs_f64());
        }),
    );
    runtime
        .block_on(tokio::signal::ctrl_c())
        .map_err(|e| e.to_string())?;
    drop(service);
    Ok(())
}
