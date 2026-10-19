//! A screen share's sound on Linux (plan Task 8): what other applications
//! play, without our own call audio.
//!
//! The portal's screen cast has no sound, and a sink's monitor would carry
//! the call we play back too (the other people would hear themselves). So
//! we open one stereo capture stream that the session manager leaves alone
//! (`node.autoconnect = false`) and link it ourselves to the output ports
//! of every application stream (`Stream/Output/Audio`) except our own
//! process's. PipeWire mixes what is linked to an input port. Streams that
//! come and go while sharing are followed from the registry.
//!
//! "Our process" is the one behind our capture stream's client, by the
//! process ID PipeWire read from the socket (`pipewire.sec.pid`): every
//! connection we make shares it, and it is the daemon's view, so it holds
//! inside a Flatpak sandbox too (where our own `getpid` differs). Nothing is
//! linked before it is known.
//!
//! The samples are handed over on the stream's thread (the main loop, not
//! the real-time data loop), 48 kHz stereo `f32`, interleaved.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use pipewire as pw;
use pw::context::ContextRc;
use pw::core::CoreRc;
use pw::link::Link;
use pw::main_loop::MainLoopRc;
use pw::properties::properties;
use pw::spa::param::ParamType;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{Object, Pod, Value};
use pw::spa::utils::{Direction, SpaTypes};
use pw::stream::{StreamFlags, StreamRc, StreamState};
use pw::types::ObjectType;

use crate::capture::CaptureError;

/// The sample rate handed over.
pub const SAMPLE_RATE: u32 = 48_000;
/// Channels, interleaved.
pub const CHANNELS: u32 = 2;

/// Where the samples go: interleaved stereo at [`SAMPLE_RATE`], in blocks
/// of the graph's size.
pub type AudioSink = Box<dyn FnMut(&[f32]) + Send>;

/// A node we may link from or to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeInfo {
    media_class: String,
    /// The client (connection) that made it.
    client: Option<u32>,
}

/// A port of a node.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PortInfo {
    node: u32,
    output: bool,
    monitor: bool,
    channel: String,
}

/// What we keep of the graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Graph {
    /// Clients and their process, by ID.
    clients: BTreeMap<u32, u32>,
    nodes: BTreeMap<u32, NodeInfo>,
    ports: BTreeMap<u32, PortInfo>,
}

/// Which of our two input channels a source channel feeds.
fn sides(channel: &str) -> &'static [&'static str] {
    match channel {
        "FL" | "SL" | "RL" | "SIDE_L" | "REAR_L" => &["FL"],
        "FR" | "SR" | "RR" | "SIDE_R" | "REAR_R" => &["FR"],
        // Mono, centre, LFE and the unnamed feed both.
        _ => &["FL", "FR"],
    }
}

impl Graph {
    /// The process behind a node, when its client is known.
    fn process_of(&self, node: u32) -> Option<u32> {
        let client = self.nodes.get(&node)?.client?;
        self.clients.get(&client).copied()
    }

    /// The links we want: (output port, input port of `ours`), from every
    /// application output stream that is not from our own process (the
    /// one behind `ours`). None while that process is not known.
    fn wanted(&self, ours: u32) -> BTreeSet<(u32, u32)> {
        let Some(own) = self.process_of(ours) else {
            return BTreeSet::new();
        };
        let inputs: Vec<(u32, &str)> = self
            .ports
            .iter()
            .filter(|(_, p)| p.node == ours && !p.output)
            .map(|(id, p)| (*id, p.channel.as_str()))
            .collect();
        let mut out = BTreeSet::new();
        for (id, port) in &self.ports {
            if !port.output || port.monitor || port.node == ours {
                continue;
            }
            let Some(node) = self.nodes.get(&port.node) else {
                continue;
            };
            // A stream whose process is not known yet waits: it may be ours.
            let process = self.process_of(port.node);
            if node.media_class != "Stream/Output/Audio" || process.is_none_or(|p| p == own) {
                continue;
            }
            for side in sides(&port.channel) {
                for (input, channel) in &inputs {
                    if channel == side {
                        out.insert((*id, *input));
                    }
                }
            }
        }
        out
    }
}

/// The share's sound, captured until dropped.
pub struct ShareAudio {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ShareAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareAudio").finish_non_exhaustive()
    }
}

impl Drop for ShareAudio {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl ShareAudio {
    /// Starts capturing what other applications play; samples go to
    /// `sink`.
    ///
    /// # Errors
    /// [`CaptureError::Unavailable`] without a PipeWire daemon,
    /// [`CaptureError::Failed`] when the stream cannot be made.
    pub fn start(sink: AudioSink) -> Result<Self, CaptureError> {
        let (stop, stopped) = pw::channel::channel::<()>();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("twicecat-share-audio".into())
            .spawn(move || run(sink, stopped, &ready_tx))
            .map_err(|_| CaptureError::Failed("no thread for the share's sound"))?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(CaptureError::Failed("the share's sound thread stopped"))
            }
        }
    }
}

fn format_param() -> Option<Vec<u8>> {
    let mut audio = AudioInfoRaw::new();
    audio.set_format(AudioFormat::F32LE);
    audio.set_rate(SAMPLE_RATE);
    audio.set_channels(CHANNELS);
    let mut position = [0; pw::spa::param::audio::MAX_CHANNELS];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
    audio.set_position(position);
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio.into(),
    };
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

/// Our links, kept alive (dropping a proxy destroys its link).
#[derive(Default)]
struct Linker {
    graph: Graph,
    ours: Option<u32>,
    links: BTreeMap<(u32, u32), Link>,
}

impl Linker {
    /// Makes the links the graph wants and drops the others.
    fn update(&mut self, core: &CoreRc) {
        let Some(ours) = self.ours else {
            return;
        };
        let wanted = self.graph.wanted(ours);
        self.links.retain(|key, _| wanted.contains(key));
        for (output, input) in wanted {
            if self.links.contains_key(&(output, input)) {
                continue;
            }
            let (Some(out_port), Some(in_port)) =
                (self.graph.ports.get(&output), self.graph.ports.get(&input))
            else {
                continue;
            };
            let props = properties! {
                "link.output.node" => out_port.node.to_string(),
                "link.output.port" => output.to_string(),
                "link.input.node" => in_port.node.to_string(),
                "link.input.port" => input.to_string(),
                "object.linger" => "false",
            };
            match core.create_object::<Link>("link-factory", &props) {
                Ok(link) => {
                    self.links.insert((output, input), link);
                }
                Err(_) => tracing::debug!("a share-sound link could not be made"),
            }
        }
    }
}

fn run(
    mut sink: AudioSink,
    stopped: pw::channel::Receiver<()>,
    ready: &mpsc::SyncSender<Result<(), CaptureError>>,
) {
    pw::init();
    let failed = |what| {
        let _ = ready.send(Err(CaptureError::Failed(what)));
    };
    let Ok(mainloop) = MainLoopRc::new(None) else {
        return failed("no PipeWire main loop");
    };
    let Ok(context) = ContextRc::new(&mainloop, None) else {
        return failed("no PipeWire context");
    };
    let Ok(core) = context.connect_rc(None) else {
        let _ = ready.send(Err(CaptureError::Unavailable("no PipeWire daemon")));
        return;
    };
    let Ok(registry) = core.get_registry_rc() else {
        return failed("no PipeWire registry");
    };
    let linker = Rc::new(RefCell::new(Linker::default()));

    let _registry = registry
        .add_listener_local()
        .global({
            let linker = linker.clone();
            let core = core.clone();
            move |global| {
                let Some(props) = global.props else {
                    return;
                };
                let mut l = linker.borrow_mut();
                match global.type_ {
                    ObjectType::Client => {
                        let Some(pid) = props.get("pipewire.sec.pid").and_then(|p| p.parse().ok())
                        else {
                            return;
                        };
                        l.graph.clients.insert(global.id, pid);
                    }
                    ObjectType::Node => {
                        let Some(media_class) = props.get("media.class") else {
                            return;
                        };
                        l.graph.nodes.insert(
                            global.id,
                            NodeInfo {
                                media_class: media_class.to_owned(),
                                client: props.get("client.id").and_then(|c| c.parse().ok()),
                            },
                        );
                    }
                    ObjectType::Port => {
                        let (Some(node), Some(direction)) = (
                            props.get("node.id").and_then(|n| n.parse().ok()),
                            props.get("port.direction"),
                        ) else {
                            return;
                        };
                        l.graph.ports.insert(
                            global.id,
                            PortInfo {
                                node,
                                output: direction == "out",
                                monitor: props.get("port.monitor") == Some("true"),
                                channel: props.get("audio.channel").unwrap_or("MONO").to_owned(),
                            },
                        );
                    }
                    _ => return,
                }
                l.update(&core);
            }
        })
        .global_remove({
            let linker = linker.clone();
            move |id| {
                let mut l = linker.borrow_mut();
                l.graph.clients.remove(&id);
                let node = l.graph.nodes.remove(&id).is_some();
                let port = l.graph.ports.remove(&id).is_some();
                if node || port {
                    // Links to a port that is gone are gone too.
                    l.links.retain(|(o, i), _| *o != id && *i != id);
                }
            }
        })
        .register();

    let props = properties! {
        "media.type" => "Audio",
        "media.category" => "Capture",
        "media.role" => "Production",
        "node.name" => "twicecat-share-audio",
        "node.description" => "Twicecat screen-share sound",
        "node.autoconnect" => "false",
        "application.name" => "Twicecat",
    };
    let Ok(stream) = StreamRc::new(core.clone(), "twicecat-share-audio", props) else {
        return failed("could not make the PipeWire stream");
    };
    let Some(format) = format_param() else {
        return failed("could not describe the share's sound");
    };
    let listener = stream
        .add_local_listener_with_user_data(())
        .state_changed({
            let linker = linker.clone();
            let core = core.clone();
            move |stream, _, _, new| {
                if matches!(new, StreamState::Paused | StreamState::Streaming) {
                    let mut l = linker.borrow_mut();
                    if l.ours.is_none() {
                        l.ours = Some(stream.node_id());
                        l.update(&core);
                    }
                }
            }
        })
        .process(move |stream, ()| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else {
                return;
            };
            let end = offset.saturating_add(size).min(bytes.len());
            let start = offset.min(end);
            let samples: Vec<f32> = bytes[start..end]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            if !samples.is_empty() {
                sink(&samples);
            }
        })
        .register();
    let Ok(_listener) = listener else {
        return failed("could not listen to the PipeWire stream");
    };
    let Some(pod) = Pod::from_bytes(&format) else {
        return failed("could not describe the share's sound");
    };
    if stream
        .connect(Direction::Input, None, StreamFlags::MAP_BUFFERS, &mut [pod])
        .is_err()
    {
        return failed("could not connect the PipeWire stream");
    }
    let _ = ready.send(Ok(()));
    let quit = mainloop.clone();
    let _stop = stopped.attach(mainloop.loop_(), move |()| quit.quit());
    mainloop.run();
    linker.borrow_mut().links.clear();
    let _ = stream.disconnect();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node of `client`; client N belongs to process N × 100.
    fn node(class: &str, client: u32) -> NodeInfo {
        NodeInfo {
            media_class: class.into(),
            client: Some(client),
        }
    }

    fn port(node: u32, output: bool, channel: &str) -> PortInfo {
        PortInfo {
            node,
            output,
            monitor: false,
            channel: channel.into(),
        }
    }

    /// Ours is node 1 (client 1, process 100; inputs 10 FL, 11 FR); a game
    /// (node 2, process 200) in stereo, a mono app (node 3, process 300),
    /// our own call playback (node 4, client 4 of process 100: another
    /// connection of ours), a sink (node 5) and its monitor.
    fn graph() -> Graph {
        let mut g = Graph::default();
        g.clients
            .extend([(1, 100), (2, 200), (3, 300), (4, 100), (5, 1)]);
        g.nodes.insert(1, node("Stream/Input/Audio", 1));
        g.nodes.insert(2, node("Stream/Output/Audio", 2));
        g.nodes.insert(3, node("Stream/Output/Audio", 3));
        g.nodes.insert(4, node("Stream/Output/Audio", 4));
        g.nodes.insert(5, node("Audio/Sink", 5));
        g.ports.insert(10, port(1, false, "FL"));
        g.ports.insert(11, port(1, false, "FR"));
        g.ports.insert(20, port(2, true, "FL"));
        g.ports.insert(21, port(2, true, "FR"));
        g.ports.insert(30, port(3, true, "MONO"));
        g.ports.insert(40, port(4, true, "MONO"));
        g.ports.insert(50, port(5, false, "FL"));
        g.ports.insert(
            51,
            PortInfo {
                monitor: true,
                ..port(5, true, "FL")
            },
        );
        g
    }

    #[test]
    fn every_application_but_ours_is_linked_channel_by_channel() {
        let wanted = graph().wanted(1);
        assert_eq!(
            wanted,
            BTreeSet::from([(20, 10), (21, 11), (30, 10), (30, 11)])
        );
    }

    #[test]
    fn nothing_is_linked_while_our_process_or_theirs_is_unknown() {
        // Our client not seen yet: we cannot tell our streams apart.
        let mut g = graph();
        g.clients.remove(&1);
        assert!(g.wanted(1).is_empty());
        // A stream whose client is not seen yet waits.
        let mut g = graph();
        g.clients.remove(&3);
        assert_eq!(g.wanted(1), BTreeSet::from([(20, 10), (21, 11)]));
    }

    #[test]
    fn nothing_is_linked_before_our_ports_exist() {
        let mut g = graph();
        g.ports.retain(|_, p| p.node != 1);
        assert!(g.wanted(1).is_empty());
    }

    #[test]
    fn surround_channels_fold_onto_their_side() {
        assert_eq!(sides("RL"), &["FL"]);
        assert_eq!(sides("SR"), &["FR"]);
        assert_eq!(sides("FC"), &["FL", "FR"]);
        assert_eq!(sides("LFE"), &["FL", "FR"]);
    }

    /// The power of `freq` Hz in `samples` (one channel at 48 kHz).
    fn goertzel(samples: &[f32], freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / SAMPLE_RATE as f32;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0_f32, 0.0_f32);
        for &x in samples {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2) / samples.len().max(1) as f32
    }

    /// A 16-bit stereo WAV of `freq` Hz for `secs` seconds.
    fn wav(freq: f32, secs: u32) -> Vec<u8> {
        let frames = SAMPLE_RATE * secs;
        let data_len = frames * 4;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&2_u16.to_le_bytes());
        out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        out.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
        out.extend_from_slice(&4_u16.to_le_bytes());
        out.extend_from_slice(&16_u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let t = i as f32 / SAMPLE_RATE as f32;
            let v = ((2.0 * std::f32::consts::PI * freq * t).sin() * 12_000.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// In this process: a null sink (so the test plays on no speakers) and
    /// a 440 Hz stream into it, "our call", until `stop`.
    fn null_sink_and_our_tone(
        ready: mpsc::Sender<()>,
        stop: pw::channel::Receiver<()>,
    ) -> JoinHandle<()> {
        std::thread::spawn(move || {
            pw::init();
            let mainloop = MainLoopRc::new(None).expect("loop");
            let context = ContextRc::new(&mainloop, None).expect("context");
            let core = context.connect_rc(None).expect("PipeWire daemon");
            let _sink = core
                .create_object::<pw::node::Node>(
                    "adapter",
                    &properties! {
                        "factory.name" => "support.null-audio-sink",
                        "node.name" => "twicecat-test-null",
                        "media.class" => "Audio/Sink",
                        "audio.position" => "[ FL FR ]",
                        "object.linger" => "false",
                    },
                )
                .expect("null sink");
            let props = properties! {
                "media.type" => "Audio",
                "media.category" => "Playback",
                "node.name" => "twicecat-test-our-call",
                "target.object" => "twicecat-test-null",
            };
            let stream =
                StreamRc::new(core.clone(), "twicecat-test-our-call", props).expect("stream");
            let phase = std::cell::Cell::new(0_u64);
            let _listener = stream
                .add_local_listener_with_user_data(())
                .process(move |stream, ()| {
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };
                    let Some(data) = buffer.datas_mut().first_mut() else {
                        return;
                    };
                    let mut written = 0;
                    if let Some(bytes) = data.data() {
                        let frames = (bytes.len() / 8).min(1024);
                        for f in 0..frames {
                            let n = phase.get() + f as u64;
                            let t = n as f32 / SAMPLE_RATE as f32;
                            let v = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.4;
                            bytes[f * 8..f * 8 + 4].copy_from_slice(&v.to_le_bytes());
                            bytes[f * 8 + 4..f * 8 + 8].copy_from_slice(&v.to_le_bytes());
                        }
                        phase.set(phase.get() + frames as u64);
                        written = frames;
                    }
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = 8;
                    *chunk.size_mut() = (written * 8) as u32;
                })
                .register()
                .expect("listener");
            let format = format_param().expect("format");
            let pod = Pod::from_bytes(&format).expect("pod");
            stream
                .connect(
                    Direction::Output,
                    None,
                    StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
                    &mut [pod],
                )
                .expect("connect");
            let _ = ready.send(());
            let quit = mainloop.clone();
            let _stop = stop.attach(mainloop.loop_(), move |()| quit.quit());
            mainloop.run();
            let _ = stream.disconnect();
        })
    }

    #[test]
    #[ignore = "needs a PipeWire daemon and pw-cat"]
    fn the_share_hears_other_applications_and_not_our_call() {
        let dir = std::env::temp_dir().join(format!("twicecat-share-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let tone = dir.join("other-app.wav");
        std::fs::write(&tone, wav(1000.0, 6)).expect("wav");

        let (ready_tx, ready) = mpsc::channel();
        let (stop, stopped) = pw::channel::channel::<()>();
        let ours = null_sink_and_our_tone(ready_tx, stopped);
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("our tone");

        // Another application: a separate process.
        let Ok(mut other) = std::process::Command::new("pw-cat")
            .args(["--playback", "--target", "twicecat-test-null"])
            .arg(&tone)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            eprintln!("pw-cat is not installed; skipped");
            let _ = stop.send(());
            let _ = ours.join();
            return;
        };

        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let into = heard.clone();
        let share = ShareAudio::start(Box::new(move |samples| {
            into.lock().expect("lock").extend_from_slice(samples)
        }))
        .expect("share audio");
        std::thread::sleep(std::time::Duration::from_secs(3));
        drop(share);
        let _ = other.kill();
        let _ = other.wait();
        let _ = stop.send(());
        let _ = ours.join();
        let _ = std::fs::remove_dir_all(&dir);

        let samples = heard.lock().expect("lock").clone();
        // The left channel of the last 2 s (links take a moment).
        let left: Vec<f32> = samples.chunks_exact(2).map(|f| f[0]).collect();
        let tail = &left[left.len().saturating_sub(SAMPLE_RATE as usize * 2)..];
        assert!(
            tail.len() >= SAMPLE_RATE as usize,
            "only {} samples heard",
            tail.len()
        );
        let other_app = goertzel(tail, 1000.0);
        let our_call = goertzel(tail, 440.0);
        assert!(
            other_app > 1.0,
            "the other application is not heard: {other_app}"
        );
        assert!(
            our_call * 100.0 < other_app,
            "our call is in the share: 440 Hz {our_call}, 1 kHz {other_app}"
        );
    }
}
