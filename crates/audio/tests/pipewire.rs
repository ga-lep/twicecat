//! Our PipeWire client against a private PipeWire daemon: null devices
//! only, its own socket, no session manager, no hardware, nothing of the
//! user's own audio graph. Needs the `pipewire`, `pw-cli`, `pw-link` and
//! `pw-metadata` binaries (Linux).
//!
//! Each test runs twice: the outer run starts the daemon and runs the same
//! test again in a child process pointed at it (`PIPEWIRE_RUNTIME_DIR`), so
//! the environment is set before PipeWire is first used.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use twicecat_audio::{
    AudioIo, DeviceList, DeviceWatcher, FRAME_LEN, Frame, IoConfig, SAMPLE_RATE, list_devices,
};

/// Set in the child: run the test body.
const CHILD: &str = "TWICECAT_PW_TEST_CHILD";

const CONFIG: &str = r#"
context.properties = {
    core.daemon = true
    core.name   = pipewire-0
    default.clock.rate = 48000
    default.clock.quantum = 480
    default.clock.min-quantum = 480
    default.clock.max-quantum = 480
    support.dbus = false
    mem.allow-mlock = false
}
context.spa-libs = {
    audio.convert.* = audioconvert/libspa-audioconvert
    audio.adapt     = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}
context.modules = [
    { name = libpipewire-module-rt flags = [ ifexists nofail ] }
    { name = libpipewire-module-protocol-native }
    { name = libpipewire-module-metadata }
    { name = libpipewire-module-spa-node-factory }
    { name = libpipewire-module-client-node }
    { name = libpipewire-module-access }
    { name = libpipewire-module-adapter }
    { name = libpipewire-module-link-factory }
]
stream.properties = {
    adapter.auto-port-config = { mode = dsp }
}
context.objects = [
    { factory = metadata
        args = { metadata.name = default }
    }
    { factory = spa-node-factory
        args = {
            factory.name    = support.node.driver
            node.name       = Dummy-Driver
            node.group      = pipewire.dummy
            priority.driver = 20000
        }
    }
    { factory = adapter
        args = {
            factory.name     = support.null-audio-sink
            node.name        = twicecat-test-sink
            node.description = "Test Speakers"
            media.class      = Audio/Sink
            audio.position   = [ MONO ]
            audio.rate       = 48000
            object.linger    = true
            adapter.auto-port-config = { mode = dsp monitor = true }
        }
    }
    { factory = adapter
        args = {
            factory.name     = support.null-audio-sink
            node.name        = twicecat-test-mic
            node.description = "Test Microphone"
            media.class      = Audio/Source/Virtual
            audio.position   = [ MONO ]
            audio.rate       = 48000
            object.linger    = true
            adapter.auto-port-config = { mode = dsp monitor = true }
        }
    }
]
"#;

/// The clients' configuration: without a session manager nobody configures
/// stream ports, so the streams do it themselves (`auto-port-config`). The
/// real-time module is loaded as in PipeWire's own `client.conf`.
const CLIENT_CONFIG: &str = r#"
context.properties = { log.level = 0 }
context.spa-libs = {
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}
context.modules = [
    { name = libpipewire-module-rt flags = [ ifexists nofail ] }
    { name = libpipewire-module-protocol-native }
    { name = libpipewire-module-client-node }
    { name = libpipewire-module-adapter }
    { name = libpipewire-module-metadata }
]
stream.properties = {
    adapter.auto-port-config = { mode = dsp }
}
"#;

/// The private daemon; killed and cleaned up on drop.
struct Daemon {
    child: Child,
    dir: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Daemon {
    fn start() -> Self {
        // One per daemon (tests run in parallel in one process); short: a
        // Unix socket path must fit in 108 bytes.
        static STARTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("twicecat-pw-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("test.conf");
        std::fs::write(&config, CONFIG).unwrap();
        std::fs::write(dir.join("client-test.conf"), CLIENT_CONFIG).unwrap();
        let child = Command::new("pipewire")
            .arg("-c")
            .arg(&config)
            .env("PIPEWIRE_RUNTIME_DIR", &dir)
            .env("XDG_RUNTIME_DIR", &dir)
            .env_remove("PIPEWIRE_REMOTE")
            .spawn()
            .expect("start pipewire (is it installed?)");
        let daemon = Self { child, dir };
        let socket = daemon.dir.join("pipewire-0");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(
                Instant::now() < deadline,
                "the private daemon did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }
}

/// In the outer run: starts a daemon, runs `test` again in a child pointed
/// at it, and returns `true` (the caller returns). In the child: `false`.
fn outer(test: &str) -> bool {
    outer_with(test, &[])
}

/// [`outer`], with more environment for the child.
fn outer_with(test: &str, env: &[(&str, &str)]) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let daemon = Daemon::start();
    let status = Command::new(std::env::current_exe().unwrap())
        .envs(env.iter().copied())
        .args([
            test,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env("PIPEWIRE_CONFIG_NAME", daemon.dir.join("client-test.conf"))
        .env("PIPEWIRE_RUNTIME_DIR", &daemon.dir)
        .env("XDG_RUNTIME_DIR", &daemon.dir)
        .env_remove("PIPEWIRE_REMOTE")
        .status()
        .unwrap();
    assert!(status.success(), "{test} failed in the private daemon");
    true
}

/// Runs a PipeWire tool against the daemon of this (child) process.
fn tool(program: &str, args: &[&str]) -> String {
    let out = Command::new(program).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn set_defaults() {
    for (key, node) in [
        ("default.audio.sink", "twicecat-test-sink"),
        ("default.audio.source", "twicecat-test-mic"),
    ] {
        tool(
            "pw-metadata",
            &[
                "-n",
                "default",
                "0",
                key,
                &format!(r#"{{"name":"{node}"}}"#),
                "Spa:String:JSON",
            ],
        );
    }
}

fn ids(list: &DeviceList) -> (Vec<String>, Vec<String>) {
    (
        list.inputs.iter().map(|d| d.id.clone()).collect(),
        list.outputs.iter().map(|d| d.id.clone()).collect(),
    )
}

#[test]
#[ignore = "needs the pipewire binaries; runs a private daemon"]
fn lists_devices_and_their_defaults_but_not_streams() {
    if outer("lists_devices_and_their_defaults_but_not_streams") {
        return;
    }
    set_defaults();
    // A stream of ours is in the graph; it is not a device.
    let io = AudioIo::start(
        IoConfig {
            playback: true,
            ..IoConfig::default()
        },
        |_: &Frame, _: &mut Frame| {},
    )
    .unwrap();
    let list = list_devices().unwrap();
    assert_eq!(
        ids(&list),
        (
            vec!["pipewire:twicecat-test-mic".to_owned()],
            vec!["pipewire:twicecat-test-sink".to_owned()]
        )
    );
    assert_eq!(list.inputs[0].name, "Test Microphone");
    assert!(list.inputs[0].is_default && list.outputs[0].is_default);
    let info = io.info().output.clone().unwrap();
    assert_eq!(info.device_id, "pipewire:twicecat-test-sink", "the default");
    assert_eq!((info.sample_rate, info.channels), (SAMPLE_RATE, 1));
}

#[test]
#[ignore = "needs the pipewire binaries; runs a private daemon"]
fn the_watcher_reports_devices_as_they_come_and_go() {
    if outer("the_watcher_reports_devices_as_they_come_and_go") {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let _watcher = DeviceWatcher::spawn(Duration::from_secs(3600), move |list| {
        let _ = tx.send(list);
    })
    .unwrap();
    let start = Instant::now();
    let created = tool(
        "pw-cli",
        &[
            "create-node",
            "adapter",
            "{ factory.name=support.null-audio-sink node.name=hotplug-headset \
             node.description=Headset media.class=Audio/Sink audio.position=[MONO] \
             object.linger=true }",
        ],
    );
    let added = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("no change reported");
    assert!(
        added
            .outputs
            .iter()
            .any(|d| d.id == "pipewire:hotplug-headset"),
        "{added:?}"
    );
    // An event, not the hour-long poll.
    assert!(start.elapsed() < Duration::from_secs(5));
    let _ = created;
    let id = node_id("hotplug-headset");
    tool("pw-cli", &["destroy", &id.to_string()]);
    let removed = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("removal not reported");
    assert!(
        removed
            .outputs
            .iter()
            .all(|d| d.id != "pipewire:hotplug-headset"),
        "{removed:?}"
    );
}

/// A node's global ID, from `pw-cli ls Node`.
fn node_id(name: &str) -> u32 {
    let listing = tool("pw-cli", &["ls", "Node"]);
    let mut current = None;
    for line in listing.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("id ") {
            current = rest.split(',').next().and_then(|n| n.parse().ok());
        } else if line == format!("node.name = \"{name}\"") {
            return current.expect("an id before the properties");
        }
    }
    panic!("no node {name} in {listing}");
}

/// The ports of a node, from `pw-link`.
fn ports(node: &str, outputs: bool) -> Vec<String> {
    let flag = if outputs { "-o" } else { "-i" };
    tool("pw-link", &[flag])
        .lines()
        .map(str::trim)
        .filter(|p| p.starts_with(&format!("{node}:")))
        .map(str::to_owned)
        .collect()
}

fn link(from_node: &str, to_node: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let from = ports(from_node, true);
        let to = ports(to_node, false);
        if let (Some(from), Some(to)) = (from.first(), to.first()) {
            let out = Command::new("pw-link").args([from, to]).output().unwrap();
            let err = String::from_utf8_lossy(&out.stderr);
            // Links between the lingering test nodes outlive a test step.
            assert!(
                out.status.success() || err.contains("File exists"),
                "pw-link {from} {to}: {err}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no ports to link {from_node} -> {to_node}; outputs: {:?}, inputs: {:?}, nodes: {}",
            tool("pw-link", &["-o"]),
            tool("pw-link", &["-i"]),
            tool("pw-cli", &["ls", "Node"])
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Amplitude of `freq` in `samples` at 48 kHz.
fn tone_amplitude(samples: &[f32], freq: f32) -> f32 {
    let (mut re, mut im) = (0.0_f64, 0.0_f64);
    for (n, s) in samples.iter().enumerate() {
        let phase =
            2.0 * std::f64::consts::PI * f64::from(freq) * n as f64 / f64::from(SAMPLE_RATE);
        re += f64::from(*s) * phase.cos();
        im += f64::from(*s) * phase.sin();
    }
    (2.0 * (re * re + im * im).sqrt() / samples.len() as f64) as f32
}

#[test]
#[ignore = "needs the pipewire binaries; runs a private daemon"]
fn a_tone_goes_out_and_comes_back_through_the_graph() {
    if outer("a_tone_goes_out_and_comes_back_through_the_graph") {
        return;
    }
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let sink = heard.clone();
    let mut phase = 0.0_f32;
    let io = AudioIo::start(
        IoConfig {
            capture: true,
            input_device: Some("pipewire:twicecat-test-mic".into()),
            playback: true,
            output_device: Some("pipewire:twicecat-test-mic".into()),
            echo_reference: false,
        },
        move |capture: &Frame, playback: &mut Frame| {
            sink.lock().unwrap().extend_from_slice(capture);
            for s in playback.iter_mut() {
                *s = 0.3 * phase.sin();
                phase += 2.0 * std::f32::consts::PI * 1000.0 / SAMPLE_RATE as f32;
                if phase > 2.0 * std::f32::consts::PI {
                    phase -= 2.0 * std::f32::consts::PI;
                }
            }
        },
    );
    // A virtual source plays back what is written to it: our playback goes
    // in, our capture comes out. No session manager runs, so we link.
    let io = io.unwrap();
    link("twicecat-playback", "twicecat-test-mic");
    link("twicecat-test-mic", "twicecat-capture");
    std::thread::sleep(Duration::from_millis(1500));
    let stats = io.stats();
    drop(io);
    let heard = heard.lock().unwrap();
    assert!(
        heard.len() > FRAME_LEN * 50,
        "{} samples, {stats:?}",
        heard.len()
    );
    let tail = &heard[heard.len() - SAMPLE_RATE as usize / 2..];
    let amplitude = tone_amplitude(tail, 1000.0);
    eprintln!("tone back at {amplitude:.4} (0.3 sent), {stats:?}");
    assert!((amplitude - 0.3).abs() < 0.03, "{amplitude}");
}

/// Music-like "game" sound: three tones, slowly modulated.
fn game_sample(n: u64) -> f32 {
    let t = n as f32 / SAMPLE_RATE as f32;
    let tau = 2.0 * std::f32::consts::PI;
    let envelope = 0.6 + 0.4 * (tau * 0.7 * t).sin();
    0.12 * envelope * ((tau * 220.0 * t).sin() + (tau * 554.0 * t).sin() + (tau * 1318.0 * t).sin())
}

/// Starts `pw-cat` playing the game sound (raw from stdin) as node
/// `test-game`; stops when dropped.
struct Game {
    child: Child,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl Game {
    fn start() -> Self {
        use std::io::Write;
        let mut child = Command::new("pw-cat")
            .args([
                "--playback",
                "--raw",
                "--format",
                "f32",
                "--rate",
                "48000",
                "--channels",
                "1",
                "--latency",
                "20ms",
                "-P",
                "{ node.name = test-game node.autoconnect = false }",
                "-",
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("pw-cat");
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || {
            let mut n = 0_u64;
            let mut block = Vec::with_capacity(FRAME_LEN * 4);
            loop {
                block.clear();
                for _ in 0..FRAME_LEN {
                    block.extend_from_slice(&game_sample(n).to_le_bytes());
                    n += 1;
                }
                // pw-cat paces the pipe; a closed pipe ends the loop.
                if stdin.write_all(&block).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            writer: Some(writer),
        }
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn rms_db(samples: &[f32]) -> f32 {
    let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    10.0 * mean.max(1e-12).log10()
}

/// What our microphone sends while a game plays on the same speakers it
/// hears (`twicecat-test-sink`'s monitor feeds the test microphone): RMS of
/// the last two seconds sent, dBFS, and the microphone's level.
fn sent_while_a_game_plays(echo_reference: bool) -> (f32, f32) {
    use twicecat_audio::{ChainSettings, GateMode, NoiseMode, VoiceEngine};
    let settings = ChainSettings {
        echo_cancellation: true,
        high_pass: true,
        auto_gain: false,
        auto_gain_max_db: 30.0,
        noise: NoiseMode::Off,
        // Always open: we measure the echo canceller, not the gate.
        gate: GateMode::VoiceActivity {
            automatic: false,
            threshold_db: -100.0,
        },
        input_gain_db: 0.0,
    };
    let (engine, control, mut outgoing) = VoiceEngine::new(settings);
    let io = AudioIo::start(
        IoConfig {
            capture: true,
            input_device: Some("pipewire:twicecat-test-mic".into()),
            playback: true,
            output_device: Some("pipewire:twicecat-test-sink".into()),
            echo_reference,
        },
        engine,
    )
    .unwrap();
    assert_eq!(io.info().echo_reference.is_some(), echo_reference);
    let game = Game::start();
    link("test-game", "twicecat-test-sink");
    link("twicecat-test-sink", "twicecat-test-mic");
    link("twicecat-test-mic", "twicecat-capture");
    link("twicecat-playback", "twicecat-test-sink");
    if echo_reference {
        link("twicecat-test-sink", "twicecat-echo-reference");
    }
    let mut sent = Vec::new();
    let mut mic_levels = Vec::new();
    let end = Instant::now() + Duration::from_secs(6);
    let mut frame = [0.0; FRAME_LEN];
    while Instant::now() < end {
        while outgoing.pop_frame(&mut frame) {
            sent.extend_from_slice(&frame);
        }
        mic_levels.push(control.meters().input_rms_db);
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(game);
    drop(io);
    assert!(
        sent.len() > SAMPLE_RATE as usize * 4,
        "{} samples sent",
        sent.len()
    );
    let tail = &sent[sent.len() - SAMPLE_RATE as usize * 2..];
    let mic = mic_levels[mic_levels.len() / 2];
    (rms_db(tail), mic)
}

#[test]
#[ignore = "needs the pipewire binaries; runs a private daemon"]
fn the_system_mix_reference_cancels_a_games_sound() {
    if outer("the_system_mix_reference_cancels_a_games_sound") {
        return;
    }
    let (without, mic_without) = sent_while_a_game_plays(false);
    let (with, mic_with) = sent_while_a_game_plays(true);
    eprintln!(
        "game heard by the mic, sent: {without:.1} dBFS with the call's own reference \
         (meter {mic_without:.1}), {with:.1} dBFS with the system mix (meter {mic_with:.1}); \
         {:.1} dB less",
        without - with
    );
    assert!(
        without > -30.0,
        "the game reaches the microphone: {without}"
    );
    assert!(without - with > 20.0, "only {:.1} dB less", without - with);
}

/// Every core busy (twice over, at normal priority, as a game and its
/// compilers might), while a call-like processor (about 150 µs per frame)
/// runs for 5 s: how late the processing thread starts after the audio
/// comes. Prints the numbers; asserts only that a granted real-time
/// thread stays on time.
fn on_time_under_load() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use twicecat_audio::Realtime;

    let io = AudioIo::start(
        IoConfig {
            capture: true,
            input_device: Some("pipewire:twicecat-test-mic".into()),
            playback: true,
            output_device: Some("pipewire:twicecat-test-mic".into()),
            echo_reference: false,
        },
        move |capture: &Frame, playback: &mut Frame| {
            let started = Instant::now();
            let mut x = capture[0];
            while started.elapsed() < Duration::from_micros(150) {
                x = std::hint::black_box(x * 0.999 + 0.001);
            }
            playback[0] = x * 0.0;
        },
    )
    .unwrap();
    link("twicecat-playback", "twicecat-test-mic");
    link("twicecat-test-mic", "twicecat-capture");
    std::thread::sleep(Duration::from_millis(1000));

    let busy = Arc::new(AtomicBool::new(true));
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let load: Vec<_> = (0..cores * 2)
        .map(|i| {
            let busy = busy.clone();
            std::thread::spawn(move || {
                let mut x = i as f64;
                while busy.load(Ordering::Relaxed) {
                    x = std::hint::black_box(x.mul_add(1.000_001, 1e-9));
                }
            })
        })
        .collect();
    let before = io.stats();
    io.take_recent_max_wake_us();
    std::thread::sleep(Duration::from_secs(5));
    let after = io.stats();
    let worst_loaded = io.take_recent_max_wake_us();
    busy.store(false, Ordering::Relaxed);
    for thread in load {
        thread.join().unwrap();
    }
    let realtime = io.info().realtime.clone();
    drop(io);

    let wakes = after.wakes - before.wakes;
    let late = after.late_wakes - before.late_wakes;
    let frames = after.frames - before.frames;
    eprintln!(
        "{realtime:?}, {} busy threads: {frames} frames, {wakes} wakes, {late} over 1 ms, \
         worst {worst_loaded} µs under load ({} µs whole run), capture overruns {}, \
         playback underruns {}",
        cores * 2,
        after.max_wake_us,
        after.capture_overruns - before.capture_overruns,
        after.playback_underruns - before.playback_underruns,
    );
    assert!(frames > 400, "processing stalled: {after:?}");
    assert!(worst_loaded > 0 && worst_loaded <= after.max_wake_us);
    if let Realtime::Granted { .. } = realtime {
        assert!(late * 100 <= wakes, "{late} of {wakes} wakes over 1 ms");
        assert_eq!(after.capture_overruns, before.capture_overruns);
    }
}

#[test]
#[ignore = "needs the pipewire binaries; runs a private daemon"]
fn processing_stays_on_time_under_load_with_real_time_priority() {
    if outer("processing_stays_on_time_under_load_with_real_time_priority") {
        return;
    }
    on_time_under_load();
}

#[test]
#[ignore = "needs the pipewire binaries; runs a private daemon"]
fn processing_under_load_at_normal_priority_for_comparison() {
    if outer_with(
        "processing_under_load_at_normal_priority_for_comparison",
        &[("TWICECAT_AUDIO_REALTIME", "0")],
    ) {
        return;
    }
    on_time_under_load();
}
