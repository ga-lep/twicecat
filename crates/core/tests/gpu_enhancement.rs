//! GPU speech enhancement through the core API (feature `gpu-enhancement`):
//! a mic test on virtual devices with DeepFilterNet3 on the GPU, the status
//! event, the probe, and a new attenuation limit applied without a reload.
//! Needs the GPU parts (scripts/fetch-onnxruntime.sh cuda13,
//! fetch-models.sh, export-deepfilternet.sh, NVIDIA's libraries) and a GPU.

#![cfg(feature = "gpu-enhancement")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{expect_event, expect_event_within, start_core_with_audio};
use tessera_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, AudioProcessing, AudioSettings, Command, CoreEvent,
    EnhancerBackend, GpuBackend, NoiseSuppression, VirtualAudio,
};
use tessera_core::secrets::InMemorySecretStore;

/// A quiet hiss on the microphone; playback is ignored.
struct Hiss(std::sync::Mutex<u32>);

impl VirtualAudio for Hiss {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        let mut seed = self.0.lock().unwrap();
        for s in frame.iter_mut() {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            *s = (*seed as f32 / u32::MAX as f32 - 0.5) * 0.02;
        }
    }
    fn playback(&self, _: &[f32; AUDIO_FRAME_LEN]) {}
}

fn deep_filter(attenuation_limit_db: f32, gpu: bool) -> AudioSettings {
    AudioSettings {
        processing: AudioProcessing {
            noise_suppression: NoiseSuppression::DeepFilter {
                attenuation_limit_db,
                gpu,
            },
            ..AudioProcessing::default()
        },
        ..AudioSettings::default()
    }
}

#[tokio::test]
#[ignore = "needs the GPU parts and a GPU"]
async fn deep_filter_runs_on_the_gpu_and_takes_a_new_limit_without_reloading() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core_with_audio(
        dir.path(),
        Arc::new(InMemorySecretStore::new()),
        AudioBackend::Virtual(Arc::new(Hiss(std::sync::Mutex::new(0x9e37_79b9)))),
    );
    let send = |cmd| core.commands.send(cmd);
    send(Command::UpdateAudioSettings(deep_filter(100.0, true)))
        .await
        .unwrap();
    send(Command::StartMicTest { loopback: false })
        .await
        .unwrap();

    let gpu = |e: &CoreEvent| matches!(e, CoreEvent::GpuEnhancement(_));
    let CoreEvent::GpuEnhancement(loaded) =
        expect_event_within(&mut core, Duration::from_secs(30), gpu).await
    else {
        unreachable!()
    };
    eprintln!("after loading: {loaded:?}");
    assert_eq!(
        loaded.running,
        Some(EnhancerBackend::Gpu {
            backend: GpuBackend::Cuda
        })
    );

    core.commands
        .send(Command::ProbeGpuEnhancement)
        .await
        .unwrap();
    let CoreEvent::GpuEnhancement(probed) = expect_event(&mut core, gpu).await else {
        unreachable!()
    };
    eprintln!("probe: {probed:?}");
    assert!(probed.available.contains(&GpuBackend::Cuda));
    assert_eq!(probed.running, loaded.running, "the probe left it running");

    // A new limit goes to the running filter: no reload, so no new status.
    core.commands
        .send(Command::UpdateAudioSettings(deep_filter(30.0, true)))
        .await
        .unwrap();
    let quiet = tokio::time::timeout(
        Duration::from_secs(3),
        expect_event(&mut core, |e| {
            matches!(e, CoreEvent::GpuEnhancement(_) | CoreEvent::Error(_))
        }),
    )
    .await;
    assert!(quiet.is_err(), "unexpected {quiet:?}");

    core.commands.send(Command::StopMicTest).await.unwrap();
    core.commands.send(Command::Shutdown).await.unwrap();
}
