//! Link and ABI smoke test for the prebuilt libwebrtc behind livekit.
//!
//! libwebrtc.a is built against Chromium's libc++, whose `trivial_abi`
//! smart pointers change the calling convention; a mismatched C++ compiler
//! would hand back garbage pointers here. Needs no server or audio device,
//! except the ignored playback test.

use livekit::webrtc::native::apm::AudioProcessingModule;
use livekit::webrtc::native::create_random_uuid;

#[test]
fn libwebrtc_returns_valid_objects_across_ffi() {
    let uuid = create_random_uuid();
    assert_eq!(uuid.len(), 36, "uuid has the 8-4-4-4-12 form");
    assert_ne!(uuid, create_random_uuid());
}

#[test]
fn libwebrtc_apm_processes_ten_ms_frames() {
    const RATE: i32 = 48_000;
    let mut apm = AudioProcessingModule::new(true, true, true, true);
    const FRAME: usize = (RATE / 100) as usize;
    let mut render = vec![0i16; FRAME];
    let mut capture = vec![0i16; FRAME];
    for frame in 0..50 {
        for (i, s) in render.iter_mut().enumerate() {
            // A quiet 1 kHz tone; the exact signal doesn't matter here.
            let t = (frame * FRAME + i) as f32 / RATE as f32;
            *s = (1000.0 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin()) as i16;
        }
        capture.copy_from_slice(&render);
        apm.process_reverse_stream(&mut render, RATE, 1).unwrap();
        apm.set_stream_delay_ms(10).unwrap();
        apm.process_stream(&mut capture, RATE, 1).unwrap();
    }
}

/// libwebrtc's weak `pw_*` stubs took libpipewire's place in this binary
/// until `scripts/fetch-libwebrtc.sh` renamed them: opening a PipeWire
/// stream then crashed in `pw_init` (address 0).
#[test]
#[ignore = "needs an audio output device; plays 300 ms of silence"]
fn hardware_audio_output_opens_in_a_binary_linking_libwebrtc() {
    let io = twicecat_audio::AudioIo::start(
        twicecat_audio::IoConfig {
            playback: true,
            ..twicecat_audio::IoConfig::default()
        },
        |_: &_, _: &mut _| {},
    )
    .expect("default output device");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(io.stats().frames >= 20, "{:?}", io.stats());
    assert!(io.info().output.is_some());
}
