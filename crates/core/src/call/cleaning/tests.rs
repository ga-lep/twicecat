//! The call's cleaning manager against a real voice engine and a fake GPU
//! bank: the switch from a bank that gives up to the CPU filters.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use twicecat_audio::{
    AudioError, BatchCleaner, ChainSettings, DeepFilterBackend, FRAME_LEN, Frame, FrameProcessor,
    GateMode, LEVEL_FLOOR_DB, NoiseMode, SILENCE, SourceInput, VoiceControl, VoiceEngine,
};

use super::*;
use crate::api::SkipCleaned;

/// A bank that cleans nothing and fails on demand.
struct FakeBank {
    fail: Arc<AtomicBool>,
}

impl BatchCleaner for FakeBank {
    fn reset_row(&mut self, _row: usize) -> Result<(), AudioError> {
        Ok(())
    }
    fn clean(&mut self, _frames: &mut [Frame], _rows: &[usize]) -> Result<(), AudioError> {
        if self.fail.load(Ordering::Relaxed) {
            Err(AudioError::Internal)
        } else {
            Ok(())
        }
    }
}

struct Rig {
    engine: VoiceEngine,
    cleaning: Cleaning,
    voice: Voice,
    sources: Vec<(SourceId, SourceInput)>,
    fail: Arc<AtomicBool>,
    status: DeepFilterStatus,
    bank_loads: Arc<AtomicUsize>,
    model_loads: Arc<AtomicUsize>,
    backend: CleaningBackend,
    flags: SharedFlags,
}

fn plain() -> ChainSettings {
    ChainSettings {
        echo_cancellation: false,
        high_pass: false,
        auto_gain: false,
        noise: NoiseMode::Off,
        gate: GateMode::VoiceActivity {
            automatic: false,
            threshold_db: LEVEL_FLOOR_DB,
        },
        input_gain_db: 0.0,
    }
}

fn settings(gpu: bool) -> RemoteCleaning {
    RemoteCleaning {
        enabled: true,
        skip_cleaned: SkipCleaned::Never,
        gpu,
        ..RemoteCleaning::default()
    }
}

/// `real_model`: load the DeepFilterNet3 model from the model directory
/// instead of finding none.
fn rig(real_model: bool) -> Rig {
    let (engine, control, _out) = VoiceEngine::new(plain());
    let voice: Voice = Arc::new(Mutex::new(control));
    let fail = Arc::new(AtomicBool::new(false));
    let status = DeepFilterStatus::new(DeepFilterBackend::Gpu);
    let bank_loads = Arc::new(AtomicUsize::new(0));
    let model_loads = Arc::new(AtomicUsize::new(0));
    let loaders = Loaders {
        bank: {
            let (fail, status, loads) = (fail.clone(), status.clone(), bank_loads.clone());
            Arc::new(move |_| {
                loads.fetch_add(1, Ordering::Relaxed);
                let bank = FakeBank { fail: fail.clone() };
                Ok(Some((
                    Box::new(bank) as Box<dyn BatchCleaner>,
                    GpuBackend::Cuda,
                    status.clone(),
                )))
            })
        },
        model: {
            let loads = model_loads.clone();
            Arc::new(move || {
                loads.fetch_add(1, Ordering::Relaxed);
                if real_model {
                    DeepFilterModel::load(&twicecat_audio::default_model_dir()?).ok()
                } else {
                    None
                }
            })
        },
    };
    let backend = CleaningBackend::default();
    let flags = SharedFlags::default();
    let cleaning = Cleaning::with_loaders(
        Some(voice.clone()),
        settings(true),
        BTreeMap::new(),
        flags.clone(),
        Some(backend.clone()),
        loaders,
    );
    Rig {
        engine,
        cleaning,
        voice,
        sources: Vec::new(),
        fail,
        status,
        bank_loads,
        model_loads,
        backend,
        flags,
    }
}

impl Rig {
    fn join(&mut self, name: &str) -> usize {
        let (id, input) = voice_control(&self.voice)
            .add_source(100)
            .expect("a source fits");
        self.cleaning
            .add(&format!("@{name}:test:DEV"), id, input.cleaning_flag());
        self.sources.push((id, input));
        self.sources.len() - 1
    }

    /// One 10 ms frame of every voice talking, and 10 ms of (paused) time.
    async fn frame(&mut self) {
        for (_, input) in &mut self.sources {
            input.push(&[0.4; FRAME_LEN]);
        }
        let mut out = SILENCE;
        self.engine.process(&SILENCE, &mut out);
        tokio::time::advance(Duration::from_millis(10)).await;
        // The loads run on blocking threads: give them real time.
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    async fn frames(&mut self, millis: u64) {
        for _ in 0..millis / 10 {
            self.frame().await;
        }
    }

    fn states(&self) -> Vec<State> {
        self.cleaning
            .lock()
            .voices
            .values()
            .map(|e| e.state)
            .collect()
    }

    fn gpu_gave_up(&self) -> Option<EnhancerBackend> {
        match self.cleaning.lock().gpu {
            Gpu::GaveUp(why) => Some(why),
            _ => None,
        }
    }

    fn statuses(&self) -> Vec<CleaningStatus> {
        let flags = self.flags.flags.read().unwrap();
        self.cleaning
            .lock()
            .voices
            .keys()
            .map(|identity| flags.cleaning[identity])
            .collect()
    }
}

/// Two voices on rows, then the bank fails; `why` is set on its status
/// first.
async fn gives_up(why: Option<DeepFilterBackend>) -> Rig {
    let mut rig = rig(false);
    rig.join("a");
    rig.join("b");
    rig.frames(100).await;
    assert_eq!(rig.states(), [State::Row(0), State::Row(1)]);
    assert!(
        rig.sources
            .iter()
            .all(|(_, i)| i.cleaning() == SourceCleaning::Running)
    );
    if let Some(why) = why {
        rig.status.set(why);
    }
    rig.fail.store(true, Ordering::Relaxed);
    rig.frames(2000).await;
    rig
}

#[tokio::test(start_paused = true)]
async fn a_bank_that_fails_hands_its_voices_to_the_cpu() {
    let rig = gives_up(None).await;
    assert_eq!(
        rig.gpu_gave_up(),
        Some(EnhancerBackend::CpuAfterGpuError),
        "the manager noticed"
    );
    assert!(
        rig.states().iter().all(|s| !matches!(s, State::Row(_))),
        "no voice stays on a row: {:?}",
        rig.states()
    );
    assert!(
        rig.model_loads.load(Ordering::Relaxed) >= 1,
        "the CPU model was asked for"
    );
    for (status, (_, input)) in rig.statuses().iter().zip(&rig.sources) {
        assert!(
            !(*status == CleaningStatus::Cleaned && input.cleaning() == SourceCleaning::None),
            "a voice shown cleaned that is not"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_bank_too_slow_says_so() {
    let rig = gives_up(Some(DeepFilterBackend::CpuAfterGpuTooSlow)).await;
    assert_eq!(rig.gpu_gave_up(), Some(EnhancerBackend::CpuAfterGpuTooSlow));
}

#[tokio::test(start_paused = true)]
async fn the_gpu_setting_turned_off_and_on_loads_a_new_bank_after_a_give_up() {
    let mut rig = gives_up(None).await;
    assert_eq!(rig.bank_loads.load(Ordering::Relaxed), 1);
    rig.cleaning.set_settings(settings(false), BTreeMap::new());
    rig.frames(100).await;
    rig.fail.store(false, Ordering::Relaxed);
    rig.cleaning.set_settings(settings(true), BTreeMap::new());
    rig.frames(100).await;
    assert_eq!(rig.bank_loads.load(Ordering::Relaxed), 2);
    assert_eq!(rig.gpu_gave_up(), None);
}

#[tokio::test(start_paused = true)]
async fn a_voice_that_leaves_frees_its_row() {
    let mut rig = rig(false);
    rig.join("a");
    rig.frames(100).await;
    assert_eq!(rig.states(), [State::Row(0)]);
    rig.cleaning.remove("@a:test:DEV");
    rig.join("b");
    rig.frames(100).await;
    assert_eq!(rig.states(), [State::Row(0)], "the next voice gets row 0");
}

#[tokio::test(start_paused = true)]
#[ignore = "needs scripts/fetch-models.sh"]
async fn the_cpu_filters_take_over_after_a_give_up() {
    let mut rig = rig(true);
    rig.join("a");
    rig.join("b");
    rig.frames(100).await;
    rig.fail.store(true, Ordering::Relaxed);
    // The model compiles and the filters are made on blocking threads.
    for _ in 0..4000 {
        rig.frame().await;
        if rig
            .sources
            .iter()
            .all(|(_, i)| i.cleaning() == SourceCleaning::Running)
            && rig.gpu_gave_up().is_some()
        {
            break;
        }
    }
    assert_eq!(rig.gpu_gave_up(), Some(EnhancerBackend::CpuAfterGpuError));
    assert_eq!(rig.states(), [State::Cleaned, State::Cleaned]);
    rig.frames(100).await;
    for (_, input) in &rig.sources {
        assert_eq!(input.cleaning(), SourceCleaning::Running);
    }
    assert_eq!(
        *rig.backend.lock().unwrap(),
        Some(EnhancerBackend::CpuAfterGpuError)
    );
    let _: &VoiceControl = &voice_control(&rig.voice);
}
