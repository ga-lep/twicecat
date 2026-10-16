//! Video in calls (plan Phase 9 Task 5): the call's video tracks, ours and
//! everyone else's on every SFU connection, reported as `VideoTracks` with
//! a [`VideoFeed`] per track, and their statistics about once a second.
//!
//! We send raw frames into LiveKit's video source ([`FramePusher`]);
//! LiveKit encodes them with the encoder [`super::encoder`] picks (NVENC or
//! VideoToolbox H.264, else libwebrtc's VP8) and makes the simulcast layers.
//! The frames come from the test pattern
//! ([`VideoBackend::TestPattern`](crate::api::VideoBackend)) or from a
//! screen capture ([`super::capture`], Task 8). A hardware encoder that
//! stops, or that LiveKit replaced, is reported to the engine
//! ([`MediaNotice::EncoderLost`]), which sends the track again on the
//! software path.
//!
//! A remote track is shown only while its sender is an active member of the
//! call, as for audio (`media`): the call services hand out tokens to any
//! Matrix user. Received frames are copied out of libwebrtc as I420 on
//! their own thread, then wait in the feed for the front end, which takes
//! the newest when it draws.
//!
//! Frames are never logged.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use livekit::prelude::{
    LocalVideoTrack, RemoteTrackPublication, RemoteVideoTrack, TrackDimension, TrackSource,
};
use livekit::webrtc::stats::{QualityLimitationReason, RtcStats};
use livekit::webrtc::video_frame::{
    BoxVideoFrame, NV12Buffer, VideoBuffer, VideoFrame as RtcFrame, VideoRotation,
};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use twicecat_video::test_pattern::TestPattern;
use twicecat_video::{CpuFrame, PixelFormat};

use super::encoder;
use super::media::MediaNotice;
use super::membership::Membership;
use crate::api::{
    CoreEvent, DeviceId, EncoderBackend, ParticipantId, QualityLimitation, RoomId, UserId,
    VideoCodec, VideoDirection, VideoFeed, VideoFeedSender, VideoFrame, VideoKind, VideoMatrix,
    VideoPixelFormat, VideoPlane, VideoRange, VideoStats, VideoTrackId, VideoTrackStats,
    VideoTrackView,
};

/// How often statistics are read and sent.
const STATS_EVERY: Duration = Duration::from_secs(1);
/// Reports in a row in which the source delivered frames and a hardware
/// encoder encoded none, before it counts as lost.
const STALLED_REPORTS: u8 = 3;
/// Microseconds on the core's video clock: monotonic, from the first time
/// anything in this process asks for it. Frame timestamps and the test
/// pattern's send times use it.
pub(crate) fn clock_us() -> i64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    i64::try_from(epoch.elapsed().as_micros()).unwrap_or(i64::MAX)
}

/// The LiveKit source of a kind of track.
pub(crate) fn source_of(kind: VideoKind) -> TrackSource {
    match kind {
        VideoKind::Camera => TrackSource::Camera,
        VideoKind::Screen => TrackSource::Screenshare,
    }
}

/// A video track's kind from its LiveKit source; `None` for anything that
/// is neither a camera nor a share.
fn kind_of(source: TrackSource) -> Option<VideoKind> {
    match source {
        TrackSource::Camera => Some(VideoKind::Camera),
        TrackSource::Screenshare => Some(VideoKind::Screen),
        _ => None,
    }
}

/// The codec of a MIME type such as `video/VP8`.
fn codec_of(mime: &str) -> Option<VideoCodec> {
    let name = mime.rsplit('/').next()?.to_ascii_lowercase();
    match name.as_str() {
        "h264" => Some(VideoCodec::H264),
        "h265" => Some(VideoCodec::H265),
        "vp8" => Some(VideoCodec::Vp8),
        "vp9" => Some(VideoCodec::Vp9),
        "av1" => Some(VideoCodec::Av1),
        _ => None,
    }
}

/// Runs a task that moves video frames on its own thread, not the core's
/// runtime (see `media::spawn_audio_task` for why). One thread serves every
/// received track; it copies frames out of libwebrtc.
fn spawn_video_task(task: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    static VIDEO: OnceLock<Option<tokio::runtime::Handle>> = OnceLock::new();
    let handle = VIDEO.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .ok()?;
        let handle = runtime.handle().clone();
        std::thread::Builder::new()
            .name("twicecat-call-video".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .ok()?;
        Some(handle)
    });
    match handle {
        Some(handle) => handle.spawn(task),
        None => {
            tracing::warn!("no thread for call video; it runs on the core's runtime");
            tokio::spawn(task)
        }
    }
}

/// The member a LiveKit identity belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Member {
    participant: ParticipantId,
    user_id: UserId,
    device_id: DeviceId,
}

/// Where a track's numbers come from.
#[derive(Clone)]
enum Rtc {
    Local(LocalVideoTrack),
    Remote(RemoteVideoTrack),
}

/// Counters at the last statistics, to turn totals into rates.
#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    at: Option<Instant>,
    bytes: u64,
    frames: u64,
    /// Seconds spent encoding or decoding those frames.
    busy_s: f64,
}

struct Track {
    /// The SFU connection (call service URL) it came on.
    sfu: String,
    identity: String,
    local: bool,
    kind: VideoKind,
    width: u32,
    height: u32,
    fps: Option<u32>,
    codec: Option<VideoCodec>,
    muted: bool,
    /// Handed out once, with the first report that shows the track.
    feed: Option<VideoFeed>,
    rtc: Rtc,
    /// To pick the layer of a received track.
    publication: Option<RemoteTrackPublication>,
    /// Copies received frames into the feed.
    reader: Option<JoinHandle<()>>,
    totals: Totals,
    /// Ours: the hardware encoder we asked LiveKit for, watched.
    encoder: Option<EncoderWatch>,
}

impl Drop for Track {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
    }
}

#[derive(Default)]
struct State {
    /// Active members by LiveKit identity.
    members: HashMap<String, Member>,
    tracks: BTreeMap<VideoTrackId, Track>,
}

impl State {
    /// The tracks to show, with their members.
    fn shown(&self) -> impl Iterator<Item = (&VideoTrackId, &Track, &Member)> {
        self.tracks
            .iter()
            .filter_map(|(id, t)| self.members.get(&t.identity).map(|m| (id, t, m)))
    }
}

/// The call's video tracks, shared by its SFU connections and reported by
/// [`Videos::report`].
#[derive(Clone)]
pub(crate) struct Videos {
    state: Arc<StdMutex<State>>,
    changed: Arc<Notify>,
    own: (String, Member),
    /// Where a lost encoder is reported.
    notices: Option<mpsc::Sender<MediaNotice>>,
}

impl Videos {
    /// The registry of a call we take part in as `user_id` on `device_id`;
    /// a hardware encoder that is lost is reported to `notices`.
    pub(crate) fn new(
        user_id: &str,
        device_id: &str,
        notices: Option<mpsc::Sender<MediaNotice>>,
    ) -> Self {
        let identity = super::membership::member_id(user_id, device_id);
        let own = Member {
            participant: ParticipantId::from(identity.as_str()),
            user_id: UserId::from(user_id),
            device_id: DeviceId::from(device_id),
        };
        let mut state = State::default();
        // Our own tracks show before our membership reaches the store.
        state.members.insert(identity.clone(), own.clone());
        Self {
            state: Arc::new(StdMutex::new(state)),
            changed: Arc::new(Notify::new()),
            own: (identity, own),
            notices,
        }
    }

    fn change(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().expect("videos lock"));
        self.changed.notify_one();
    }

    /// The call's active members (and us).
    pub(crate) fn set_members(&self, active: &[Membership]) {
        let mut members: HashMap<String, Member> = active
            .iter()
            .map(|m| {
                (
                    m.backend_identity.clone(),
                    Member {
                        participant: ParticipantId::from(m.member_id.as_str()),
                        user_id: UserId::from(m.user_id.as_str()),
                        device_id: DeviceId::from(m.device_id.as_str()),
                    },
                )
            })
            .collect();
        members.insert(self.own.0.clone(), self.own.1.clone());
        let mut state = self.state.lock().expect("videos lock");
        if state.members != members {
            state.members = members;
            drop(state);
            self.changed.notify_one();
        }
    }

    /// A remote track we are now subscribed to: its frames go to a feed.
    /// Tracks that are neither a camera nor a screen share are ignored.
    pub(crate) fn add_remote(
        &self,
        sfu: &str,
        identity: &str,
        track: RemoteVideoTrack,
        publication: RemoteTrackPublication,
    ) {
        let Some(kind) = kind_of(publication.source()) else {
            return;
        };
        let id = VideoTrackId::from(publication.sid().as_str());
        let (tx, feed) = VideoFeed::channel();
        let stream = NativeVideoStream::new(track.rtc_track());
        let reader = spawn_video_task(read_video(stream, tx, self.clone(), id.clone()));
        let entry = Track {
            sfu: sfu.to_owned(),
            identity: identity.to_owned(),
            local: false,
            kind,
            width: 0,
            height: 0,
            fps: None,
            codec: codec_of(&publication.mime_type()),
            muted: publication.is_muted(),
            feed: Some(feed),
            rtc: Rtc::Remote(track),
            publication: Some(publication),
            reader: Some(reader),
            totals: Totals::default(),
            encoder: None,
        };
        // A track of the same ID (resubscribed) replaces the old one.
        self.change(|s| {
            s.tracks.insert(id, entry);
        });
    }

    /// One of our own tracks, published on `sfu`.
    pub(crate) fn add_local(&self, sfu: &str, id: VideoTrackId, local: LocalSend) {
        let entry = Track {
            sfu: sfu.to_owned(),
            identity: self.own.0.clone(),
            local: true,
            kind: local.kind,
            width: local.width,
            height: local.height,
            fps: Some(local.fps),
            codec: Some(local.codec),
            muted: false,
            feed: Some(local.feed),
            rtc: Rtc::Local(local.track),
            publication: None,
            reader: None,
            totals: Totals::default(),
            encoder: (local.backend != EncoderBackend::Software)
                .then(|| EncoderWatch::new(local.backend)),
        };
        self.change(|s| {
            s.tracks.insert(id, entry);
        });
    }

    /// Forgets a track (unsubscribed or unpublished): its feed ends.
    pub(crate) fn remove(&self, id: &VideoTrackId) {
        self.change(|s| {
            s.tracks.remove(id);
        });
    }

    /// Forgets every track `identity` sent on `sfu` (it left).
    pub(crate) fn remove_identity(&self, sfu: &str, identity: &str) {
        self.change(|s| {
            s.tracks
                .retain(|_, t| t.sfu != sfu || t.identity != identity)
        });
    }

    /// Forgets every track of an SFU connection (closed).
    pub(crate) fn remove_sfu(&self, sfu: &str) {
        self.change(|s| s.tracks.retain(|_, t| t.sfu != sfu));
    }

    /// Its sender turned a track off or on.
    pub(crate) fn set_muted(&self, id: &VideoTrackId, muted: bool) {
        self.change(|s| {
            if let Some(t) = s.tracks.get_mut(id) {
                t.muted = muted;
            }
        });
    }

    /// A received track's picture size changed (a different simulcast
    /// layer, or the sender's own change).
    fn set_size(&self, id: &VideoTrackId, width: u32, height: u32) {
        self.change(|s| {
            if let Some(t) = s.tracks.get_mut(id) {
                t.width = width;
                t.height = height;
            }
        });
    }

    /// How the front end shows a received track: its size on screen in
    /// physical pixels (the SFU picks the smallest layer that covers it),
    /// or hidden (the SFU stops sending it). `false` when there is no such
    /// received track.
    pub(crate) fn set_view(
        &self,
        id: &VideoTrackId,
        width: u32,
        height: u32,
        visible: bool,
    ) -> bool {
        let state = self.state.lock().expect("videos lock");
        let Some(publication) = state.tracks.get(id).and_then(|t| t.publication.clone()) else {
            return false;
        };
        drop(state);
        // Both send an update to the SFU (from a task: a runtime is needed).
        publication.set_enabled(visible);
        if visible {
            publication.update_video_dimensions(TrackDimension(width, height));
        }
        true
    }

    /// The tracks to show, and the feeds of those shown for the first time.
    fn views(&self) -> (Vec<VideoTrackView>, Vec<(VideoTrackId, VideoFeed)>) {
        let mut state = self.state.lock().expect("videos lock");
        let views = state
            .shown()
            .map(|(id, t, m)| VideoTrackView {
                id: id.clone(),
                participant: m.participant.clone(),
                user_id: m.user_id.clone(),
                device_id: m.device_id.clone(),
                is_local: t.local,
                kind: t.kind,
                width: t.width,
                height: t.height,
                fps: t.fps,
                codec: t.codec,
                muted: t.muted,
            })
            .collect::<Vec<_>>();
        let shown: Vec<VideoTrackId> = views.iter().map(|v| v.id.clone()).collect();
        let feeds = shown
            .into_iter()
            .filter_map(|id| {
                let feed = state.tracks.get_mut(&id)?.feed.take()?;
                Some((id, feed))
            })
            .collect();
        (views, feeds)
    }

    /// Reports the tracks whenever they change (`VideoTracks`, then a
    /// `VideoFeed` for each new one) and their statistics every second
    /// while there are any. Runs for the length of the call.
    pub(crate) async fn report(self, room_id: RoomId, events: mpsc::Sender<CoreEvent>) {
        let mut last: Option<Vec<VideoTrackView>> = None;
        let mut ticker = tokio::time::interval(STATS_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.changed.notified() => {}
                _ = ticker.tick() => {
                    if let Some(stats) = self.stats().await
                        && events.send(CoreEvent::VideoStats(stats)).await.is_err()
                    {
                        return;
                    }
                    continue;
                }
            }
            let (views, feeds) = self.views();
            // Nothing to say before the first track.
            if last.as_ref() != Some(&views) && !(last.is_none() && views.is_empty()) {
                let event = CoreEvent::VideoTracks {
                    room_id: room_id.clone(),
                    tracks: views.clone(),
                };
                if events.send(event).await.is_err() {
                    return;
                }
                last = Some(views);
            }
            for (track, feed) in feeds {
                if events
                    .send(CoreEvent::VideoFeed { track, feed })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }

    /// Whether `report` has anything out that leaving must clear.
    pub(crate) fn any_shown(&self) -> bool {
        self.state
            .lock()
            .expect("videos lock")
            .shown()
            .next()
            .is_some()
    }

    /// The numbers of every shown track since the last call; `None` when
    /// there is no track.
    async fn stats(&self) -> Option<VideoStats> {
        let tracks: Vec<(VideoTrackId, Rtc)> = {
            let state = self.state.lock().expect("videos lock");
            state
                .shown()
                .map(|(id, t, _)| (id.clone(), t.rtc.clone()))
                .collect()
        };
        if tracks.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(tracks.len());
        let mut lost = Vec::new();
        for (id, rtc) in tracks {
            let (direction, report) = match &rtc {
                Rtc::Local(t) => (VideoDirection::Sent, t.get_stats().await),
                Rtc::Remote(t) => (VideoDirection::Received, t.get_stats().await),
            };
            let Ok(report) = report else {
                continue;
            };
            let now = Instant::now();
            let reading = match direction {
                VideoDirection::Sent => Reading::sent(&report),
                VideoDirection::Received => Reading::received(&report),
            };
            let (implementation, hardware) = implementation(&report, direction);
            let mut state = self.state.lock().expect("videos lock");
            let Some(track) = state.tracks.get_mut(&id) else {
                continue;
            };
            if let Some(watch) = &mut track.encoder
                && watch.update(&reading, implementation.as_deref())
            {
                lost.push(track.kind);
            }
            let mut stats = reading.stats(id, direction, &track.totals, now);
            stats.implementation = implementation;
            stats.hardware = hardware;
            out.push(stats);
            track.totals = reading.totals(now);
        }
        if let Some(notices) = &self.notices {
            for kind in lost {
                tracing::warn!(
                    ?kind,
                    "the hardware video encoder stopped; sending in software"
                );
                let _ = notices.send(MediaNotice::EncoderLost(kind)).await;
            }
        }
        Some(VideoStats { tracks: out })
    }
}

/// What one track's report says, summed over its RTP streams (a sent
/// track has one per simulcast layer).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Reading {
    width: u32,
    height: u32,
    bytes: u64,
    frames: u64,
    busy_s: f64,
    limitation: QualityLimitation,
    freezes: u32,
    /// Sent: some layer is active (dynacast pauses the ones nobody takes).
    active: bool,
    /// Sent: frames encoded over every layer.
    encoded: u64,
    /// Sent: frames the source delivered.
    source_frames: u64,
}

impl Reading {
    fn sent(report: &[RtcStats]) -> Self {
        let mut r = Self::default();
        for stats in report {
            if let RtcStats::MediaSource(m) = stats
                && m.source.kind == "video"
            {
                r.source_frames = u64::from(m.video.frames);
            }
            let RtcStats::OutboundRtp(s) = stats else {
                continue;
            };
            if s.stream.kind != "video" {
                continue;
            }
            r.bytes += s.sent.bytes_sent;
            r.encoded += u64::from(s.outbound.frames_encoded);
            r.active |= s.outbound.active;
            // The largest layer that is sent stands for the track.
            if s.outbound.active && s.outbound.frame_height >= r.height {
                r.width = s.outbound.frame_width;
                r.height = s.outbound.frame_height;
                r.frames = u64::from(s.outbound.frames_encoded);
                r.busy_s = s.outbound.total_encode_time;
                r.limitation = match s.outbound.quality_limitation_reason {
                    QualityLimitationReason::None => QualityLimitation::None,
                    QualityLimitationReason::Cpu => QualityLimitation::Cpu,
                    QualityLimitationReason::Bandwidth => QualityLimitation::Bandwidth,
                    QualityLimitationReason::Other => QualityLimitation::Other,
                };
            }
        }
        r
    }

    fn received(report: &[RtcStats]) -> Self {
        let mut r = Self::default();
        for stats in report {
            let RtcStats::InboundRtp(s) = stats else {
                continue;
            };
            if s.stream.kind != "video" {
                continue;
            }
            r.width = s.inbound.frame_width;
            r.height = s.inbound.frame_height;
            r.bytes += s.inbound.bytes_received;
            r.frames += u64::from(s.inbound.frames_decoded);
            r.busy_s += s.inbound.total_decode_time;
            r.freezes += s.inbound.freeze_count;
        }
        r
    }

    fn totals(self, now: Instant) -> Totals {
        Totals {
            at: Some(now),
            bytes: self.bytes,
            frames: self.frames,
            busy_s: self.busy_s,
        }
    }

    /// The rates since `before` (nothing the first time; a counter that
    /// went back, as when the top layer changes, counts from zero).
    fn stats(
        self,
        track: VideoTrackId,
        direction: VideoDirection,
        before: &Totals,
        now: Instant,
    ) -> VideoTrackStats {
        let since = |old: u64, new: u64| if new >= old { new - old } else { new };
        let (fps, bitrate_kbps, frame_ms) = match before.at {
            Some(at) if now > at => {
                let secs = (now - at).as_secs_f64();
                let frames = since(before.frames, self.frames);
                let bytes = since(before.bytes, self.bytes);
                let busy = if self.busy_s >= before.busy_s {
                    self.busy_s - before.busy_s
                } else {
                    self.busy_s
                };
                (
                    (frames as f64 / secs) as f32,
                    u32::try_from((bytes as f64 * 8.0 / 1000.0 / secs) as u64).unwrap_or(u32::MAX),
                    (frames > 0).then(|| (busy * 1000.0 / frames as f64) as f32),
                )
            }
            _ => (0.0, 0, None),
        };
        VideoTrackStats {
            track,
            direction,
            width: self.width,
            height: self.height,
            fps,
            bitrate_kbps,
            frame_ms,
            limitation: match direction {
                VideoDirection::Sent => self.limitation,
                VideoDirection::Received => QualityLimitation::None,
            },
            freezes: self.freezes,
            implementation: None,
            hardware: false,
        }
    }
}

/// The encoder (sent) or decoder (received) a report names, and whether it
/// runs in hardware: the first video stream that names one.
fn implementation(report: &[RtcStats], direction: VideoDirection) -> (Option<String>, bool) {
    for stats in report {
        let found = match (stats, direction) {
            (RtcStats::OutboundRtp(s), VideoDirection::Sent) if s.stream.kind == "video" => (
                s.outbound.encoder_implementation.as_str(),
                s.outbound.power_efficient_encoder,
            ),
            (RtcStats::InboundRtp(s), VideoDirection::Received) if s.stream.kind == "video" => (
                s.inbound.decoder_implementation.as_str(),
                s.inbound.power_efficient_decoder,
            ),
            _ => continue,
        };
        if !found.0.is_empty() {
            return (Some(found.0.to_owned()), found.1);
        }
    }
    (None, false)
}

/// Watches one of our tracks sent on a hardware encoder: it is lost when
/// LiveKit names another encoder (it fell back), or when the source keeps
/// delivering frames and no active layer encodes any for
/// [`STALLED_REPORTS`] reports (the encoder failed to start or stopped).
#[derive(Clone, Copy, Debug)]
struct EncoderWatch {
    backend: EncoderBackend,
    last: Option<(u64, u64)>,
    stalls: u8,
    lost: bool,
}

impl EncoderWatch {
    fn new(backend: EncoderBackend) -> Self {
        Self {
            backend,
            last: None,
            stalls: 0,
            lost: false,
        }
    }

    /// Takes a report; true the one time the encoder counts as lost.
    fn update(&mut self, reading: &Reading, implementation: Option<&str>) -> bool {
        if self.lost {
            return false;
        }
        let replaced = implementation.is_some_and(|i| !encoder::is_running(self.backend, i));
        let stalled = match self.last {
            Some((source, encoded)) => {
                reading.active && reading.source_frames > source && reading.encoded <= encoded
            }
            None => false,
        };
        self.last = Some((reading.source_frames, reading.encoded));
        self.stalls = if stalled { self.stalls + 1 } else { 0 };
        self.lost = replaced || self.stalls >= STALLED_REPORTS;
        self.lost
    }
}

/// Copies one received track's frames into its feed, and tells the
/// registry when the picture's size changes.
async fn read_video(
    mut stream: NativeVideoStream,
    feed: VideoFeedSender,
    videos: Videos,
    id: VideoTrackId,
) {
    let mut size = (0, 0);
    while let Some(frame) = stream.next().await {
        let frame = to_feed_frame(&frame);
        if (frame.width, frame.height) != size {
            size = (frame.width, frame.height);
            videos.set_size(&id, size.0, size.1);
        }
        feed.send(frame);
    }
}

/// A decoded frame as I420 planes the front end can keep. libwebrtc does
/// not tell its colour space; frames that do not say are BT.601 limited
/// range in WebRTC (what browsers assume too).
fn to_feed_frame(frame: &BoxVideoFrame) -> VideoFrame {
    let buffer = frame.buffer.as_ref();
    let converted;
    let i420 = match buffer.as_i420() {
        Some(i420) => i420,
        None => {
            converted = buffer.to_i420();
            &converted
        }
    };
    let (sy, su, sv) = i420.strides();
    let (y, u, v) = i420.data();
    VideoFrame {
        format: VideoPixelFormat::I420,
        width: i420.width(),
        height: i420.height(),
        planes: vec![
            VideoPlane {
                data: y.into(),
                stride: sy as usize,
            },
            VideoPlane {
                data: u.into(),
                stride: su as usize,
            },
            VideoPlane {
                data: v.into(),
                stride: sv as usize,
            },
        ],
        matrix: VideoMatrix::Bt601,
        range: VideoRange::Limited,
        timestamp_us: clock_us(),
    }
}

/// One of our tracks being sent, for the registry.
pub(crate) struct LocalSend {
    pub kind: VideoKind,
    pub track: LocalVideoTrack,
    pub codec: VideoCodec,
    /// Watched when it is a hardware encoder.
    pub backend: EncoderBackend,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// What we send, for a self-view.
    pub feed: VideoFeed,
}

/// Puts our frames into a LiveKit source and the self-view: NV12 frames
/// of any size, scaled down (by libwebrtc's libyuv) to fit the size the
/// track was published at when they are larger. Called on the thread that
/// makes the frames.
pub(crate) struct FramePusher {
    source: NativeVideoSource,
    feed: VideoFeedSender,
    /// The largest size sent.
    size: (u32, u32),
}

impl FramePusher {
    pub(crate) fn new(source: NativeVideoSource, feed: VideoFeedSender, size: (u32, u32)) -> Self {
        Self { source, feed, size }
    }

    /// Sends one frame now. A frame that is not NV12 is converted first; one
    /// that cannot be is dropped.
    pub(crate) fn push(&mut self, frame: &CpuFrame) {
        let converted;
        let frame = if frame.format() == PixelFormat::Nv12 {
            frame
        } else {
            match twicecat_video::convert::to_nv12(frame, frame.color()) {
                Ok(nv12) => {
                    converted = nv12;
                    &converted
                }
                Err(_) => return,
            }
        };
        let now_us = clock_us();
        let size = (frame.width(), frame.height());
        let target = twicecat_video::capture::fit_size(size, self.size);
        let mut buffer = to_nv12_buffer(frame);
        let api = if target == size {
            to_api_frame(frame, now_us)
        } else {
            buffer = buffer.scale(target.0 as i32, target.1 as i32);
            buffer_to_api_frame(&buffer, now_us)
        };
        self.source.capture_frame(&RtcFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: now_us,
            frame_metadata: None,
            buffer,
        });
        self.feed.send(api);
    }
}

/// Sends a test pattern into a LiveKit source from its own thread, until
/// stopped or dropped.
pub(crate) struct PatternSender {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PatternSender {
    /// Starts sending `width` × `height` at `fps` into `source`, and the same
    /// frames to `feed` (the self-view).
    pub(crate) fn start(
        source: NativeVideoSource,
        feed: VideoFeedSender,
        (width, height, fps): (u32, u32, u32),
    ) -> Result<Self, std::io::Error> {
        let stop = Arc::new(AtomicBool::new(false));
        let running = stop.clone();
        let pusher = FramePusher::new(source, feed, (width, height));
        let thread = std::thread::Builder::new()
            .name("twicecat-video-pattern".into())
            .spawn(move || send_pattern(pusher, (width, height, fps), &running))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the frames and waits until the last one is handed over (within
    /// a frame), off the async runtime's threads.
    pub(crate) async fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        }
    }
}

impl Drop for PatternSender {
    fn drop(&mut self) {
        // Without `stop`: the thread ends within a frame; nothing waits.
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn send_pattern(mut pusher: FramePusher, (width, height, fps): (u32, u32, u32), stop: &AtomicBool) {
    let pattern = TestPattern::new(width, height);
    let interval = Duration::from_secs(1) / fps.max(1);
    let mut next = Instant::now();
    let mut counter = 0_u32;
    while !stop.load(Ordering::Relaxed) {
        // The stamp's send time is the core's video clock, as the pusher's.
        let now_us = clock_us();
        let frame = pattern.frame(counter, Duration::from_micros(now_us.unsigned_abs()));
        pusher.push(&frame);
        counter = counter.wrapping_add(1);
        next += interval;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > interval {
            // Late by more than a frame (a busy machine): skip, do not burst.
            next = now;
        }
    }
}

/// An NV12 frame of ours in a libwebrtc buffer.
fn to_nv12_buffer(frame: &CpuFrame) -> NV12Buffer {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let mut buffer = NV12Buffer::new(frame.width(), frame.height());
    let (stride_y, stride_uv) = buffer.strides();
    let (dst_y, dst_uv) = buffer.data_mut();
    let planes = frame.planes();
    for (plane, dst, stride, rows) in [
        (&planes[0], dst_y, stride_y as usize, h),
        (&planes[1], dst_uv, stride_uv as usize, h / 2),
    ] {
        for row in 0..rows {
            dst[row * stride..row * stride + w]
                .copy_from_slice(&plane.data[row * plane.stride..row * plane.stride + w]);
        }
    }
    buffer
}

/// An NV12 frame of ours as the API's frame (the planes are shared).
fn to_api_frame(frame: &CpuFrame, timestamp_us: i64) -> VideoFrame {
    VideoFrame {
        format: VideoPixelFormat::Nv12,
        width: frame.width(),
        height: frame.height(),
        planes: frame
            .planes()
            .iter()
            .map(|p| VideoPlane {
                data: p.data.clone(),
                stride: p.stride,
            })
            .collect(),
        matrix: VideoMatrix::Bt709,
        range: VideoRange::Limited,
        timestamp_us,
    }
}

/// A libwebrtc NV12 buffer as the API's frame (copied out).
fn buffer_to_api_frame(buffer: &NV12Buffer, timestamp_us: i64) -> VideoFrame {
    let (w, h) = (buffer.width() as usize, buffer.height() as usize);
    let (stride_y, stride_uv) = buffer.strides();
    let (y, uv) = buffer.data();
    let copy = |plane: &[u8], stride: u32, rows: usize| {
        let stride = stride as usize;
        let mut out = Vec::with_capacity(w * rows);
        for row in 0..rows {
            out.extend_from_slice(&plane[row * stride..row * stride + w]);
        }
        VideoPlane {
            data: out.into(),
            stride: w,
        }
    };
    VideoFrame {
        format: VideoPixelFormat::Nv12,
        width: buffer.width(),
        height: buffer.height(),
        planes: vec![copy(y, stride_y, h), copy(uv, stride_uv, h / 2)],
        matrix: VideoMatrix::Bt709,
        range: VideoRange::Limited,
        timestamp_us,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sending(source_frames: u64, encoded: u64) -> Reading {
        Reading {
            active: true,
            source_frames,
            encoded,
            ..Reading::default()
        }
    }

    #[test]
    fn a_hardware_encoder_that_encodes_nothing_is_lost_after_three_reports() {
        let mut watch = EncoderWatch::new(EncoderBackend::Nvenc);
        assert!(!watch.update(&sending(30, 0), None));
        assert!(!watch.update(&sending(60, 0), None));
        assert!(!watch.update(&sending(90, 0), None));
        assert!(watch.update(&sending(120, 0), None));
        // Reported once.
        assert!(!watch.update(&sending(150, 0), None));
    }

    #[test]
    fn a_hardware_encoder_that_encodes_or_waits_for_frames_is_kept() {
        let mut watch = EncoderWatch::new(EncoderBackend::Nvenc);
        let nvenc = Some("SimulcastEncoderAdapter (NVIDIA H264 Encoder, NVIDIA H264 Encoder)");
        for i in 1..10 {
            assert!(!watch.update(&sending(30 * i, 90 * i), nvenc));
        }
        // No frames from the source (a stalled camera) is not the encoder's
        // fault, and neither are layers dynacast paused.
        for _ in 0..5 {
            assert!(!watch.update(&sending(270, 810), nvenc));
        }
        let paused = Reading {
            active: false,
            ..sending(1000, 810)
        };
        for _ in 0..5 {
            assert!(!watch.update(&paused, nvenc));
        }
    }

    #[test]
    fn a_hardware_encoder_libwebrtc_replaced_is_lost_at_once() {
        let mut watch = EncoderWatch::new(EncoderBackend::Nvenc);
        assert!(watch.update(&sending(30, 30), Some("libvpx")));
    }

    #[test]
    fn sources_and_codecs_map_both_ways() {
        for kind in [VideoKind::Camera, VideoKind::Screen] {
            assert_eq!(kind_of(source_of(kind)), Some(kind));
        }
        assert_eq!(kind_of(TrackSource::Microphone), None);
        assert_eq!(codec_of("video/VP8"), Some(VideoCodec::Vp8));
        assert_eq!(codec_of("video/h264"), Some(VideoCodec::H264));
        assert_eq!(codec_of("video/AV1"), Some(VideoCodec::Av1));
        assert_eq!(codec_of(""), None);
    }

    #[test]
    fn the_pattern_goes_into_libwebrtc_and_the_self_view_unchanged() {
        let frame = TestPattern::new(128, 72).frame(7, Duration::from_micros(42));
        let buffer = to_nv12_buffer(&frame);
        let (sy, suv) = buffer.strides();
        let (y, uv) = buffer.data();
        let planes = frame.planes();
        for row in 0..72 {
            assert_eq!(
                &y[row * sy as usize..][..128],
                &planes[0].data[row * planes[0].stride..][..128]
            );
        }
        for row in 0..36 {
            assert_eq!(
                &uv[row * suv as usize..][..128],
                &planes[1].data[row * planes[1].stride..][..128]
            );
        }
        let api = to_api_frame(&frame, 9);
        assert_eq!((api.width, api.height, api.timestamp_us), (128, 72, 9));
        assert!(Arc::ptr_eq(&api.planes[0].data, &planes[0].data));
    }

    // LiveKit's source spawns onto Tokio when made.
    #[tokio::test]
    async fn frames_larger_than_the_track_are_scaled_keeping_their_shape() {
        use livekit::webrtc::video_source::VideoResolution;

        let source = NativeVideoSource::new(
            VideoResolution {
                width: 640,
                height: 360,
            },
            true,
        );
        let (tx, feed) = VideoFeed::channel();
        let mut pusher = FramePusher::new(source, tx, (640, 360));

        // Larger: scaled down to the size sent.
        pusher.push(&TestPattern::new(1280, 720).frame(1, Duration::ZERO));
        let got = feed.latest().expect("a frame");
        assert_eq!((got.width, got.height), (640, 360));
        assert_eq!(got.planes[0].data.len(), 640 * 360);

        // A wider shape keeps it.
        pusher.push(&TestPattern::new(1280, 400).frame(2, Duration::ZERO));
        let got = feed.latest().expect("a frame");
        assert_eq!((got.width, got.height), (640, 200));

        // One that fits goes as it is: the self-view shares its planes.
        let small = TestPattern::new(320, 180).frame(3, Duration::ZERO);
        pusher.push(&small);
        let got = feed.latest().expect("a frame");
        assert_eq!((got.width, got.height), (320, 180));
        assert!(Arc::ptr_eq(&got.planes[0].data, &small.planes()[0].data));
    }

    #[test]
    fn rates_come_from_the_difference_between_readings() {
        let t0 = Instant::now();
        let first = Reading {
            width: 1280,
            height: 720,
            bytes: 1000,
            frames: 10,
            busy_s: 0.05,
            ..Reading::default()
        };
        let none = first.stats(
            VideoTrackId::from("t"),
            VideoDirection::Received,
            &Totals::default(),
            t0,
        );
        assert_eq!((none.fps, none.bitrate_kbps, none.frame_ms), (0.0, 0, None));
        let then = Reading {
            bytes: 126_000,
            frames: 40,
            busy_s: 0.2,
            ..first
        };
        let s = then.stats(
            VideoTrackId::from("t"),
            VideoDirection::Received,
            &first.totals(t0),
            t0 + Duration::from_secs(1),
        );
        assert!((s.fps - 30.0).abs() < 0.01, "{s:?}");
        assert_eq!(s.bitrate_kbps, 1000);
        assert!((s.frame_ms.unwrap() - 5.0).abs() < 0.01, "{s:?}");
        // A counter that goes back (another layer) counts from zero.
        let reset = Reading {
            frames: 3,
            bytes: 0,
            ..then
        };
        let s = reset.stats(
            VideoTrackId::from("t"),
            VideoDirection::Received,
            &then.totals(t0),
            t0 + Duration::from_secs(1),
        );
        assert!((s.fps - 3.0).abs() < 0.01, "{s:?}");
    }
}
