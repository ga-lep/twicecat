//! The engine's audio and call side: audio settings and devices, the mic
//! test, and the call (joining, leaving, muting, SFU notices, declining a
//! ring).

use std::time::Duration;

use super::{Engine, blocking, parse_room_id};
use crate::api::{
    AudioDevice, AudioDeviceId, AudioDevices, AudioSettings, CallEndReason, CallState, CoreError,
    CoreEvent, RoomId,
};
use crate::audio_session::{AudioSession, Mode};
use crate::call::watcher::CallWatch;
use crate::call::{CallAudio, CallSession, MediaNotice};
use crate::call_settings;

/// The current call's next SFU notice, or forever without a call (or once
/// its notices end).
pub(super) async fn next_media_notice(call: &mut Option<CallSession>) -> MediaNotice {
    match call {
        Some(call) => match call.notices.recv().await {
            Some(notice) => notice,
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

/// How often the device watcher re-lists audio devices.
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn audio_device(info: twicecat_audio::DeviceInfo) -> AudioDevice {
    AudioDevice {
        id: AudioDeviceId::from(info.id),
        name: info.name,
        is_default: info.is_default,
    }
}

fn audio_devices(list: twicecat_audio::DeviceList) -> AudioDevices {
    AudioDevices {
        inputs: list.inputs.into_iter().map(audio_device).collect(),
        outputs: list.outputs.into_iter().map(audio_device).collect(),
    }
}

impl Engine {
    /// Probes the GPU backends off the async threads and reports them with
    /// where the microphone's DeepFilterNet runs now.
    pub(super) fn probe_gpu(&self) {
        let running = self
            .audio_session
            .as_ref()
            .and_then(|s| s.enhancer_running());
        let events = self.events.clone();
        tokio::spawn(async move {
            let available = tokio::task::spawn_blocking(|| {
                twicecat_audio::default_model_dir()
                    .map(|dir| crate::gpu_enhancement::probe(&dir))
                    .unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let event = crate::api::GpuEnhancement { available, running };
            let _ = events.send(CoreEvent::GpuEnhancement(event)).await;
        });
    }

    /// Lists audio devices (blocking enumeration off the async threads) and,
    /// the first time, starts the hot-plug watcher, which sends
    /// `AudioDevices` again whenever the list changes.
    pub(super) async fn list_audio_devices(&mut self) {
        match blocking(twicecat_audio::list_devices).await {
            Ok(Ok(list)) => {
                self.emit(CoreEvent::AudioDevices(audio_devices(list)))
                    .await
            }
            Ok(Err(_)) | Err(_) => {
                self.emit(CoreEvent::Error(CoreError::Audio(
                    "audio devices could not be listed".into(),
                )))
                .await;
            }
        }
        if self.device_watcher.is_none() {
            let events = self.events.clone();
            let watcher = twicecat_audio::DeviceWatcher::spawn(DEVICE_POLL_INTERVAL, move |list| {
                // A plain thread, not a runtime one, so blocking is fine; a
                // closed channel means the engine is gone.
                let _ = events.blocking_send(CoreEvent::AudioDevices(audio_devices(list)));
            });
            match watcher {
                Ok(w) => self.device_watcher = Some(w),
                Err(_) => tracing::warn!("audio device watcher could not start"),
            }
        }
    }

    /// The audio settings, loading them from disk on first use. A file that
    /// cannot be used is reported once and replaced by the defaults in memory
    /// (the file itself stays until the next save).
    pub(super) async fn audio_settings(&mut self) -> &AudioSettings {
        if self.audio_settings.is_none() {
            let paths = self.config.data.clone();
            let loaded = blocking(move || call_settings::load(&paths))
                .await
                .unwrap_or_else(|e| call_settings::Loaded {
                    settings: AudioSettings::default(),
                    problem: Some(e),
                });
            if let Some(problem) = loaded.problem {
                self.emit(CoreEvent::Error(problem)).await;
            }
            self.audio_settings = Some(loaded.settings);
        }
        self.audio_settings.as_ref().expect("loaded above")
    }

    /// Adopts already validated settings, saves them, and echoes them back.
    /// A failed save is reported; the settings still apply for this run.
    pub(super) async fn store_audio_settings(&mut self, settings: AudioSettings) {
        let paths = self.config.data.clone();
        let to_save = settings.clone();
        let saved = blocking(move || call_settings::save(&paths, &to_save))
            .await
            .and_then(|r| r);
        self.audio_settings = Some(settings.clone());
        if let Ok(mut shared) = self.user_volumes.write() {
            shared.clone_from(&settings.user_volumes);
        }
        if let Some(call) = &self.call {
            call.set_cleaning(&settings);
        }
        if let Err(e) = saved {
            self.emit(CoreEvent::Error(e)).await;
        }
        self.emit(CoreEvent::AudioSettings(settings.clone())).await;
        // A running session follows; a device change reopens it.
        if let Some(session) = &self.audio_session
            && !session.apply(&settings)
        {
            match session.mode() {
                Mode::MicTest { loopback } => self.start_mic_test(loopback).await,
                // Rewiring a running call's media to new devices is a
                // follow-up; the new devices are used from the next call.
                Mode::Call => {
                    self.emit(CoreEvent::Error(CoreError::Audio(
                        "the new audio devices are used from the next call".into(),
                    )))
                    .await;
                }
            }
        }
    }

    /// Joins `room_id`'s call, leaving another call first.
    pub(super) async fn join_call(&mut self, room_id: RoomId) {
        let Some(active) = self.active.as_ref() else {
            self.emit(CoreEvent::Error(CoreError::NotLoggedIn)).await;
            return;
        };
        let client = active.client.clone();
        if let Some(call) = &self.call {
            if call.room_id == room_id {
                let encrypted = call.encrypted;
                self.emit(CoreEvent::CallState(CallState::Connected {
                    room_id,
                    encrypted,
                }))
                .await;
                return;
            }
            self.leave_call().await;
        }
        // Participants carry the per-user volumes.
        let volumes = self.audio_settings().await.user_volumes.clone();
        if let Ok(mut shared) = self.user_volumes.write() {
            *shared = volumes;
        }
        self.mic_muted = false;
        self.deafened = false;
        self.emit(CoreEvent::CallState(CallState::Joining {
            room_id: room_id.clone(),
        }))
        .await;
        let audio = self.start_call_audio().await;
        match CallSession::join(
            &client,
            room_id.clone(),
            audio,
            self.events.clone(),
            self.user_volumes.clone(),
            self.config.call_service_override.as_deref(),
        )
        .await
        {
            Ok(session) => {
                let encrypted = session.encrypted;
                if let Some(calls) = self.calls() {
                    calls.set_sent(&room_id, session.notification.clone());
                    if calls.take_ring(&room_id).is_some() {
                        // Answered here.
                        self.emit(CoreEvent::RingingStopped {
                            room_id: room_id.clone(),
                        })
                        .await;
                    }
                }
                self.call = Some(session);
                self.emit(CoreEvent::CallState(CallState::Connected {
                    room_id,
                    encrypted,
                }))
                .await;
            }
            Err(e) => {
                self.stop_call_audio().await;
                self.emit(CoreEvent::CallState(CallState::Left {
                    room_id,
                    reason: CallEndReason::Failed(e.to_string()),
                }))
                .await;
                self.emit(CoreEvent::Error(e)).await;
            }
        }
    }

    /// Opens the audio devices for a call (replacing a mic test). Without
    /// usable devices the call still joins, silent, and the error is shown.
    async fn start_call_audio(&mut self) -> CallAudio {
        if let Some(session) = self.audio_session.take() {
            session.stop().await;
        }
        let settings = self.audio_settings().await.clone();
        let started = AudioSession::start(
            &settings,
            Mode::Call,
            &self.config.audio,
            self.events.clone(),
        )
        .await;
        let (outgoing, voice, cleaning_backend) = match started {
            Ok((session, outgoing)) => {
                let handles = (Some(session.voice()), Some(session.remote_cleaning()));
                self.audio_session = Some(session);
                (outgoing, handles.0, handles.1)
            }
            Err(e) => {
                self.emit(CoreEvent::Error(e)).await;
                (None, None, None)
            }
        };
        CallAudio {
            voice,
            outgoing,
            bitrate_kbps: settings.voice_bitrate_kbps,
            cleaning: (settings.remote_cleaning, settings.user_cleaning.clone()),
            announce: crate::call::announcement(&settings),
            cleaning_backend,
        }
    }

    /// Closes the call's audio devices, if they are open.
    async fn stop_call_audio(&mut self) {
        if self
            .audio_session
            .as_ref()
            .is_some_and(|s| s.mode() == Mode::Call)
            && let Some(session) = self.audio_session.take()
        {
            session.stop().await;
        }
    }

    /// Applies mute and deafen (deafen also mutes, as in Discord).
    pub(super) fn apply_mute(&self) {
        let muted = self.mic_muted || self.deafened;
        if let Some(session) = &self.audio_session {
            session.set_muted(muted);
            session.set_deafened(self.deafened);
        }
        if let Some(call) = &self.call {
            call.set_muted(muted);
        }
    }

    /// Reacts to the SFU connection of the current call.
    pub(super) async fn on_media_notice(&mut self, notice: MediaNotice) {
        let Some(call) = &self.call else {
            return;
        };
        let room_id = call.room_id.clone();
        match notice {
            MediaNotice::Reconnecting => {
                self.call_reconnects += 1;
                self.emit(CoreEvent::CallState(CallState::Reconnecting {
                    room_id,
                    attempt: self.call_reconnects,
                }))
                .await;
            }
            MediaNotice::Reconnected => {
                self.call_reconnects = 0;
                let encrypted = self.call.as_ref().is_some_and(|c| c.encrypted);
                self.emit(CoreEvent::CallState(CallState::Connected {
                    room_id,
                    encrypted,
                }))
                .await;
            }
            MediaNotice::Disconnected => {
                if let Some(call) = self.call.take() {
                    if let Some(calls) = self.calls() {
                        calls.set_sent(&call.room_id, None);
                    }
                    call.leave().await;
                }
                self.stop_call_audio().await;
                self.emit(CoreEvent::CallState(CallState::Left {
                    room_id,
                    reason: CallEndReason::Failed("the call server connection was lost".into()),
                }))
                .await;
            }
        }
    }

    /// The session's call watcher, while syncing.
    pub(super) fn calls(&self) -> Option<&CallWatch> {
        self.active.as_ref()?.sync.as_ref().map(|s| &s.calls)
    }

    /// Declines the ring in `room_id`, on every device of ours.
    pub(super) async fn decline_call(&mut self, room_id: RoomId) {
        let Some(active) = self.active.as_ref() else {
            self.emit(CoreEvent::Error(CoreError::NotLoggedIn)).await;
            return;
        };
        let Some(notification) = self.calls().and_then(|c| c.take_ring(&room_id)) else {
            self.emit(CoreEvent::Error(CoreError::Call(
                "nothing is ringing in this room".into(),
            )))
            .await;
            return;
        };
        let room = parse_room_id(&room_id)
            .ok()
            .and_then(|id| active.client.get_room(&id));
        let sent = match room {
            Some(room) => room
                .send_raw(
                    crate::call::ringing::DECLINE_EVENT,
                    crate::call::ringing::decline_content(&notification),
                )
                .await
                .is_ok(),
            None => false,
        };
        self.emit(CoreEvent::RingingStopped {
            room_id: room_id.clone(),
        })
        .await;
        if !sent {
            self.emit(CoreEvent::Error(CoreError::Call(
                "could not send the decline; the caller keeps ringing".into(),
            )))
            .await;
        }
    }

    /// Leaves the current call, if any.
    pub(super) async fn leave_call(&mut self) {
        if let Some(call) = self.call.take() {
            let room_id = call.room_id.clone();
            if let Some(calls) = self.calls() {
                calls.set_sent(&room_id, None);
            }
            call.leave().await;
            self.stop_call_audio().await;
            self.emit(CoreEvent::CallState(CallState::Left {
                room_id,
                reason: CallEndReason::Hangup,
            }))
            .await;
        }
    }

    /// Opens the microphone (and, with `loopback`, the speakers) and starts
    /// reporting levels; replaces a running mic test.
    pub(super) async fn start_mic_test(&mut self, loopback: bool) {
        if let Some(session) = self.audio_session.take() {
            session.stop().await;
        }
        if self.call.is_some() {
            self.emit(CoreEvent::Error(CoreError::Audio(
                "the mic test is not available during a call".into(),
            )))
            .await;
            return;
        }
        let settings = self.audio_settings().await.clone();
        let mode = Mode::MicTest { loopback };
        match AudioSession::start(&settings, mode, &self.config.audio, self.events.clone()).await {
            Ok((session, _)) => self.audio_session = Some(session),
            Err(e) => self.emit(CoreEvent::Error(e)).await,
        }
    }
}
