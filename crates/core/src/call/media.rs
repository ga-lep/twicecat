//! Call media over LiveKit: publish the processed microphone, play every
//! remote audio track through the voice engine's mixer, and track who
//! speaks and who is muted.
//!
//! In Element Call's compatibility mode a participant's LiveKit identity is
//! its membership ID, `user:device`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::Duration;

use futures_util::StreamExt as _;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::{
    LocalAudioTrack, LocalTrack, RemoteTrack, Room, RoomEvent, RoomOptions, TrackKind, TrackSource,
};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tessera_audio::{FRAME_LEN, Outgoing, SAMPLE_RATE, SILENCE, SourceId};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use super::UserVolumes;
use super::jwt::SfuAccess;
use crate::api::{CoreError, UserId};
use crate::audio_session::{self, Voice};

/// How long `Room::connect` may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Buffer inside the LiveKit audio source, ms (multiple of 10).
const SOURCE_QUEUE_MS: u32 = 50;

/// Who speaks and who is muted, by LiveKit identity (= member ID).
#[derive(Debug, Default)]
pub(crate) struct MediaFlags {
    pub speaking: HashSet<String>,
    pub muted: HashSet<String>,
}

/// Flags shared with the participant reporting, and a nudge to re-report.
#[derive(Clone, Default)]
pub(crate) struct SharedFlags {
    pub flags: Arc<RwLock<MediaFlags>>,
    pub changed: Arc<Notify>,
}

/// What the engine must react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaNotice {
    Reconnecting,
    Reconnected,
    /// The SFU connection is gone for good.
    Disconnected,
}

/// The user of a `user:device` identity.
fn user_of(identity: &str) -> &str {
    identity.rsplit_once(':').map_or(identity, |(user, _)| user)
}

fn to_i16(frame: &tessera_audio::Frame) -> Vec<i16> {
    frame
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
        .collect()
}

/// Remote sources by identity: the mixer source and its reader task.
type Sources = Arc<StdMutex<HashMap<String, (SourceId, JoinHandle<()>)>>>;

/// A connected SFU session.
pub(crate) struct Media {
    room: Room,
    track: LocalAudioTrack,
    voice: Option<Voice>,
    sources: Sources,
    tasks: Vec<JoinHandle<()>>,
}

impl Media {
    /// Connects to the SFU, publishes the microphone track (fed from
    /// `outgoing`), and plays remote tracks through `voice`.
    pub(crate) async fn connect(
        sfu: &SfuAccess,
        voice: Option<Voice>,
        outgoing: Option<Outgoing>,
        bitrate_kbps: u16,
        volumes: UserVolumes,
        flags: SharedFlags,
        notices: mpsc::Sender<MediaNotice>,
    ) -> Result<Self, CoreError> {
        let (room, room_events) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            Room::connect(&sfu.url, &sfu.jwt, RoomOptions::default()),
        )
        .await
        .map_err(|_| CoreError::Call("connecting to the call server timed out".into()))?
        .map_err(|_| CoreError::Call("could not connect to the call server".into()))?;

        let source = NativeAudioSource::new(
            // Our own pipeline already did echo cancellation, noise
            // suppression and gain.
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            SAMPLE_RATE,
            1,
            SOURCE_QUEUE_MS,
        );
        let track = LocalAudioTrack::create_audio_track(
            "microphone",
            RtcAudioSource::Native(source.clone()),
        );
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(track.clone()),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    audio_encoding: Some(AudioEncoding {
                        max_bitrate: u64::from(bitrate_kbps) * 1000,
                    }),
                    // Opus DTX (near-free silence) and RED (redundant
                    // packets against loss), both on by default in LiveKit.
                    dtx: true,
                    red: true,
                    ..TrackPublishOptions::default()
                },
            )
            .await
            .map_err(|_| CoreError::Call("could not publish the microphone".into()))?;

        let sources: Sources = Arc::default();
        let mut tasks = Vec::new();
        if let Some(outgoing) = outgoing {
            tasks.push(tokio::spawn(send_microphone(outgoing, source)));
        }
        tasks.push(tokio::spawn(handle_events(
            room_events,
            voice.clone(),
            sources.clone(),
            volumes,
            flags,
            notices,
        )));
        Ok(Self {
            room,
            track,
            voice,
            sources,
            tasks,
        })
    }

    /// Mutes or unmutes the published track (others see it as muted).
    pub(crate) fn set_muted(&self, muted: bool) {
        if muted {
            self.track.mute();
        } else {
            self.track.unmute();
        }
    }

    /// Applies a user's new playback volume to their devices' sources.
    pub(crate) fn set_user_volume(&self, user: &UserId, percent: u16) {
        let Some(voice) = &self.voice else {
            return;
        };
        let sources = self.sources.lock().expect("sources lock");
        for (identity, (id, _)) in sources.iter() {
            if user_of(identity) == user.as_str() {
                audio_session::set_source_volume(voice, *id, percent);
            }
        }
    }

    /// Disconnects and stops every task.
    pub(crate) async fn close(mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
        let readers: Vec<_> = self
            .sources
            .lock()
            .expect("sources lock")
            .drain()
            .map(|(_, (_, task))| task)
            .collect();
        for task in readers {
            task.abort();
        }
        let _ = self.room.close().await;
    }
}

/// Moves processed microphone frames into the LiveKit source as they come.
async fn send_microphone(mut outgoing: Outgoing, source: NativeAudioSource) {
    let mut frame = SILENCE;
    let mut ticker = tokio::time::interval(Duration::from_millis(5));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        while outgoing.pop_frame(&mut frame) {
            let audio = AudioFrame {
                data: to_i16(&frame).into(),
                sample_rate: SAMPLE_RATE,
                num_channels: 1,
                samples_per_channel: FRAME_LEN as u32,
            };
            if source.capture_frame(&audio).await.is_err() {
                tracing::warn!("microphone frame rejected by the call track");
            }
        }
    }
}

/// Reads one remote track into its mixer source.
async fn read_track(mut stream: NativeAudioStream, mut input: tessera_audio::SourceInput) {
    let mut samples = Vec::with_capacity(FRAME_LEN);
    while let Some(frame) = stream.next().await {
        let channels = frame.num_channels.max(1) as usize;
        samples.clear();
        samples.extend(
            frame
                .data
                .chunks_exact(channels)
                .map(|c| c.iter().map(|s| f32::from(*s)).sum::<f32>() / channels as f32 / 32768.0),
        );
        input.push(&samples);
    }
}

fn remove(sources: &Sources, voice: Option<&Voice>, identity: &str) {
    if let Some((id, task)) = sources.lock().expect("sources lock").remove(identity) {
        task.abort();
        if let Some(voice) = voice {
            audio_session::remove_source(voice, id);
        }
    }
}

async fn handle_events(
    mut events: mpsc::UnboundedReceiver<RoomEvent>,
    voice: Option<Voice>,
    sources: Sources,
    volumes: UserVolumes,
    flags: SharedFlags,
    notices: mpsc::Sender<MediaNotice>,
) {
    while let Some(event) = events.recv().await {
        match event {
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                participant,
                ..
            } => {
                let identity = participant.identity().as_str().to_owned();
                let Some(voice) = voice.as_ref() else {
                    continue;
                };
                let volume = volumes
                    .read()
                    .ok()
                    .and_then(|v| v.get(&UserId::from(user_of(&identity))).copied())
                    .unwrap_or(100);
                let Some((id, input)) = audio_session::add_source(voice, volume) else {
                    tracing::warn!("mixer queue full; a participant is not played");
                    continue;
                };
                let stream = NativeAudioStream::new(track.rtc_track(), SAMPLE_RATE as i32, 1);
                let reader = tokio::spawn(read_track(stream, input));
                if let Some((old, old_task)) = sources
                    .lock()
                    .expect("sources lock")
                    .insert(identity, (id, reader))
                {
                    old_task.abort();
                    audio_session::remove_source(voice, old);
                }
            }
            RoomEvent::TrackUnsubscribed {
                track: RemoteTrack::Audio(_),
                participant,
                ..
            } => remove(&sources, voice.as_ref(), participant.identity().as_str()),
            RoomEvent::ParticipantDisconnected(participant) => {
                let identity = participant.identity().as_str().to_owned();
                remove(&sources, voice.as_ref(), &identity);
                if let Ok(mut f) = flags.flags.write() {
                    f.speaking.remove(&identity);
                    f.muted.remove(&identity);
                }
                flags.changed.notify_one();
            }
            RoomEvent::ActiveSpeakersChanged { speakers } => {
                if let Ok(mut f) = flags.flags.write() {
                    f.speaking = speakers
                        .iter()
                        .map(|p| p.identity().as_str().to_owned())
                        .collect();
                }
                flags.changed.notify_one();
            }
            RoomEvent::TrackMuted {
                participant,
                publication,
            } if publication.kind() == TrackKind::Audio => {
                if let Ok(mut f) = flags.flags.write() {
                    f.muted.insert(participant.identity().as_str().to_owned());
                }
                flags.changed.notify_one();
            }
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } if publication.kind() == TrackKind::Audio => {
                if let Ok(mut f) = flags.flags.write() {
                    f.muted.remove(participant.identity().as_str());
                }
                flags.changed.notify_one();
            }
            RoomEvent::Reconnecting => {
                let _ = notices.send(MediaNotice::Reconnecting).await;
            }
            RoomEvent::Reconnected => {
                let _ = notices.send(MediaNotice::Reconnected).await;
            }
            RoomEvent::Disconnected { .. } => {
                let _ = notices.send(MediaNotice::Disconnected).await;
                return;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_maps_to_its_user() {
        assert_eq!(user_of("@alice:example.org:DEVICE"), "@alice:example.org");
        assert_eq!(
            user_of("@bob:example.org:8448:DEV"),
            "@bob:example.org:8448"
        );
        assert_eq!(user_of("opaque"), "opaque");
    }

    #[test]
    fn samples_convert_to_i16_and_clip() {
        let mut f = SILENCE;
        f[0] = 1.0;
        f[1] = -2.0;
        f[2] = 0.5;
        let s = to_i16(&f);
        assert_eq!(s.len(), FRAME_LEN);
        assert_eq!(s[0], i16::MAX);
        assert_eq!(s[1], -i16::MAX);
        assert_eq!(s[2], i16::MAX / 2);
    }
}
