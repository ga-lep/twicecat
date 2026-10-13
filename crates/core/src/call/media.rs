//! Call media over LiveKit, one connection per SFU (Element Call's
//! multi-SFU mode): ours publishes the processed microphone, on the others
//! we only subscribe. Each connection plays the remote audio tracks of the
//! members expected there through the voice engine's mixer, and tracks who
//! speaks and who is muted. Ours also announces our microphone's cleaning
//! ([`super::cleaning::ANNOUNCE_KEY`]), and each reads what the others say
//! about theirs.
//!
//! A LiveKit identity is the member's `user:device` (session memberships).
//! A participant may send two audio tracks, its voice and the sound of a
//! screen share (Element Call): each plays as its own source, and only the
//! voice is cleaned and counts as muted.
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
    LocalAudioTrack, LocalParticipant, LocalTrack, Participant, RemoteAudioTrack,
    RemoteParticipant, RemoteTrack, RemoteTrackPublication, Room, RoomEvent, RoomOptions,
    TrackKind, TrackSource,
};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use twicecat_audio::{FRAME_LEN, Outgoing, SAMPLE_RATE, SILENCE, SourceId};

use super::cleaning::{ANNOUNCE_KEY, Claim, Cleaning};
use super::jwt::SfuAccess;
use super::{UserVolumes, user_volume};
use crate::api::{CleaningStatus, CoreError, UserId};
use crate::audio_session::{self, Voice};

/// How long one attempt to open the SFU's signalling websocket may take.
/// LiveKit's own default is 5 s, too short on a slow or lossy link (TCP,
/// TLS and the upgrade over a 700 ms round trip with packet loss);
/// livekit-client, which Element Call uses, allows 15 s.
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(15);
/// How long `Room::connect` may take, LiveKit's own retries included.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
/// Buffer inside the LiveKit audio source, ms (multiple of 10).
const SOURCE_QUEUE_MS: u32 = 50;
/// How often what senders say about their voice is read again: a track's
/// flags change without an event.
const CLAIM_RECHECK: Duration = Duration::from_secs(2);
/// Wait before announcing again after the SFU refused or timed out.
const ANNOUNCE_RETRY: Duration = Duration::from_secs(5);

/// Who speaks, who is muted and whose voice is cleaned, by LiveKit
/// identity (`user:device`).
#[derive(Debug, Default)]
pub(crate) struct MediaFlags {
    /// Active speakers per SFU connection: each room reports only its own.
    pub speaking: HashMap<String, HashSet<String>>,
    pub muted: HashSet<String>,
    /// Set by [`super::cleaning::Cleaning`]; absent: not cleaned.
    pub cleaning: HashMap<String, CleaningStatus>,
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
    /// What we announce under [`ANNOUNCE_KEY`] (`None`: nothing).
    pub announce: watch::Receiver<Option<&'static str>>,
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
pub(crate) fn user_of(identity: &str) -> &str {
    identity.rsplit_once(':').map_or(identity, |(user, _)| user)
}

/// `frame` as 16-bit samples, clipped, into `out`.
fn to_i16(frame: &twicecat_audio::Frame, out: &mut [i16; FRAME_LEN]) {
    for (o, s) in out.iter_mut().zip(frame) {
        *o = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
    }
}

fn volume_of(volumes: &UserVolumes, identity: &str) -> u16 {
    user_volume(volumes, &UserId::from(user_of(identity)))
}

/// What a remote audio track carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Feed {
    /// The microphone, or a track that names no source.
    Voice,
    /// A screen share's sound: never cleaned, its mute is not the
    /// participant's.
    Share,
}

impl Feed {
    fn of(source: TrackSource) -> Self {
        match source {
            TrackSource::ScreenshareAudio => Self::Share,
            _ => Self::Voice,
        }
    }
}

/// A participant's track: its identity and what the track carries.
type TrackKey = (String, Feed);

/// The subscribed remote audio tracks of one connection and the ones
/// playing: exactly those of expected identities.
#[derive(Default)]
struct Players {
    tracks: HashMap<TrackKey, RemoteAudioTrack>,
    playing: HashMap<TrackKey, (SourceId, JoinHandle<()>)>,
    expected: BTreeSet<String>,
    /// Cleans the voices that play (shared by the call's connections).
    cleaning: Option<Cleaning>,
}

impl Players {
    fn stop(&mut self, key: &TrackKey, voice: Option<&Voice>) {
        if let Some((id, task)) = self.playing.remove(key) {
            task.abort();
            if let Some(voice) = voice {
                audio_session::voice_control(voice).remove_source(id);
            }
            if let (Some(cleaning), (identity, Feed::Voice)) = (&self.cleaning, key) {
                cleaning.remove(identity);
            }
        }
    }

    /// Forgets every track of `identity`.
    fn forget(&mut self, identity: &str) {
        self.tracks.retain(|(id, _), _| id != identity);
    }

    /// Starts and stops mixer sources so that exactly the subscribed tracks
    /// of expected identities play.
    fn reconcile(&mut self, voice: Option<&Voice>, volumes: &UserVolumes) {
        let stale: Vec<TrackKey> = self
            .playing
            .keys()
            .filter(|key| !self.tracks.contains_key(*key) || !self.expected.contains(&key.0))
            .cloned()
            .collect();
        for key in stale {
            self.stop(&key, voice);
        }
        let Some(voice) = voice else {
            return;
        };
        for (key, track) in &self.tracks {
            let (identity, feed) = key;
            if self.playing.contains_key(key) || !self.expected.contains(identity) {
                continue;
            }
            let Some((id, input)) =
                audio_session::voice_control(voice).add_source(volume_of(volumes, identity))
            else {
                tracing::warn!("mixer queue full; a participant is not played");
                continue;
            };
            let stream = NativeAudioStream::new(track.rtc_track(), SAMPLE_RATE as i32, 1);
            let flag = input.cleaning_flag();
            self.playing.insert(
                key.clone(),
                (id, spawn_audio_task(read_track(stream, input))),
            );
            if let (Some(cleaning), Feed::Voice) = (&self.cleaning, feed) {
                cleaning.add(identity, id, flag);
            }
        }
    }

    fn stop_all(&mut self, voice: Option<&Voice>) {
        let all: Vec<TrackKey> = self.playing.keys().cloned().collect();
        for key in all {
            self.stop(&key, voice);
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
#[derive(Clone)]
pub(crate) struct ConnectionShared {
    /// The call's frame encryption (one key provider for all SFUs), in an
    /// encrypted room.
    pub encryption: Option<E2eeOptions>,
    pub voice: Option<Voice>,
    pub volumes: UserVolumes,
    pub flags: SharedFlags,
    pub cleaning: Cleaning,
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
        options.connect_timeout = SIGNAL_TIMEOUT;
        let (room, room_events) =
            tokio::time::timeout(CONNECT_TIMEOUT, Room::connect(&sfu.url, &sfu.jwt, options))
                .await
                .map_err(|_| CoreError::Call("connecting to the call server timed out".into()))?
                .map_err(|_| CoreError::Call("could not connect to the call server".into()))?;

        let mut tasks = Vec::new();
        let mut track = None;
        let mut key_index = None;
        let mut reannounce = None;
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
            let reconnected = Arc::new(Notify::new());
            tasks.push(tokio::spawn(announce(
                room.local_participant(),
                publish.announce,
                reconnected.clone(),
            )));
            reannounce = Some(reconnected);
            track = Some(local);
        }

        let players = Arc::new(StdMutex::new(Players {
            expected: expected.borrow_and_update().clone(),
            cleaning: Some(shared.cleaning.clone()),
            ..Players::default()
        }));
        tasks.push(tokio::spawn(handle_events(
            EventContext {
                key: key.clone(),
                voice: shared.voice.clone(),
                players: players.clone(),
                volumes: shared.volumes.clone(),
                flags: shared.flags.clone(),
                cleaning: shared.cleaning.clone(),
                reannounce,
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
        for ((identity, _), (id, _)) in &players.playing {
            if user_of(identity) == user.as_str() {
                audio_session::voice_control(voice).set_source_volume(*id, percent);
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
                .map(|((identity, _), _)| identity)
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

/// Keeps our cleaning announcement on the SFU: set once we publish, again
/// when it changes and after a reconnection (a full one starts a new
/// session, without it), retried while the SFU refuses.
async fn announce(
    local: LocalParticipant,
    mut value: watch::Receiver<Option<&'static str>>,
    reconnected: Arc<Notify>,
) {
    // What the SFU holds, if known: nothing at first.
    let mut sent = Some(None);
    loop {
        let now = *value.borrow_and_update();
        if sent != Some(now) {
            // An empty value removes the attribute.
            let attributes =
                HashMap::from([(ANNOUNCE_KEY.to_owned(), now.unwrap_or_default().to_owned())]);
            sent = match local.set_attributes(attributes).await {
                Ok(()) => Some(now),
                Err(_) => {
                    tracing::debug!("the call server did not take our cleaning announcement");
                    None
                }
            };
        }
        tokio::select! {
            changed = value.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = reconnected.notified() => sent = None,
            () = tokio::time::sleep(ANNOUNCE_RETRY), if sent.is_none() => {}
        }
    }
}

/// Moves processed microphone frames into the LiveKit source as they come.
async fn send_microphone(mut outgoing: Outgoing, source: NativeAudioSource) {
    let mut frame = SILENCE;
    let mut samples = [0_i16; FRAME_LEN];
    let mut ticker = tokio::time::interval(Duration::from_millis(5));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        while outgoing.pop_frame(&mut frame) {
            to_i16(&frame, &mut samples);
            let audio = AudioFrame {
                data: samples[..].into(),
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
            .name("twicecat-call-audio".into())
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
async fn read_track(mut stream: NativeAudioStream, mut input: twicecat_audio::SourceInput) {
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
    cleaning: Cleaning,
    /// On the connection that publishes: announce again.
    reannounce: Option<Arc<Notify>>,
    notices: mpsc::Sender<(String, MediaNotice)>,
}

impl EventContext {
    /// Passes on what `participant` says about the voice on `publication`.
    fn read_claim(&self, participant: &RemoteParticipant, publication: &RemoteTrackPublication) {
        let attributes = participant.attributes();
        let claim = Claim::of(
            attributes.get(ANNOUNCE_KEY).map(String::as_str),
            &publication.audio_features(),
        );
        self.cleaning
            .set_claim(participant.identity().as_str(), claim);
    }

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
    // The voices we play, to read what their senders say about them.
    let mut heard: HashMap<String, (RemoteParticipant, RemoteTrackPublication)> = HashMap::new();
    let mut recheck = tokio::time::interval(CLAIM_RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let event = tokio::select! {
            _ = recheck.tick() => {
                for (participant, publication) in heard.values() {
                    cx.read_claim(participant, publication);
                }
                continue;
            }
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
                publication,
                participant,
            } => {
                let key = (
                    participant.identity().as_str().to_owned(),
                    Feed::of(publication.source()),
                );
                if key.1 == Feed::Voice {
                    cx.read_claim(&participant, &publication);
                    heard.insert(key.0.clone(), (participant, publication));
                }
                cx.with_players(|p| {
                    // A new track of the same identity and feed replaces the
                    // old one.
                    p.stop(&key, cx.voice.as_ref());
                    p.tracks.insert(key, track);
                });
            }
            RoomEvent::TrackUnsubscribed {
                track: RemoteTrack::Audio(_),
                publication,
                participant,
            } => {
                let key = (
                    participant.identity().as_str().to_owned(),
                    Feed::of(publication.source()),
                );
                if key.1 == Feed::Voice {
                    heard.remove(&key.0);
                    cx.cleaning.set_claim(&key.0, Claim::None);
                }
                cx.with_players(|p| {
                    p.tracks.remove(&key);
                });
            }
            RoomEvent::ParticipantAttributesChanged {
                participant: Participant::Remote(participant),
                ..
            } => {
                if let Some((_, publication)) = heard.get(participant.identity().as_str()) {
                    cx.read_claim(&participant, publication);
                }
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                let identity = participant.identity().as_str().to_owned();
                talkers.remove(&identity);
                heard.remove(&identity);
                cx.cleaning.set_claim(&identity, Claim::None);
                cx.with_players(|p| p.forget(&identity));
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
            } if publication.kind() == TrackKind::Audio
                && Feed::of(publication.source()) == Feed::Voice =>
            {
                let identity = participant.identity().as_str().to_owned();
                cx.flags(|f| {
                    f.muted.insert(identity);
                });
            }
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } if publication.kind() == TrackKind::Audio
                && Feed::of(publication.source()) == Feed::Voice =>
            {
                let identity = participant.identity().as_str().to_owned();
                cx.flags(|f| {
                    f.muted.remove(&identity);
                });
            }
            RoomEvent::Reconnecting => cx.notice(MediaNotice::Reconnecting).await,
            RoomEvent::Reconnected => {
                if let Some(reannounce) = &cx.reannounce {
                    reannounce.notify_one();
                }
                cx.notice(MediaNotice::Reconnected).await;
            }
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
        let mut s = [0; FRAME_LEN];
        to_i16(&f, &mut s);
        assert_eq!(s[0], i16::MAX);
        assert_eq!(s[1], -i16::MAX);
        assert_eq!(s[2], i16::MAX / 2);
    }

    #[test]
    fn only_screen_share_audio_is_a_share() {
        assert_eq!(Feed::of(TrackSource::ScreenshareAudio), Feed::Share);
        for source in [TrackSource::Microphone, TrackSource::Unknown] {
            assert_eq!(Feed::of(source), Feed::Voice);
        }
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
