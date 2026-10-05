//! Prints the devices and what the audio I/O does when they change, for
//! checking device following on a real desktop (human checkpoint):
//!
//! ```text
//! cargo run -p tessera-audio --example device_probe -- [--mic] [--seconds N]
//! ```
//!
//! It plays a faint 440 Hz tone (-40 dBFS) on the default output (on
//! Windows the default communications device) and, with `--mic`, opens the
//! default microphone and prints its level. While it runs, change the
//! default device or unplug the one in use: the stream should move
//! (`switched ...`) and the tone carry on, and the device list should be
//! printed again at once. Stops after `--seconds` (default 60).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tessera_audio::{AudioIo, DeviceList, DeviceWatcher, Frame, IoConfig, IoEvent, list_devices};

/// A tone out, and the microphone's peak in.
struct Probe {
    phase: f32,
    peak: Arc<AtomicU32>,
}

impl tessera_audio::FrameProcessor for Probe {
    fn process(&mut self, capture: &Frame, playback: &mut Frame) {
        let peak = capture.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        let step = 440.0 * std::f32::consts::TAU / tessera_audio::SAMPLE_RATE as f32;
        for s in playback.iter_mut() {
            *s = 0.01 * self.phase.sin();
            self.phase = (self.phase + step) % std::f32::consts::TAU;
        }
    }
}

fn print_list(list: &DeviceList) {
    for (kind, devices) in [("input", &list.inputs), ("output", &list.outputs)] {
        for d in devices {
            let default = if d.is_default { " (default)" } else { "" };
            println!("  {kind}: {}{default}", d.name);
        }
    }
}

fn main() -> Result<(), String> {
    let mut mic = false;
    let mut seconds = 60;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mic" => mic = true,
            "--seconds" => {
                seconds = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--seconds needs a number")?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let start = Instant::now();
    println!("devices:");
    print_list(&list_devices().map_err(|e| e.to_string())?);
    let _watcher = DeviceWatcher::spawn(Duration::from_secs(2), move |list| {
        println!("{:>6.1} s  devices changed:", start.elapsed().as_secs_f32());
        print_list(&list);
    })
    .map_err(|e| e.to_string())?;
    let peak = Arc::new(AtomicU32::new(0));
    let io = AudioIo::start(
        IoConfig {
            capture: mic,
            playback: true,
            ..IoConfig::default()
        },
        Probe {
            phase: 0.0,
            peak: peak.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    let info = io.info();
    if let Some(input) = &info.input {
        println!("input: {}", input.device_name);
    }
    if let Some(output) = &info.output {
        println!("output: {}", output.device_name);
    }
    println!("priority: {:?}", info.realtime);
    let end = start + Duration::from_secs(seconds);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_secs(1));
        let at = start.elapsed().as_secs_f32();
        while let Some(event) = io.try_event() {
            match event {
                IoEvent::Switched { input, device } => println!(
                    "{at:>6.1} s  switched {} to {}",
                    if input { "input" } else { "output" },
                    device.device_name
                ),
                other => println!("{at:>6.1} s  {other:?}"),
            }
        }
        let stats = io.stats();
        let level = f32::from_bits(peak.swap(0, Ordering::Relaxed));
        let mic_level = if mic {
            format!(", mic peak {level:.3}")
        } else {
            String::new()
        };
        println!(
            "{at:>6.1} s  frames {}, underruns {}{mic_level}",
            stats.frames, stats.playback_underruns
        );
    }
    Ok(())
}
