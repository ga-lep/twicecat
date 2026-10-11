//! Opus cost per 10 ms of audio, for the call path's CPU budget (plan Task
//! 15): the Opus our calls use (built into libwebrtc, linked through
//! LiveKit), with WebRTC's settings (48 kHz mono, VOIP, in-band FEC, DTX,
//! 20 ms packets, complexity 9 on desktop Linux and the Steam Frame, 5 on
//! Android and iOS, 10 below about 12.5 kb/s), on a voice in noise, on room
//! noise (the gate open, nobody talking) and on digital silence (muted, push
//! to talk released, the gate closed); encoding on the thread as WebRTC runs
//! it, and with denormals flushed (x86: FTZ/DAZ). Decoding is per remote
//! voice.
//!
//! Run: `cargo run --release -p twicecat-core --example opus_profile [seconds]`
//!
//! On a hybrid CPU (Intel P- and E-cores) run it on an E-core too
//! (`taskset -c <cpu>`): WebRTC's encoder thread runs there, and Opus on
//! digital silence costs about seven times more there (denormals).

// Links libwebrtc (and the Opus inside it) without using LiveKit's API here.
extern crate livekit as _;

use std::ffi::{c_int, c_void};
use std::time::{Duration, Instant};

const RATE: i32 = 48_000;
/// One 20 ms packet, as WebRTC sends.
const PACKET: usize = 960;

const OPUS_APPLICATION_VOIP: c_int = 2048;
const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
const OPUS_SET_COMPLEXITY_REQUEST: c_int = 4010;
const OPUS_SET_INBAND_FEC_REQUEST: c_int = 4012;
const OPUS_SET_DTX_REQUEST: c_int = 4016;

// libopus's C API (opus.h), as built into libwebrtc.
unsafe extern "C" {
    fn opus_encoder_create(
        fs: i32,
        channels: c_int,
        application: c_int,
        error: *mut c_int,
    ) -> *mut c_void;
    fn opus_encoder_ctl(st: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_encode(
        st: *mut c_void,
        pcm: *const i16,
        frame_size: c_int,
        data: *mut u8,
        max_data_bytes: i32,
    ) -> i32;
    fn opus_encoder_destroy(st: *mut c_void);
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut c_void;
    fn opus_decode(
        st: *mut c_void,
        data: *const u8,
        len: i32,
        pcm: *mut i16,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;
    fn opus_decoder_destroy(st: *mut c_void);
}

/// An Opus encoder with WebRTC's settings.
struct Encoder(*mut c_void);

impl Encoder {
    fn new(bitrate: i32, complexity: i32) -> Self {
        let mut error = 0;
        // SAFETY: valid arguments (48 kHz, mono, a known application); the
        // error code is written to a live local.
        let st = unsafe { opus_encoder_create(RATE, 1, OPUS_APPLICATION_VOIP, &mut error) };
        assert!(!st.is_null() && error == 0, "opus_encoder_create: {error}");
        for (request, value) in [
            (OPUS_SET_BITRATE_REQUEST, bitrate),
            (OPUS_SET_COMPLEXITY_REQUEST, complexity),
            (OPUS_SET_INBAND_FEC_REQUEST, 1),
            (OPUS_SET_DTX_REQUEST, 1),
        ] {
            // SAFETY: `st` is a live encoder; each of these requests takes
            // one opus_int32.
            let res = unsafe { opus_encoder_ctl(st, request, value) };
            assert_eq!(res, 0, "opus_encoder_ctl({request})");
        }
        Self(st)
    }

    /// Encodes one 20 ms packet into `out`; returns its length (0 or 1
    /// byte: DTX, nothing to send).
    fn encode(&mut self, pcm: &[i16; PACKET], out: &mut [u8; 1500]) -> usize {
        // SAFETY: `self.0` is a live encoder; `pcm` holds one packet of mono
        // samples and `out` its stated size.
        let n = unsafe {
            opus_encode(
                self.0,
                pcm.as_ptr(),
                PACKET as c_int,
                out.as_mut_ptr(),
                1500,
            )
        };
        usize::try_from(n).expect("opus_encode failed")
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_encoder_create, destroyed once.
        unsafe { opus_encoder_destroy(self.0) }
    }
}

struct Decoder(*mut c_void);

impl Decoder {
    fn new() -> Self {
        let mut error = 0;
        // SAFETY: as for the encoder.
        let st = unsafe { opus_decoder_create(RATE, 1, &mut error) };
        assert!(!st.is_null() && error == 0, "opus_decoder_create: {error}");
        Self(st)
    }

    /// Decodes one packet (empty: lost or DTX, concealed).
    fn decode(&mut self, packet: &[u8], pcm: &mut [i16; PACKET]) {
        let data = if packet.is_empty() {
            std::ptr::null()
        } else {
            packet.as_ptr()
        };
        // SAFETY: `self.0` is a live decoder; `data` is null or `packet`
        // (its length passed); `pcm` holds one packet.
        let n = unsafe {
            opus_decode(
                self.0,
                data,
                packet.len() as i32,
                pcm.as_mut_ptr(),
                PACKET as c_int,
                0,
            )
        };
        assert_eq!(n, PACKET as c_int, "opus_decode");
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_decoder_create, destroyed once.
        unsafe { opus_decoder_destroy(self.0) }
    }
}

#[derive(Clone, Copy)]
enum Signal {
    /// Harmonics of 140 Hz in syllables (4 Hz) over noise, as
    /// `voice_profile`.
    VoiceInNoise,
    /// Noise at about -50 dBFS: the gate open, nobody talking.
    RoomNoise,
    /// Zeros: muted, push to talk released, the gate closed.
    DigitalSilence,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Self::VoiceInNoise => "voice in noise",
            Self::RoomNoise => "room noise",
            Self::DigitalSilence => "digital silence",
        }
    }

    fn samples(self, len: usize) -> Vec<i16> {
        let mut seed = 0x9e37_79b9_u32;
        (0..len)
            .map(|n| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                let t = n as f32 / RATE as f32;
                let x = match self {
                    Self::VoiceInNoise => {
                        let syllable = (std::f32::consts::TAU * 4.0 * t).sin().max(0.0);
                        let voice: f32 = (1..=12)
                            .map(|h| {
                                (std::f32::consts::TAU * 140.0 * h as f32 * t).sin() / h as f32
                            })
                            .sum();
                        0.1 * syllable * voice + 0.03 * noise
                    }
                    Self::RoomNoise => 0.003 * noise,
                    Self::DigitalSilence => 0.0,
                };
                (x * 32767.0) as i16
            })
            .collect()
    }
}

/// Mean time per 10 ms of audio (two per packet) and the share of packets
/// sent (not DTX).
struct Cost {
    encode: Duration,
    decode: Duration,
    sent: f32,
}

fn measure(signal: &[i16], bitrate: i32, complexity: i32) -> Cost {
    let mut encoder = Encoder::new(bitrate, complexity);
    let mut decoder = Decoder::new();
    let mut out = [0_u8; 1500];
    let mut pcm = [0_i16; PACKET];
    let (mut encode, mut decode, mut sent) = (Duration::ZERO, Duration::ZERO, 0_usize);
    let packets = signal.chunks_exact(PACKET);
    let count = packets.len();
    // One second first, so the encoder's state (and its denormals) settles.
    for (i, chunk) in packets.enumerate() {
        let chunk: &[i16; PACKET] = chunk.try_into().unwrap();
        let start = Instant::now();
        let len = encoder.encode(chunk, &mut out);
        let encoded = start.elapsed();
        let start = Instant::now();
        decoder.decode(if len > 2 { &out[..len] } else { &[] }, &mut pcm);
        let decoded = start.elapsed();
        if i >= 50 {
            encode += encoded;
            decode += decoded;
            sent += usize::from(len > 2);
        }
    }
    let tens = 2 * (count - 50) as u32;
    Cost {
        encode: encode / tens,
        decode: decode / tens,
        sent: sent as f32 / (count - 50) as f32,
    }
}

fn main() {
    let seconds: usize = std::env::args()
        .nth(1)
        .map_or(10, |s| s.parse().expect("seconds"));
    let len = (seconds + 1) * RATE as usize / PACKET * PACKET;
    let signals = [
        Signal::VoiceInNoise,
        Signal::RoomNoise,
        Signal::DigitalSilence,
    ];
    let audio: Vec<Vec<i16>> = signals.iter().map(|s| s.samples(len)).collect();
    println!("Opus in libwebrtc, per 10 ms of audio, mean over {seconds} s (after 1 s)");
    for flushed in [false, true] {
        if flushed {
            twicecat_audio::flush_denormals_on_this_thread();
        }
        println!(
            "\n-- denormals {}",
            if flushed {
                "flushed (FTZ/DAZ)"
            } else {
                "as on WebRTC's encoder thread"
            }
        );
        for (bitrate, complexity) in [(64_000, 9), (32_000, 9), (32_000, 5), (12_000, 10)] {
            for (signal, samples) in signals.iter().zip(&audio) {
                let cost = measure(samples, bitrate, complexity);
                println!(
                    "{:>3} kb/s complexity {complexity:>2}, {:<15}: encode {:>6.1} µs, decode {:>5.1} µs, {:>3.0} % of packets sent",
                    bitrate / 1000,
                    signal.name(),
                    cost.encode.as_secs_f64() * 1e6,
                    cost.decode.as_secs_f64() * 1e6,
                    cost.sent * 100.0,
                );
            }
        }
    }
}
