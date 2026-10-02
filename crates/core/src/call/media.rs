//! Call media over LiveKit, one connection per SFU (Element Call's
//! multi-SFU mode): ours publishes the processed microphone, on the others
//! we only subscribe. Each connection plays the remote audio tracks of the
//! members expected there through the voice engine's mixer, and tracks who
//! speaks and who is muted.
//!
//! A LiveKit identity is the member's `user:device` (session memberships).
//! A participant that is not an active member on that SFU is not played:
//! the call services hand out tokens to any Matrix user (as Element Call,
//! which only shows participants matching a membership).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock};
use std::time::Duration;

use futures_util::StreamExt as _;
use livekit::e2ee::E2eeOptions;
use livekit::e2ee::manager::E2eeManager;
use livekit::id::ParticipantIdentity;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::{
    LocalAudioTrack, LocalTrack, Participant, RemoteAudioTrack, RemoteTrack, Room, RoomEvent,
    RoomOptions, TrackKind, TrackSource,
};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tessera_audio::{FRAME_LEN, Outgoing, SAMPLE_RATE, SILENCE, SourceId};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;

use super::UserVolumes;
use super::jwt::SfuAccess;
use crate::api::{CoreError, UserId};
use crate::audio_session::{self, Voice};

/// How long `Room::connect` may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Buffer inside the LiveKit audio source, ms (multiple of 10).
const SOURCE_QUEUE_MS: u32 = 50;

/// Who speaks and who is muted, by LiveKit identity (`user:device`).
#[derive(Debug, Default)]
pub(crate) struct MediaFlags {
    /// Active speakers per SFU connection: each room reports only its own.
    pub speaking: HashMap<String, HashSet<String>>,
    pub muted: HashSet<String>,
}

impl MediaFlags {
    pub(crate) fn is_speaking(&self, identity: &str) -> bool {
        self.speaking.values().any(|s| s.contains(identity))
    }
}

/// Flags shared with the participant reporting, and a nudge to re-report.
#[derive(Clone, Default)]
pub(crate) struct SharedFlags {
    pub flags: Arc<RwLock<MediaFlags>>,
    pub changed: Arc<Notify>,
}

/// What a connection reports, with its SFU (call service URL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaNotice {
    Reconnecting,
    Reconnected,
    /// The SFU connection is gone for good.
    Disconnected,
}

/// The identities a connection plays; the call updates it as memberships
/// change.
pub(crate) type Expected = watch::Receiver<BTreeSet<String>>;

/// The microphone side of the connection that publishes.
pub(crate) struct Publish {
    pub outgoing: Option<Outgoing>,
    pub bitrate_kbps: u16,
    /// In an encrypted call: the index of our current key. LiveKit's native
    /// encryptor uses the index it was given (0 by default), not the latest
    /// key set, so a rotation has to be applied to our track's cryptor.
    pub key_index: Option<watch::Receiver<i32>>,
}

/// Keeps our published tracks' encryptors on our current key index.
struct KeyIndex {
    e2ee: E2eeManager,
    local: ParticipantIdentity,
    index: watch::Receiver<i32>,
}

impl KeyIndex {
    fn apply(&self) {
        let index = *self.index.borrow();
        for ((identity, _), cryptor) in self.e2ee.frame_cryptors() {
            if identity == self.local {
                cryptor.set_key_index(index);
            }
        }
    }
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

fn volume_of(volumes: &UserVolumes, identity: &str) -> u16 {
    volumes
        .read()
        .ok()
        .and_then(|v| v.get(&UserId::from(user_of(identity))).copied())
        .unwrap_or(100)
}

/// The subscribed remote audio tracks of one connection and the ones
/// playing: exactly those of expected identities.
#[derive(Default)]
struct Players {
    tracks: HashMap<String, RemoteAudioTrack>,
    playing: HashMap<String, (SourceId, JoinHandle<()>)>,
    expected: BTreeSet<String>,
}

impl Players {
    fn stop(&mut self, identity: &str, voice: Option<&Voice>) {
        if let Some((id, task)) = self.playing.remove(identity) {
            task.abort();
            if let Some(voice) = voice {
                audio_session::remove_source(voice, id);
            }
        }
    }

    /// Starts and stops mixer sources so that exactly the subscribed tracks
    /// of expected identities play.
    fn reconcile(&mut self, voice: Option<&Voice>, volumes: &UserVolumes) {
        let stale: Vec<String> = self
            .playing
            .keys()
            .filter(|id| !self.tracks.contains_key(*id) || !self.expected.contains(*id))
            .cloned()
            .collect();
        for identity in stale {
            self.stop(&identity, voice);
        }
        let Some(voice) = voice else {
            return;
        };
        for (identity, track) in &self.tracks {
            if self.playing.contains_key(identity) || !self.expected.contains(identity) {
                continue;
            }
            let Some((id, input)) = audio_session::add_source(voice, volume_of(volumes, identity))
            else {
                tracing::warn!("mixer queue full; a participant is not played");
                continue;
            };
            let stream = NativeAudioStream::new(track.rtc_track(), SAMPLE_RATE as i32, 1);
            self.playing.insert(
                identity.clone(),
                (id, spawn_audio_task(read_track(stream, input))),
            );
        }
    }

    fn stop_all(&mut self, voice: Option<&Voice>) {
        let all: Vec<String> = self.playing.keys().cloned().collect();
        for identity in all {
            self.stop(&identity, voice);
        }
    }
}

/// One SFU connection.
pub(crate) struct SfuConnection {
    /// The call service URL this SFU belongs to.
    key: String,
    room: Room,
    /// Our microphone, on the connection that publishes.
    track: Option<LocalAudioTrack>,
    voice: Option<Voice>,
    players: Arc<StdMutex<Players>>,
    flags: SharedFlags,
    tasks: Vec<JoinHandle<()>>,
}

/// What a connection needs besides its SFU access.
pub(crate) struct ConnectionShared {
    /// The call's frame encryption (one key provider for all SFUs), in an
    /// encrypted room.
    pub encryption: Option<E2eeOptions>,
    pub voice: Option<Voice>,
    pub volumes: UserVolumes,
    pub flags: SharedFlags,
    pub notices: mpsc::Sender<(String, MediaNotice)>,
}

impl SfuConnection {
    /// Connects to the SFU of the call service `key`, plays the expected
    /// members' audio and, with `publish`, publishes the microphone (fed
    /// from its `outgoing`).
    pub(crate) async fn connect(
        key: String,
        sfu: &SfuAccess,
        publish: Option<Publish>,
        mut expected: Expected,
        shared: &ConnectionShared,
    ) -> Result<Self, CoreError> {
        let mut options = RoomOptions::default();
        options.encryption = shared.encryption.clone();
        let (room, room_events) =
            tokio::time::timeout(CONNECT_TIMEOUT, Room::connect(&sfu.url, &sfu.jwt, options))
                .await
                .map_err(|_| CoreError::Call("connecting to the call server timed out".into()))?
                .map_err(|_| CoreError::Call("could not connect to the call server".into()))?;

        let mut tasks = Vec::new();
        let mut track = None;
        let mut key_index = None;
        if let Some(publish) = publish {
            key_index = publish.key_index.map(|index| KeyIndex {
                e2ee: room.e2ee_manager().clone(),
                local: room.local_participant().identity(),
                index,
            });
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
            let local = LocalAudioTrack::create_audio_track(
                "microphone",
                RtcAudioSource::Native(source.clone()),
            );
            room.local_participant()
                .publish_track(
                    LocalTrack::Audio(local.clone()),
                    TrackPublishOptions {
                        source: TrackSource::Microphone,
                        audio_encoding: Some(AudioEncoding {
                            max_bitrate: u64::from(publish.bitrate_kbps) * 1000,
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
            if let Some(outgoing) = publish.outgoing {
                tasks.push(spawn_audio_task(send_microphone(outgoing, source)));
            }
            track = Some(local);
        }

        let players = Arc::new(StdMutex::new(Players {
            expected: expected.borrow_and_update().clone(),
            ..Players::default()
        }));
        tasks.push(tokio::spawn(handle_events(
            EventContext {
                key: key.clone(),
                voice: shared.voice.clone(),
                players: players.clone(),
                volumes: shared.volumes.clone(),
                flags: shared.flags.clone(),
                notices: shared.notices.clone(),
            },
            room_events,
            expected,
            key_index,
        )));
        Ok(Self {
            key,
            room,
            track,
            voice: shared.voice.clone(),
            players,
            flags: shared.flags.clone(),
            tasks,
        })
    }

    /// Mutes or unmutes the published track (others see it as muted).
    pub(crate) fn set_muted(&self, muted: bool) {
        if let Some(track) = &self.track {
            if muted {
                track.mute();
            } else {
                track.unmute();
            }
        }
    }

    /// Applies a user's new playback volume to their devices' sources.
    pub(crate) fn set_user_volume(&self, user: &UserId, percent: u16) {
        let Some(voice) = &self.voice else {
            return;
        };
        let players = self.players.lock().expect("players lock");
        for (identity, (id, _)) in &players.playing {
            if user_of(identity) == user.as_str() {
                audio_session::set_source_volume(voice, *id, percent);
            }
        }
    }

    /// Disconnects, stops every task and forgets this SFU's flags.
    pub(crate) async fn close(mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
        let identities: Vec<String> = {
            let mut players = self.players.lock().expect("players lock");
            players.stop_all(self.voice.as_ref());
            players
                .tracks
                .drain()
                .map(|(identity, _)| identity)
                .collect()
        };
        if let Ok(mut f) = self.flags.flags.write() {
            f.speaking.remove(&self.key);
            for identity in &identities {
                f.muted.remove(identity);
            }
        }
        self.flags.changed.notify_one();
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

/// Runs a task that moves call audio between LiveKit and our voice engine
/// on its own thread, not the core's runtime. LiveKit's queues are short:
/// decoded audio waits in 100 ms that drop the oldest frames when full, and
/// the microphone source holds 50 ms and sends silence when it runs dry. A
/// task waiting behind the core's other work (a large sync, decryption; in
/// tests a single-threaded runtime) for longer is heard as gaps: a few
/// frames, then silence, every 200 ms. One thread serves every call track;
/// it only moves samples.
fn spawn_audio_task(task: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    static AUDIO: OnceLock<Option<tokio::runtime::Handle>> = OnceLock::new();
    let handle = AUDIO.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .ok()?;
        let handle = runtime.handle().clone();
        std::thread::Builder::new()
            .name("tessera-call-audio".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .ok()?;
        Some(handle)
    });
    match handle {
        Some(handle) => handle.spawn(task),
        None => {
            tracing::warn!("no thread for call audio; it runs on the core's runtime");
            tokio::spawn(task)
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

struct EventContext {
    key: String,
    voice: Option<Voice>,
    players: Arc<StdMutex<Players>>,
    volumes: UserVolumes,
    flags: SharedFlags,
    notices: mpsc::Sender<(String, MediaNotice)>,
}

impl EventContext {
    fn with_players(&self, change: impl FnOnce(&mut Players)) {
        let mut players = self.players.lock().expect("players lock");
        change(&mut players);
        players.reconcile(self.voice.as_ref(), &self.volumes);
    }

    fn flags(&self, change: impl FnOnce(&mut MediaFlags)) {
        if let Ok(mut f) = self.flags.flags.write() {
            change(&mut f);
        }
        self.flags.changed.notify_one();
    }

    async fn notice(&self, notice: MediaNotice) {
        let _ = self.notices.send((self.key.clone(), notice)).await;
    }
}

async fn handle_events(
    cx: EventContext,
    mut events: mpsc::UnboundedReceiver<RoomEvent>,
    mut expected: Expected,
    mut key_index: Option<KeyIndex>,
) {
    let mut expected_open = true;
    let mut index_open = key_index.is_some();
    // Who speaks on this SFU, as last seen starting (see
    // `ActiveSpeakersChanged` below).
    let mut talkers: HashMap<String, Participant> = HashMap::new();
    loop {
        let event = tokio::select! {
            changed = async {
                match key_index.as_mut() {
                    Some(k) => k.index.changed().await,
                    None => std::future::pending().await,
                }
            }, if index_open => {
                match changed {
                    Ok(()) => {
                        if let Some(k) = &key_index {
                            k.apply();
                        }
                    }
                    Err(_) => index_open = false,
                }
                continue;
            }
            event = events.recv() => match event {
                Some(event) => event,
                None => return,
            },
            changed = expected.changed(), if expected_open => {
                match changed {
                    Ok(()) => {
                        let now = expected.borrow_and_update().clone();
                        cx.with_players(|p| p.expected = now);
                    }
                    Err(_) => expected_open = false,
                }
                continue;
            }
        };
        match event {
            // Also after a full reconnect, which republishes with a fresh
            // encryptor.
            RoomEvent::LocalTrackPublished { .. } => {
                if let Some(k) = &key_index {
                    k.apply();
                }
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                participant,
                ..
            } => {
                let identity = participant.identity().as_str().to_owned();
                cx.with_players(|p| {
                    // A new track of the same identity replaces the old one.
                    p.stop(&identity, cx.voice.as_ref());
                    p.tracks.insert(identity, track);
                });
            }
            RoomEvent::TrackUnsubscribed {
                track: RemoteTrack::Audio(_),
                participant,
                ..
            } => {
                let identity = participant.identity().as_str().to_owned();
                cx.with_players(|p| {
                    p.tracks.remove(&identity);
                });
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                let identity = participant.identity().as_str().to_owned();
                talkers.remove(&identity);
                cx.with_players(|p| {
                    p.tracks.remove(&identity);
                });
                cx.flags(|f| {
                    if let Some(s) = f.speaking.get_mut(&cx.key) {
                        s.remove(&identity);
                    }
                    f.muted.remove(&identity);
                });
            }
            // LiveKit lists only the participants whose speaking changed and
            // started; each participant's own flag is current. Taking the
            // list as the whole set cleared everyone still talking whenever
            // someone else began.
            RoomEvent::ActiveSpeakersChanged { speakers } => {
                for p in speakers {
                    talkers.insert(p.identity().as_str().to_owned(), p);
                }
                talkers.retain(|_, p| p.is_speaking());
                let now = talkers.keys().cloned().collect();
                cx.flags(|f| {
                    f.speaking.insert(cx.key.clone(), now);
                });
            }
            RoomEvent::TrackMuted {
                participant,
                publication,
            } if publication.kind() == TrackKind::Audio => {
                let identity = participant.identity().as_str().to_owned();
                cx.flags(|f| {
                    f.muted.insert(identity);
                });
            }
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } if publication.kind() == TrackKind::Audio => {
                let identity = participant.identity().as_str().to_owned();
                cx.flags(|f| {
                    f.muted.remove(&identity);
                });
            }
            RoomEvent::Reconnecting => cx.notice(MediaNotice::Reconnecting).await,
            RoomEvent::Reconnected => cx.notice(MediaNotice::Reconnected).await,
            RoomEvent::Disconnected { .. } => {
                cx.notice(MediaNotice::Disconnected).await;
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

    #[test]
    fn speaking_counts_on_any_sfu() {
        let mut f = MediaFlags::default();
        f.speaking
            .insert("https://a".into(), HashSet::from(["@a:x:D".to_owned()]));
        f.speaking.insert("https://b".into(), HashSet::new());
        assert!(f.is_speaking("@a:x:D"));
        assert!(!f.is_speaking("@b:y:D"));
    }
}
