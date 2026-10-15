//! Video in calls (Phase 9 Tasks 10 and 11): the call's video tracks and
//! their pictures, the stage's tiles and how they are arranged, what we ask
//! the call server to send, our camera and screen share, and the video
//! settings the core reports. Pure: the core's events change this state, and
//! only `App`'s handlers in this file send commands.
//!
//! Pictures never pass through `update`: a [`VideoFeed`] is kept here and the
//! widget that shows it reads its newest frame when it is drawn. What the
//! widgets tell us is how big they are on screen, so the call server sends
//! a layer that is big enough and no bigger.

use std::collections::{HashMap, HashSet};
use std::fmt;

use iced::widget::image;
use iced::{Padding, Rectangle, Size, Task};
use twicecat_core::api::{
    AvatarId, Command, CoreError, CoreEvent, LocalVideo, ParticipantId, ScreenSourceId,
    ScreenSourceKind, ScreenSources, UserId, VideoAdjustment, VideoCodec, VideoDevices,
    VideoEncoders, VideoFeed, VideoKind, VideoQuality, VideoSettings, VideoStats, VideoTrackId,
    VideoTrackView,
};

use crate::calls::CurrentCall;
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder, one_line_capped};
use crate::ui::video_view::{GpuSeen, Place};

/// Most adjustments kept of one report.
pub const MAX_ADJUSTMENTS: usize = 32;
/// Most video tracks kept; the core never sends more than a call can hold.
pub const MAX_TRACKS: usize = 64;
/// Most screens and windows kept for the picker.
pub const MAX_SOURCES: usize = 64;
/// Characters of a window title shown in the picker.
pub const MAX_SOURCE_TITLE: usize = 80;
/// The size asked of the call server is rounded up to a multiple of this many
/// physical pixels, so dragging a window's edge is not a stream of commands.
pub const VIEW_BUCKET: u32 = 64;
/// Space between tiles and around them, logical pixels.
pub const GAP: f32 = 8.0;
/// The shape of a tile.
pub const TILE_ASPECT: f32 = 16.0 / 9.0;
/// The frame a thumbnail is shown in (width, height), logical pixels.
pub const THUMBNAIL_FRAME: (f32, f32) = (160.0, 90.0);
/// `picture` scaled to the largest size that keeps its shape inside `frame`.
pub fn fit_in(picture: Size, frame: Size) -> Size {
    let scale = (frame.width / picture.width.max(1.0)).min(frame.height / picture.height.max(1.0));
    Size::new(
        (picture.width * scale).floor().max(1.0),
        (picture.height * scale).floor().max(1.0),
    )
}
/// The picture a thumbnail may have at most, per side.
pub const MAX_THUMBNAIL_SIDE: u32 = 1024;

/// A control of the call's video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoMessage {
    /// A picture was laid out at this size, in physical pixels.
    TileSized {
        /// The track.
        track: VideoTrackId,
        /// Where it is drawn.
        place: Place,
        /// Its size on screen.
        width: u32,
        /// Its size on screen.
        height: u32,
    },
    /// Pin a track to the big place, or let go of it when it is pinned.
    Pin(VideoTrackId),
    /// Open the full-screen view of a track.
    Fullscreen(VideoTrackId),
    /// Close the full-screen view.
    ExitFullscreen,
    /// Show or hide the stage (the pictures stop coming while it is hidden).
    ToggleStage,
    /// The camera button (Ctrl+Shift+O).
    ToggleCamera,
    /// The share button (Ctrl+Shift+E): stops the share, or opens the picker.
    ToggleShare,
    /// A screen or window picked in the picker.
    PickSource(ScreenSourceId),
    /// The picker's "share the sound" box.
    ShareAudio(bool),
    /// The picker's quality menu.
    ShareQuality(VideoQuality),
    /// **Share** in the picker.
    StartShare,
    /// **Cancel** in the picker (or Esc).
    CancelShare,
    /// **Stop sharing** (the bar, the button).
    StopShare,
}

/// A screen or window the picker offers.
#[derive(Clone)]
pub struct SourceCard {
    /// For `StartScreenShare`.
    pub id: ScreenSourceId,
    /// A monitor or a window.
    pub kind: ScreenSourceKind,
    /// The monitor's name or the window's title, cleaned and capped.
    pub title: String,
    /// Size in physical pixels.
    pub width: u32,
    /// Size in physical pixels.
    pub height: u32,
    /// A picture of it, and the size that fits it in the picker's frame.
    pub thumbnail: Option<(image::Handle, Size)>,
}

// Window titles are user data.
impl fmt::Debug for SourceCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceCard")
            .field("kind", &self.kind)
            .field("thumbnail", &self.thumbnail.is_some())
            .finish_non_exhaustive()
    }
}

/// The share picker while it is open.
#[derive(Clone, Debug, Default)]
pub struct SharePicker {
    /// What can be shared; `None` until the core answered.
    pub sources: Option<Vec<SourceCard>>,
    /// The system shows its own picker once the share starts.
    pub system_picker: bool,
    /// The screen or window chosen.
    pub selected: Option<ScreenSourceId>,
    /// Share the sound too.
    pub audio: bool,
}

/// What the call server was last asked for one track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asked {
    /// Width, physical pixels (a multiple of [`VIEW_BUCKET`]).
    pub width: u32,
    /// Height, physical pixels.
    pub height: u32,
    /// Shown at all.
    pub visible: bool,
}

/// The call's video and the video settings.
pub struct VideoModel {
    /// The video tracks of the call, ours included.
    pub tracks: Vec<VideoTrackView>,
    /// The pictures, by track.
    pub feeds: HashMap<VideoTrackId, VideoFeed>,
    /// Our own camera and share.
    pub local: LocalVideo,
    /// The video settings as the core last echoed them.
    pub settings: Option<VideoSettings>,
    /// What the core changed in the settings it was last given (a value out
    /// of bounds): said on the Video card until the next change.
    pub adjusted: Vec<VideoAdjustment>,
    /// What our camera and share are really sent as when that differs from
    /// the settings, by kind: the encoder cannot do what was asked.
    pub sending: HashMap<VideoKind, Vec<VideoAdjustment>>,
    /// The cameras.
    pub devices: Option<VideoDevices>,
    /// The encoders this machine has, and the one in use.
    pub encoders: Option<VideoEncoders>,
    /// The latest numbers of the call's video.
    pub stats: Option<VideoStats>,
    /// The picture of the camera preview, while one is open.
    pub preview: Option<VideoFeed>,
    /// The track pinned to the big place by the user.
    pub pinned: Option<VideoTrackId>,
    /// The track shown full screen.
    pub fullscreen: Option<VideoTrackId>,
    /// The stage is shown (it is only when the call's room is open).
    pub stage_open: bool,
    /// How big each picture is on screen, physical pixels.
    pub sizes: HashMap<(VideoTrackId, Place), (u32, u32)>,
    /// What the call server was last asked, by track.
    pub asked: HashMap<VideoTrackId, Asked>,
    /// The share picker, while it is open.
    pub picker: Option<SharePicker>,
    /// `StartCamera` or `StopCamera` was sent and not answered.
    pub camera_busy: bool,
    /// `StartScreenShare` or `StopScreenShare` was sent and not answered.
    pub share_busy: bool,
    /// The window itself is in full-screen mode for the full-screen view.
    pub window_fullscreen: bool,
    /// Whether a video widget was ever drawn by the wgpu renderer.
    pub gpu: GpuSeen,
}

impl Default for VideoModel {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            feeds: HashMap::new(),
            local: LocalVideo::default(),
            settings: None,
            adjusted: Vec::new(),
            sending: HashMap::new(),
            devices: None,
            encoders: None,
            stats: None,
            preview: None,
            pinned: None,
            fullscreen: None,
            stage_open: true,
            sizes: HashMap::new(),
            asked: HashMap::new(),
            picker: None,
            camera_busy: false,
            share_busy: false,
            window_fullscreen: false,
            gpu: GpuSeen::default(),
        }
    }
}

// Counts only: no name, title or picture in logs.
impl fmt::Debug for VideoModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoModel")
            .field("tracks", &self.tracks.len())
            .field("feeds", &self.feeds.len())
            .field("camera", &self.local.camera.is_some())
            .field("screen", &self.local.screen.is_some())
            .field("settings", &self.settings.is_some())
            .field("picker", &self.picker.is_some())
            .finish_non_exhaustive()
    }
}

/// One tile of the stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// The video track, or `None` for a person without a picture (their
    /// avatar is drawn).
    pub track: Option<VideoTrackId>,
    /// A camera, a screen share, or neither.
    pub kind: Option<VideoKind>,
    /// The device that sends it.
    pub participant: ParticipantId,
    /// Their account.
    pub user_id: UserId,
    /// Their name as shown, cleaned and capped.
    pub name: String,
    /// Their picture.
    pub avatar: Option<AvatarId>,
    /// This device's own camera.
    pub is_local: bool,
    /// They are speaking.
    pub speaking: bool,
    /// Their microphone is off.
    pub mic_off: bool,
    /// Their camera is switched off for now (the avatar shows instead).
    pub camera_off: bool,
    /// Width of the picture now, pixels; 0 before the first frame.
    pub width: u32,
    /// Height of the picture now, pixels.
    pub height: u32,
    /// Frames per second now, when known.
    pub fps: Option<u32>,
    /// The codec, when known.
    pub codec: Option<VideoCodec>,
}

impl Tile {
    /// A picture can be drawn here: the track has a feed and its sender has
    /// not switched it off.
    pub fn draws_video(&self, model: &VideoModel) -> bool {
        self.track
            .as_ref()
            .is_some_and(|t| model.feeds.contains_key(t))
            && !self.camera_off
    }
}

/// What the picture sizes asked of the core should be: the on-screen size
/// rounded up to [`VIEW_BUCKET`] physical pixels on each side.
pub fn bucket(width: u32, height: u32) -> (u32, u32) {
    let up = |v: u32| v.max(1).div_ceil(VIEW_BUCKET).saturating_mul(VIEW_BUCKET);
    (up(width), up(height))
}

/// The tiles of the stage, in order: every remote screen share first, then
/// each person, with their camera when they send one and their avatar when
/// they do not. Our own screen share is not a tile: the sharing bar says it.
pub fn tiles(model: &VideoModel, call: &CurrentCall) -> Vec<Tile> {
    let mut shares = Vec::new();
    let mut people = Vec::new();
    let mut seen: HashSet<&ParticipantId> = HashSet::new();
    for p in &call.participants {
        let name = call.name_of(p);
        let avatar = call.member(&p.user_id).and_then(|m| m.avatar.clone());
        let on = |kind: VideoKind| {
            model
                .tracks
                .iter()
                .find(|t| t.participant == p.id && t.kind == kind)
        };
        let tile = |view: Option<&VideoTrackView>, kind: Option<VideoKind>| Tile {
            track: view.map(|v| v.id.clone()),
            kind,
            participant: p.id.clone(),
            user_id: p.user_id.clone(),
            name: name.clone(),
            avatar: avatar.clone(),
            is_local: p.is_local,
            speaking: p.speaking,
            mic_off: p.muted || (p.is_local && (call.muted || call.deafened)),
            camera_off: view.is_some_and(|v| v.muted && kind == Some(VideoKind::Camera)),
            width: view.map_or(0, |v| v.width),
            height: view.map_or(0, |v| v.height),
            fps: view.and_then(|v| v.fps),
            codec: view.and_then(|v| v.codec),
        };
        if !p.is_local
            && let Some(screen) = on(VideoKind::Screen)
        {
            shares.push(tile(Some(screen), Some(VideoKind::Screen)));
        }
        if seen.insert(&p.id) {
            people.push(match on(VideoKind::Camera) {
                Some(camera) => tile(Some(camera), Some(VideoKind::Camera)),
                None => tile(None, None),
            });
        }
    }
    shares.extend(people);
    shares.truncate(MAX_TRACKS);
    shares
}

/// The tile in the big place: the pinned track when it is there, else the
/// first remote screen share, else none (everyone is the same size).
pub fn focus(model: &VideoModel, tiles: &[Tile]) -> Option<usize> {
    if let Some(pinned) = &model.pinned
        && let Some(i) = tiles.iter().position(|t| t.track.as_ref() == Some(pinned))
    {
        return Some(i);
    }
    tiles
        .iter()
        .position(|t| t.kind == Some(VideoKind::Screen) && !t.is_local)
}

/// Where each of `count` tiles goes in an area of `area`: a grid of equal
/// 16:9 tiles that fills the area best, or, with a `focus`, the focused tile
/// as large as fits above a strip of the others.
pub fn arrange(area: Size, count: usize, focus: Option<usize>) -> Vec<Rectangle> {
    if count == 0 || area.width < 1.0 || area.height < 1.0 {
        return Vec::new();
    }
    match focus.filter(|f| *f < count && count > 1) {
        Some(main) => arrange_focus(area, count, main),
        None => grid(area, count),
    }
}

/// The columns and tile size of the grid with the biggest tiles.
fn best_grid(area: Size, count: usize) -> (usize, Size) {
    let mut best = (1, Size::new(0.0, 0.0));
    for columns in 1..=count {
        let rows = count.div_ceil(columns);
        let width = (area.width - GAP * (columns as f32 - 1.0)) / columns as f32;
        let height = (area.height - GAP * (rows as f32 - 1.0)) / rows as f32;
        let (w, h) = if width / height > TILE_ASPECT {
            (height * TILE_ASPECT, height)
        } else {
            (width, width / TILE_ASPECT)
        };
        if w * h > best.1.width * best.1.height {
            best = (columns, Size::new(w.max(0.0), h.max(0.0)));
        }
    }
    best
}

fn grid(area: Size, count: usize) -> Vec<Rectangle> {
    let (columns, tile) = best_grid(area, count);
    let rows = count.div_ceil(columns);
    let total_h = rows as f32 * tile.height + GAP * (rows as f32 - 1.0);
    let top = ((area.height - total_h) / 2.0).max(0.0);
    (0..count)
        .map(|i| {
            let (row, col) = (i / columns, i % columns);
            // A short last row is centred.
            let in_row = if row + 1 == rows {
                count - row * columns
            } else {
                columns
            };
            let row_w = in_row as f32 * tile.width + GAP * (in_row as f32 - 1.0);
            let left = ((area.width - row_w) / 2.0).max(0.0);
            Rectangle::new(
                iced::Point::new(
                    left + col as f32 * (tile.width + GAP),
                    top + row as f32 * (tile.height + GAP),
                ),
                tile,
            )
        })
        .collect()
}

/// The share of the area's height the strip of the other tiles takes.
const STRIP_SHARE: f32 = 0.22;
/// The strip is at least this high, and never taller than a third of the area.
const STRIP_MIN: f32 = 72.0;

fn arrange_focus(area: Size, count: usize, main: usize) -> Vec<Rectangle> {
    let others = count - 1;
    let strip_h = (area.height * STRIP_SHARE)
        .max(STRIP_MIN)
        .min(area.height / 3.0);
    let main_area = Size::new(area.width, (area.height - strip_h - GAP).max(1.0));
    let big = fit_aspect(main_area);
    let big_rect = Rectangle::new(
        iced::Point::new(
            (area.width - big.width) / 2.0,
            (main_area.height - big.height) / 2.0,
        ),
        big,
    );
    // The others share one row, as many as fit at the strip's height and the
    // rest shrinking to fit.
    let tile_h = strip_h;
    let tile_w =
        ((area.width - GAP * (others as f32 - 1.0)) / others as f32).min(tile_h * TILE_ASPECT);
    let tile = fit_aspect(Size::new(tile_w, tile_h));
    let row_w = others as f32 * tile.width + GAP * (others as f32 - 1.0);
    let left = ((area.width - row_w) / 2.0).max(0.0);
    let top = main_area.height + GAP + (strip_h - tile.height) / 2.0;
    let mut small = 0;
    (0..count)
        .map(|i| {
            if i == main {
                big_rect
            } else {
                let rect = Rectangle::new(
                    iced::Point::new(left + small as f32 * (tile.width + GAP), top),
                    tile,
                );
                small += 1;
                rect
            }
        })
        .collect()
}

/// The largest 16:9 rectangle inside `area`.
fn fit_aspect(area: Size) -> Size {
    if area.width / area.height.max(1.0) > TILE_ASPECT {
        Size::new(area.height * TILE_ASPECT, area.height)
    } else {
        Size::new(area.width, area.width / TILE_ASPECT)
    }
}

/// The padding that puts a child of `rect`'s size at `rect`'s place inside an
/// area that fills the stage.
pub fn place(rect: Rectangle) -> Padding {
    Padding::new(0.0).top(rect.y).left(rect.x)
}

/// The quality menu's entries, with the words for each.
pub const QUALITIES: [VideoQuality; 6] = [
    VideoQuality::Auto,
    VideoQuality::Hd720p30,
    VideoQuality::Hd1080p60,
    VideoQuality::Qhd1440p60,
    VideoQuality::Uhd2160p30,
    VideoQuality::Uhd2160p60,
];

/// A quality in words.
pub const fn quality_words(quality: VideoQuality) -> &'static str {
    match quality {
        VideoQuality::Auto => "Automatic",
        VideoQuality::Hd720p30 => "720p, 30 fps",
        VideoQuality::Hd1080p60 => "1080p, 60 fps",
        VideoQuality::Qhd1440p60 => "1440p, 60 fps",
        VideoQuality::Uhd2160p30 => "4K, 30 fps",
        VideoQuality::Uhd2160p60 => "4K, 60 fps",
    }
}

/// A quality as a menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QualityOption(pub VideoQuality);

impl QualityOption {
    /// Every quality.
    pub const ALL: [Self; 6] = [
        Self(QUALITIES[0]),
        Self(QUALITIES[1]),
        Self(QUALITIES[2]),
        Self(QUALITIES[3]),
        Self(QUALITIES[4]),
        Self(QUALITIES[5]),
    ];
}

impl fmt::Display for QualityOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(quality_words(self.0))
    }
}

impl VideoModel {
    /// What the video settings say, or the defaults before the core answered.
    pub fn settings_or_default(&self) -> VideoSettings {
        self.settings.clone().unwrap_or_default()
    }

    /// The track view of `id`.
    pub fn track(&self, id: &VideoTrackId) -> Option<&VideoTrackView> {
        self.tracks.iter().find(|t| t.id == *id)
    }

    /// The stage has something to show: a picture other than our own share.
    pub fn has_pictures(&self, call: &CurrentCall) -> bool {
        tiles(self, call).iter().any(|t| t.track.is_some())
    }

    /// Our own camera is on.
    pub fn camera_on(&self) -> bool {
        self.local.camera.is_some()
    }

    /// We are sharing a screen or window.
    pub fn sharing(&self) -> bool {
        self.local.screen.is_some()
    }

    /// Applies a video event of the core API. Returns `false` for any other.
    pub fn on_event(&mut self, event: CoreEvent) -> bool {
        match event {
            CoreEvent::VideoDevices(devices) => self.devices = Some(devices),
            CoreEvent::ScreenSources(sources) => self.on_sources(sources),
            CoreEvent::VideoSettings(settings) => {
                // What was said about the last change is about that change.
                self.adjusted.clear();
                self.settings = Some(settings);
            }
            CoreEvent::VideoSettingsAdjusted(changes) => {
                self.adjusted = changes.into_iter().take(MAX_ADJUSTMENTS).collect();
            }
            CoreEvent::VideoSending { kind, changes } => {
                self.sending
                    .insert(kind, changes.into_iter().take(MAX_ADJUSTMENTS).collect());
            }
            CoreEvent::VideoTracks { tracks, .. } => self.on_tracks(tracks),
            CoreEvent::VideoFeed { track, feed } => {
                if self.feeds.len() < MAX_TRACKS || self.feeds.contains_key(&track) {
                    self.feeds.insert(track, feed);
                }
            }
            CoreEvent::CameraPreview(feed) => self.preview = Some(feed),
            CoreEvent::LocalVideo(local) => {
                self.camera_busy = false;
                self.share_busy = false;
                // What was said about a track that is off is over.
                if local.camera.is_none() {
                    self.sending.remove(&VideoKind::Camera);
                }
                if local.screen.is_none() {
                    self.sending.remove(&VideoKind::Screen);
                }
                self.local = local;
            }
            CoreEvent::VideoEncoders(encoders) => self.encoders = Some(encoders),
            CoreEvent::VideoStats(stats) => self.stats = Some(stats),
            _ => return false,
        }
        true
    }

    fn on_tracks(&mut self, mut tracks: Vec<VideoTrackView>) {
        tracks.truncate(MAX_TRACKS);
        let alive: HashSet<&VideoTrackId> = tracks.iter().map(|t| &t.id).collect();
        self.feeds.retain(|id, _| alive.contains(id));
        self.sizes.retain(|(id, _), _| alive.contains(id));
        self.asked.retain(|id, _| alive.contains(id));
        if self.pinned.as_ref().is_some_and(|t| !alive.contains(t)) {
            self.pinned = None;
        }
        if self.fullscreen.as_ref().is_some_and(|t| !alive.contains(t)) {
            self.fullscreen = None;
        }
        self.tracks = tracks;
    }

    fn on_sources(&mut self, sources: ScreenSources) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        picker.system_picker = sources.system_picker;
        let cards = sources
            .sources
            .into_iter()
            .take(MAX_SOURCES)
            .map(|s| SourceCard {
                id: s.id,
                kind: s.kind,
                title: name_or_placeholder(
                    &one_line_capped(&s.title, MAX_SOURCE_TITLE),
                    MAX_NAME_CHARS,
                ),
                width: s.width,
                height: s.height,
                thumbnail: s.thumbnail.and_then(|t| {
                    let fits = t.width > 0
                        && t.height > 0
                        && t.width <= MAX_THUMBNAIL_SIDE
                        && t.height <= MAX_THUMBNAIL_SIDE
                        && t.rgba.len() == t.width as usize * t.height as usize * 4;
                    fits.then(|| {
                        let handle = image::Handle::from_rgba(t.width, t.height, t.rgba.to_vec());
                        let fit = fit_in(
                            Size::new(t.width as f32, t.height as f32),
                            Size::new(THUMBNAIL_FRAME.0, THUMBNAIL_FRAME.1),
                        );
                        (handle, fit)
                    })
                }),
            })
            .collect();
        picker.sources = Some(cards);
    }

    /// A refused command: what waited for an answer stops waiting.
    pub fn on_error(&mut self, _error: &CoreError) {
        self.camera_busy = false;
        self.share_busy = false;
    }

    /// The call ended: its pictures and what was asked go. The settings, the
    /// cameras and the encoders stay.
    pub fn end_call(&mut self) {
        self.tracks.clear();
        self.feeds.clear();
        self.local = LocalVideo::default();
        self.stats = None;
        self.sending.clear();
        self.pinned = None;
        self.fullscreen = None;
        self.sizes.clear();
        self.asked.clear();
        self.picker = None;
        self.camera_busy = false;
        self.share_busy = false;
    }

    /// The size the call server should send `track` at: the biggest place it
    /// is shown in, rounded up; `None` while it is not shown at all.
    pub fn wanted(&self, track: &VideoTrackId, shown: &HashSet<VideoTrackId>) -> Option<Asked> {
        if !shown.contains(track) {
            return None;
        }
        let (w, h) = self
            .sizes
            .iter()
            .filter(|((id, _), _)| id == track)
            .map(|(_, size)| *size)
            .max_by_key(|(w, h)| u64::from(*w) * u64::from(*h))?;
        let (width, height) = bucket(w, h);
        Some(Asked {
            width,
            height,
            visible: true,
        })
    }
}

impl CurrentCall {
    /// `p` as a name: the name the call reports, else the member's, else the
    /// account; cleaned and capped.
    pub fn name_of(&self, p: &twicecat_core::api::Participant) -> String {
        let member = self.member(&p.user_id);
        let name = p
            .display_name
            .as_deref()
            .or_else(|| member.and_then(|m| m.display_name.as_deref()))
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(p.user_id.as_str());
        name_or_placeholder(name, MAX_NAME_CHARS)
    }
}

impl App {
    /// The tracks shown right now, so the others are asked to stop: those of
    /// the stage while the call's room is open and the stage is, and the one
    /// in the full-screen view.
    fn shown_tracks(&self) -> HashSet<VideoTrackId> {
        let mut shown = HashSet::new();
        let Some(call) = self.calls.call.as_ref() else {
            return shown;
        };
        let video = &self.calls.video;
        if self.stage_visible() {
            for tile in tiles(video, call) {
                // Its feed follows the track list by a moment: the track is
                // already wanted.
                if let Some(track) = tile.track
                    && !tile.camera_off
                {
                    shown.insert(track);
                }
            }
        }
        if let Some(track) = &video.fullscreen {
            shown.insert(track.clone());
        }
        shown
    }

    /// The stage is on screen: a call with pictures, its room open, the stage
    /// not folded, and a pane that has room for it.
    pub fn stage_visible(&self) -> bool {
        let Some(call) = self.calls.call.as_ref() else {
            return false;
        };
        let video = &self.calls.video;
        video.stage_open
            && self.selected.as_ref() == Some(&call.room_id)
            && !self.pane_layout().panel_fills
            && video.has_pictures(call)
    }

    /// Tells the core which pictures are on screen and how big, once per
    /// change: after every message.
    pub(crate) fn video_sync_views(&mut self) -> Task<Message> {
        let shown = self.shown_tracks();
        let remote: Vec<VideoTrackId> = self
            .calls
            .video
            .tracks
            .iter()
            .filter(|t| !t.is_local)
            .map(|t| t.id.clone())
            .collect();
        let mut commands = Vec::new();
        for track in remote {
            let video = &mut self.calls.video;
            let wanted = video.wanted(&track, &shown);
            let before = video.asked.get(&track).copied();
            let now = match (wanted, before) {
                (Some(w), _) => Some(w),
                // Hidden: said once, with the last size.
                (None, Some(b)) if b.visible => Some(Asked {
                    visible: false,
                    ..b
                }),
                (None, None) if !shown.contains(&track) => Some(Asked {
                    width: VIEW_BUCKET,
                    height: VIEW_BUCKET,
                    visible: false,
                }),
                (None, _) => None,
            };
            if let Some(now) = now
                && before != Some(now)
            {
                video.asked.insert(track.clone(), now);
                commands.push(Command::SetVideoView {
                    track,
                    width: now.width,
                    height: now.height,
                    visible: now.visible,
                });
            }
        }
        let mut tasks: Vec<Task<Message>> = commands.into_iter().map(|c| self.send(c)).collect();
        tasks.push(self.sync_window_mode());
        Task::batch(tasks)
    }

    /// The window is full screen exactly while the full-screen view is open:
    /// also when the track ends under it.
    fn sync_window_mode(&mut self) -> Task<Message> {
        let video = &mut self.calls.video;
        let wanted = video.fullscreen.is_some();
        if video.window_fullscreen == wanted {
            return Task::none();
        }
        video.window_fullscreen = wanted;
        let mode = if wanted {
            iced::window::Mode::Fullscreen
        } else {
            iced::window::Mode::Windowed
        };
        iced::window::oldest().and_then(move |id| iced::window::set_mode(id, mode))
    }

    /// A video event of the core.
    pub(crate) fn on_video_event(&mut self, event: CoreEvent) -> Task<Message> {
        let handled = self.calls.video.on_event(event);
        debug_assert!(handled, "on_video_event got another kind of event");
        Task::none()
    }

    /// A control of the call's video.
    pub(crate) fn on_video(&mut self, message: VideoMessage) -> Task<Message> {
        match message {
            VideoMessage::TileSized {
                track,
                place,
                width,
                height,
            } => {
                if self.calls.video.track(&track).is_some() && width > 0 && height > 0 {
                    self.calls
                        .video
                        .sizes
                        .insert((track, place), (width, height));
                }
                Task::none()
            }
            VideoMessage::Pin(track) => {
                let video = &mut self.calls.video;
                video.pinned = if video.pinned.as_ref() == Some(&track) {
                    None
                } else {
                    video.track(&track).map(|t| t.id.clone())
                };
                Task::none()
            }
            VideoMessage::Fullscreen(track) => {
                if self.calls.video.feeds.contains_key(&track) {
                    self.calls.video.fullscreen = Some(track);
                }
                Task::none()
            }
            VideoMessage::ExitFullscreen => {
                let video = &mut self.calls.video;
                video.fullscreen = None;
                video
                    .sizes
                    .retain(|(_, place), _| *place != Place::Fullscreen);
                Task::none()
            }
            VideoMessage::ToggleStage => {
                self.calls.video.stage_open = !self.calls.video.stage_open;
                Task::none()
            }
            VideoMessage::ToggleCamera => self.toggle_camera(),
            VideoMessage::ToggleShare => {
                if self.calls.video.sharing() {
                    self.on_video(VideoMessage::StopShare)
                } else {
                    self.open_picker()
                }
            }
            VideoMessage::PickSource(id) => {
                if let Some(picker) = self.calls.video.picker.as_mut() {
                    picker.selected = Some(id);
                }
                Task::none()
            }
            VideoMessage::ShareAudio(on) => {
                if let Some(picker) = self.calls.video.picker.as_mut() {
                    picker.audio = on;
                }
                Task::none()
            }
            VideoMessage::ShareQuality(quality) => self.set_screen_quality(quality),
            VideoMessage::StartShare => self.start_share(),
            VideoMessage::CancelShare => {
                self.calls.video.picker = None;
                Task::none()
            }
            VideoMessage::StopShare => {
                let video = &mut self.calls.video;
                if !video.sharing() || video.share_busy {
                    return Task::none();
                }
                video.share_busy = true;
                self.send(Command::StopScreenShare)
            }
        }
    }

    fn toggle_camera(&mut self) -> Task<Message> {
        if self.calls.call.is_none() || self.calls.video.camera_busy {
            return Task::none();
        }
        self.calls.video.camera_busy = true;
        if self.calls.video.camera_on() {
            self.send(Command::StopCamera)
        } else {
            let quality = self.calls.video.settings_or_default().camera_quality;
            self.send(Command::StartCamera {
                device: None,
                quality,
            })
        }
    }

    fn open_picker(&mut self) -> Task<Message> {
        let video = &mut self.calls.video;
        if self.calls.call.is_none() || video.share_busy || video.picker.is_some() {
            return Task::none();
        }
        video.picker = Some(SharePicker::default());
        self.send(Command::ListScreenSources)
    }

    fn start_share(&mut self) -> Task<Message> {
        let video = &mut self.calls.video;
        let Some(picker) = video.picker.as_ref() else {
            return Task::none();
        };
        // Where we pick, a screen has to be picked; where the system picks,
        // it asks after this.
        let ready = picker.system_picker || picker.selected.is_some();
        if !ready || video.share_busy || self.calls.call.is_none() {
            return Task::none();
        }
        let source = picker.selected.clone().filter(|_| !picker.system_picker);
        let audio = picker.audio;
        let quality = video.settings_or_default().screen_quality;
        video.picker = None;
        video.share_busy = true;
        self.send(Command::StartScreenShare {
            source,
            quality,
            audio,
        })
    }

    fn set_screen_quality(&mut self, quality: VideoQuality) -> Task<Message> {
        let mut settings = self.calls.video.settings_or_default();
        if settings.screen_quality == quality {
            return Task::none();
        }
        settings.screen_quality = quality;
        // Shown at once; the core's echo replaces it.
        self.calls.video.settings = Some(settings.clone());
        self.send(Command::UpdateVideoSettings(settings))
    }

    /// After a login: the video settings, so the controls show the choices.
    pub(crate) fn video_after_login(&mut self) -> Task<Message> {
        self.send(Command::GetVideoSettings)
    }
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{
        CallIntent, CallState, CleaningStatus, DeviceId, ListDiff, Participant, RoomId, RoomSummary,
    };

    use super::*;

    fn track(id: &str, who: &str, kind: VideoKind, local: bool) -> VideoTrackView {
        VideoTrackView {
            id: VideoTrackId::from(id),
            participant: ParticipantId::from(format!("{who}-dev")),
            user_id: UserId::from(format!("@{who}:x")),
            device_id: DeviceId::from("DEV"),
            is_local: local,
            kind,
            width: 1280,
            height: 720,
            fps: Some(30),
            codec: Some(VideoCodec::H264),
            muted: false,
        }
    }

    fn person(who: &str) -> Participant {
        Participant {
            id: ParticipantId::from(format!("{who}-dev")),
            user_id: UserId::from(format!("@{who}:x")),
            device_id: DeviceId::from("DEV"),
            display_name: Some(who.to_uppercase()),
            is_local: who == "a",
            muted: false,
            speaking: false,
            volume_percent: 100,
            share_volume_percent: 100,
            cleaning: CleaningStatus::Off,
        }
    }

    fn call_of(people: &[&str]) -> CurrentCall {
        let mut call = CurrentCall::new(
            RoomId::from("!a:x"),
            crate::calls::CallPhase::Connected { encrypted: true },
        );
        call.participants = people.iter().map(|p| person(p)).collect();
        call
    }

    fn with_tracks(tracks: Vec<VideoTrackView>) -> VideoModel {
        let mut model = VideoModel::default();
        model.on_tracks(tracks);
        model
    }

    #[test]
    fn a_person_without_a_camera_is_an_avatar_tile() {
        let call = call_of(&["a", "b", "c"]);
        let model = with_tracks(vec![track("tb", "b", VideoKind::Camera, false)]);
        let tiles = tiles(&model, &call);
        assert_eq!(tiles.len(), 3);
        let b = tiles.iter().find(|t| t.name == "B").unwrap();
        assert_eq!(b.track, Some(VideoTrackId::from("tb")));
        let a = tiles.iter().find(|t| t.name == "A").unwrap();
        assert!(a.track.is_none() && a.is_local);
    }

    #[test]
    fn screen_shares_come_first_and_ours_is_no_tile() {
        let call = call_of(&["a", "b"]);
        let model = with_tracks(vec![
            track("cam-b", "b", VideoKind::Camera, false),
            track("scr-b", "b", VideoKind::Screen, false),
            track("scr-a", "a", VideoKind::Screen, true),
        ]);
        let tiles = tiles(&model, &call);
        let kinds: Vec<_> = tiles.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [Some(VideoKind::Screen), None, Some(VideoKind::Camera)],
            "{tiles:?}"
        );
        assert!(
            tiles
                .iter()
                .all(|t| t.track.as_ref().map(VideoTrackId::as_str) != Some("scr-a"))
        );
    }

    #[test]
    fn a_muted_camera_shows_the_avatar() {
        let call = call_of(&["a", "b"]);
        let mut view = track("tb", "b", VideoKind::Camera, false);
        view.muted = true;
        let mut model = with_tracks(vec![view]);
        let (_tx, feed) = VideoFeed::channel();
        model.feeds.insert(VideoTrackId::from("tb"), feed);
        let tiles = tiles(&model, &call);
        let b = tiles.iter().find(|t| t.name == "B").unwrap();
        assert!(b.camera_off && !b.draws_video(&model));
    }

    #[test]
    fn focus_is_the_pin_else_the_first_remote_share() {
        let call = call_of(&["a", "b", "c"]);
        let mut model = with_tracks(vec![
            track("cam-b", "b", VideoKind::Camera, false),
            track("scr-c", "c", VideoKind::Screen, false),
        ]);
        let shown = tiles(&model, &call);
        let share = shown.iter().position(|t| t.kind == Some(VideoKind::Screen));
        assert_eq!(focus(&model, &shown), share);
        model.pinned = Some(VideoTrackId::from("cam-b"));
        let pinned = shown
            .iter()
            .position(|t| t.track.as_ref().map(VideoTrackId::as_str) == Some("cam-b"));
        assert_eq!(focus(&model, &shown), pinned);
        model.pinned = Some(VideoTrackId::from("gone"));
        assert_eq!(focus(&model, &shown), share);
        model.pinned = None;
        let no_share = with_tracks(vec![track("cam-b", "b", VideoKind::Camera, false)]);
        assert_eq!(focus(&no_share, &tiles(&no_share, &call)), None);
    }

    #[test]
    fn the_grid_fills_the_area_with_equal_16_9_tiles() {
        let area = Size::new(1000.0, 500.0);
        for count in 1..=9 {
            let rects = arrange(area, count, None);
            assert_eq!(rects.len(), count);
            let first = rects[0];
            for r in &rects {
                assert!((r.width - first.width).abs() < 0.01);
                assert!((r.width / r.height - TILE_ASPECT).abs() < 0.01, "{r:?}");
                assert!(r.x >= -0.01 && r.y >= -0.01, "{r:?}");
                assert!(r.x + r.width <= area.width + 0.01, "{count}: {r:?}");
                assert!(r.y + r.height <= area.height + 0.01, "{count}: {r:?}");
            }
            // No two overlap.
            for (i, a) in rects.iter().enumerate() {
                for b in &rects[i + 1..] {
                    assert!(
                        a.intersection(b)
                            .is_none_or(|x| x.width < 0.5 || x.height < 0.5)
                    );
                }
            }
        }
        // One tile takes the most it can.
        let one = arrange(area, 1, None)[0];
        assert!(
            (one.height - 500.0).abs() < 0.01 && (one.width - 500.0 * TILE_ASPECT).abs() < 0.01
        );
    }

    #[test]
    fn a_focused_tile_is_big_above_a_strip_of_the_others() {
        let area = Size::new(1000.0, 600.0);
        let rects = arrange(area, 4, Some(2));
        let big = rects[2];
        assert!(big.width > 500.0, "{big:?}");
        for (i, r) in rects.iter().enumerate() {
            assert!(r.x >= -0.01 && r.x + r.width <= area.width + 0.01, "{r:?}");
            assert!(r.y + r.height <= area.height + 0.01, "{r:?}");
            if i != 2 {
                assert!(r.y >= big.y + big.height - 0.01, "{r:?} under {big:?}");
                assert!(r.width < big.width);
            }
        }
        // Alone, or with nothing to focus, it is a grid.
        assert_eq!(arrange(area, 1, Some(0)), arrange(area, 1, None));
        assert_eq!(arrange(area, 3, Some(9)), arrange(area, 3, None));
        assert!(arrange(Size::new(0.0, 0.0), 3, None).is_empty());
        assert!(arrange(area, 0, None).is_empty());
    }

    #[test]
    fn sizes_are_rounded_up_to_the_bucket() {
        assert_eq!(bucket(1, 1), (64, 64));
        assert_eq!(bucket(64, 65), (64, 128));
        assert_eq!(bucket(960, 540), (960, 576));
        assert_eq!(bucket(0, 0), (64, 64));
    }

    #[test]
    fn a_picture_not_shown_has_no_wanted_size() {
        let mut model = with_tracks(vec![track("t", "b", VideoKind::Camera, false)]);
        model
            .sizes
            .insert((VideoTrackId::from("t"), Place::Tile), (300, 170));
        let id = VideoTrackId::from("t");
        let mut shown = HashSet::new();
        assert_eq!(model.wanted(&id, &shown), None);
        shown.insert(id.clone());
        assert_eq!(
            model.wanted(&id, &shown),
            Some(Asked {
                width: 320,
                height: 192,
                visible: true
            })
        );
        // The biggest place wins.
        model
            .sizes
            .insert((id.clone(), Place::Fullscreen), (1900, 1000));
        assert_eq!(model.wanted(&id, &shown).map(|a| a.width), Some(1920));
        // Shown but not measured yet: nothing to ask.
        model.sizes.clear();
        assert_eq!(model.wanted(&id, &shown), None);
    }

    #[test]
    fn tracks_that_end_take_their_feeds_pin_and_sizes_with_them() {
        let mut model = with_tracks(vec![
            track("t1", "b", VideoKind::Camera, false),
            track("t2", "c", VideoKind::Camera, false),
        ]);
        let (_tx, feed) = VideoFeed::channel();
        for id in ["t1", "t2"] {
            model.feeds.insert(VideoTrackId::from(id), feed.clone());
            model
                .sizes
                .insert((VideoTrackId::from(id), Place::Tile), (10, 10));
        }
        model.pinned = Some(VideoTrackId::from("t2"));
        model.fullscreen = Some(VideoTrackId::from("t2"));
        model.on_tracks(vec![track("t1", "b", VideoKind::Camera, false)]);
        assert_eq!(model.feeds.len(), 1);
        assert_eq!(model.sizes.len(), 1);
        assert!(model.pinned.is_none() && model.fullscreen.is_none());
    }

    #[test]
    fn sources_are_cleaned_and_a_bad_thumbnail_is_dropped() {
        use twicecat_core::api::{ScreenSource, ScreenThumbnail};
        let mut model = VideoModel {
            picker: Some(SharePicker::default()),
            ..VideoModel::default()
        };
        let source = |title: &str, thumb: Option<ScreenThumbnail>| ScreenSource {
            id: ScreenSourceId::from(title),
            kind: ScreenSourceKind::Window,
            title: title.into(),
            width: 800,
            height: 600,
            thumbnail: thumb,
        };
        let good = ScreenThumbnail {
            width: 2,
            height: 1,
            rgba: vec![0; 8].into(),
        };
        let short = ScreenThumbnail {
            width: 2,
            height: 2,
            rgba: vec![0; 8].into(),
        };
        model.on_event(CoreEvent::ScreenSources(ScreenSources {
            system_picker: false,
            sources: vec![
                source("A\nB", Some(good)),
                source("short", Some(short)),
                source(&"x".repeat(500), None),
            ],
        }));
        let cards = model.picker.as_ref().unwrap().sources.as_ref().unwrap();
        assert_eq!(cards.len(), 3);
        assert!(!cards[0].title.contains('\n') && cards[0].thumbnail.is_some());
        assert!(cards[1].thumbnail.is_none());
        assert!(cards[2].title.chars().count() <= MAX_SOURCE_TITLE + 1);
        // Titles stay out of Debug.
        assert!(!format!("{:?}", cards[0]).contains('B'));
    }

    #[test]
    fn leaving_the_call_clears_the_pictures_but_keeps_the_settings() {
        let mut model = with_tracks(vec![track("t", "b", VideoKind::Camera, false)]);
        model.settings = Some(VideoSettings::default());
        model.local.camera = Some(VideoTrackId::from("local"));
        model.camera_busy = true;
        model.end_call();
        assert!(model.tracks.is_empty() && model.local == LocalVideo::default());
        assert!(!model.camera_busy && model.settings.is_some());
    }

    // ---- through App::update ------------------------------------------

    use tokio::sync::mpsc;

    use crate::bridge::EventsSlot;
    use crate::state::{Screen, Startup};

    fn core(event: CoreEvent) -> Message {
        Message::Core(Box::new(event))
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// An app on the main screen, in the call of room `a` with `b` in it,
    /// the room open, commands drained.
    fn in_call() -> (App, mpsc::Receiver<Command>) {
        let (tx, mut rx) = mpsc::channel(64);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let _ = app.update(core(CoreEvent::NoSession));
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }));
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
            values: vec![RoomSummary::new(RoomId::from("!a:x"), "a".to_owned())],
        }])));
        assert_eq!(app.screen, Screen::Main);
        let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
        let _ = app.update(core(CoreEvent::CallState(CallState::Joining {
            room_id: RoomId::from("!a:x"),
        })));
        let _ = app.update(core(CoreEvent::CallState(CallState::Connected {
            room_id: RoomId::from("!a:x"),
            encrypted: true,
        })));
        let _ = app.update(core(CoreEvent::CallParticipants {
            room_id: RoomId::from("!a:x"),
            diffs: vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        }));
        drain(&mut rx);
        (app, rx)
    }

    fn remote_camera(app: &mut App) -> VideoTrackId {
        let (tx, feed) = VideoFeed::channel();
        std::mem::forget(tx);
        let id = VideoTrackId::from("tb");
        let _ = app.update(core(CoreEvent::VideoTracks {
            room_id: RoomId::from("!a:x"),
            tracks: vec![track("tb", "b", VideoKind::Camera, false)],
        }));
        let _ = app.update(core(CoreEvent::VideoFeed {
            track: id.clone(),
            feed,
        }));
        id
    }

    fn sized(app: &mut App, id: &VideoTrackId, w: u32, h: u32) {
        let _ = app.update(Message::Video(VideoMessage::TileSized {
            track: id.clone(),
            place: Place::Tile,
            width: w,
            height: h,
        }));
    }

    fn views(sent: &[Command]) -> Vec<(u32, u32, bool)> {
        sent.iter()
            .filter_map(|c| match c {
                Command::SetVideoView {
                    width,
                    height,
                    visible,
                    ..
                } => Some((*width, *height, *visible)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn logging_in_asks_for_the_video_settings() {
        let (tx, mut rx) = mpsc::channel(16);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let _ = app.update(core(CoreEvent::NoSession));
        drain(&mut rx);
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::GetVideoSettings))
        );
    }

    #[test]
    fn a_remote_picture_is_asked_for_at_the_size_it_is_drawn() {
        let (mut app, mut rx) = in_call();
        let id = remote_camera(&mut app);
        // Shown but not measured: nothing to ask yet.
        assert!(views(&drain(&mut rx)).is_empty());
        sized(&mut app, &id, 500, 280);
        assert_eq!(views(&drain(&mut rx)), [(512, 320, true)]);
        // A small change stays in the same bucket: nothing sent.
        sized(&mut app, &id, 510, 300);
        assert!(views(&drain(&mut rx)).is_empty());
        sized(&mut app, &id, 900, 500);
        assert_eq!(views(&drain(&mut rx)), [(960, 512, true)]);
    }

    #[test]
    fn a_picture_that_stops_being_shown_is_hidden_once() {
        let (mut app, mut rx) = in_call();
        let id = remote_camera(&mut app);
        sized(&mut app, &id, 500, 280);
        drain(&mut rx);
        // Folding the stage hides it.
        let _ = app.update(Message::Video(VideoMessage::ToggleStage));
        assert_eq!(views(&drain(&mut rx)), [(512, 320, false)]);
        let _ = app.update(Message::Video(VideoMessage::ToggleStage));
        assert_eq!(views(&drain(&mut rx)), [(512, 320, true)]);
        // Opening another room hides it too, and nothing more is said.
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
            value: RoomSummary::new(RoomId::from("!z:x"), "z".to_owned()),
        }])));
        let _ = app.update(Message::SelectRoom(RoomId::from("!z:x")));
        assert_eq!(views(&drain(&mut rx)), [(512, 320, false)]);
        let _ = app.update(Message::Video(VideoMessage::TileSized {
            track: id,
            place: Place::Tile,
            width: 500,
            height: 280,
        }));
        assert!(views(&drain(&mut rx)).is_empty());
    }

    #[test]
    fn a_track_nobody_shows_is_hidden_when_it_arrives() {
        let (mut app, mut rx) = in_call();
        // The stage is folded before the track comes.
        let _ = app.update(Message::Video(VideoMessage::ToggleStage));
        drain(&mut rx);
        let _ = remote_camera(&mut app);
        assert_eq!(views(&drain(&mut rx)), [(64, 64, false)]);
    }

    #[test]
    fn full_screen_asks_for_the_bigger_size_and_leaving_it_for_the_smaller() {
        let (mut app, mut rx) = in_call();
        let id = remote_camera(&mut app);
        sized(&mut app, &id, 500, 280);
        drain(&mut rx);
        let _ = app.update(Message::Video(VideoMessage::Fullscreen(id.clone())));
        let _ = app.update(Message::Video(VideoMessage::TileSized {
            track: id.clone(),
            place: Place::Fullscreen,
            width: 1920,
            height: 1080,
        }));
        assert_eq!(views(&drain(&mut rx)), [(1920, 1088, true)]);
        let _ = app.update(Message::Video(VideoMessage::ExitFullscreen));
        assert_eq!(views(&drain(&mut rx)), [(512, 320, true)]);
    }

    #[test]
    fn the_camera_button_starts_and_stops_the_camera_once_per_answer() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(Message::Video(VideoMessage::ToggleCamera));
        let _ = app.update(Message::Video(VideoMessage::ToggleCamera));
        let sent = drain(&mut rx);
        let starts: Vec<_> = sent
            .iter()
            .filter(|c| matches!(c, Command::StartCamera { .. }))
            .collect();
        assert_eq!(starts.len(), 1, "{sent:?}");
        // The core answers: the camera is on; the next press stops it.
        let _ = app.update(core(CoreEvent::LocalVideo(LocalVideo {
            camera: Some(VideoTrackId::from("local-camera-1")),
            ..LocalVideo::default()
        })));
        let _ = app.update(Message::Video(VideoMessage::ToggleCamera));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::StopCamera))
        );
        // A refusal frees the button.
        let _ = app.update(core(CoreEvent::Error(CoreError::NotSupported(
            "cameras".into(),
        ))));
        assert!(!app.calls.video.camera_busy);
        assert!(app.calls.notice.is_some());
    }

    #[test]
    fn the_camera_does_nothing_outside_a_call() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(core(CoreEvent::CallState(CallState::Idle)));
        drain(&mut rx);
        let _ = app.update(Message::Video(VideoMessage::ToggleCamera));
        let _ = app.update(Message::Video(VideoMessage::ToggleShare));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn sharing_goes_through_the_picker() {
        use twicecat_core::api::ScreenSource;
        let (mut app, mut rx) = in_call();
        let _ = app.update(Message::Video(VideoMessage::ToggleShare));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::ListScreenSources))
        );
        assert!(app.calls.video.picker.is_some());
        // Start is refused until a screen is picked.
        let _ = app.update(Message::Video(VideoMessage::StartShare));
        assert!(drain(&mut rx).is_empty());
        let _ = app.update(core(CoreEvent::ScreenSources(ScreenSources {
            system_picker: false,
            sources: vec![ScreenSource {
                id: ScreenSourceId::from("monitor-1"),
                kind: ScreenSourceKind::Monitor,
                title: "Screen 1".into(),
                width: 1920,
                height: 1080,
                thumbnail: None,
            }],
        })));
        let _ = app.update(Message::Video(VideoMessage::PickSource(
            ScreenSourceId::from("monitor-1"),
        )));
        let _ = app.update(Message::Video(VideoMessage::ShareAudio(true)));
        let _ = app.update(Message::Video(VideoMessage::StartShare));
        let sent = drain(&mut rx);
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::StartScreenShare { source: Some(s), audio: true, .. } if s.as_str() == "monitor-1"
            )),
            "{sent:?}"
        );
        assert!(app.calls.video.picker.is_none() && app.calls.video.share_busy);
        // Sharing: the button now stops it.
        let _ = app.update(core(CoreEvent::LocalVideo(LocalVideo {
            screen: Some(VideoTrackId::from("local-screen-1")),
            screen_audio: true,
            ..LocalVideo::default()
        })));
        let _ = app.update(Message::Video(VideoMessage::ToggleShare));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::StopScreenShare))
        );
    }

    #[test]
    fn where_the_system_picks_the_share_starts_without_a_source() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(Message::Video(VideoMessage::ToggleShare));
        let _ = app.update(core(CoreEvent::ScreenSources(ScreenSources {
            system_picker: true,
            sources: Vec::new(),
        })));
        drain(&mut rx);
        let _ = app.update(Message::Video(VideoMessage::StartShare));
        assert!(drain(&mut rx).iter().any(|c| matches!(
            c,
            Command::StartScreenShare {
                source: None,
                audio: false,
                ..
            }
        )));
    }

    #[test]
    fn cancelling_the_picker_sends_nothing() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(Message::Video(VideoMessage::ToggleShare));
        drain(&mut rx);
        let _ = app.update(Message::Video(VideoMessage::CancelShare));
        assert!(app.calls.video.picker.is_none());
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_share_quality_is_saved_in_the_video_settings() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(Message::Video(VideoMessage::ShareQuality(
            VideoQuality::Hd1080p60,
        )));
        let sent = drain(&mut rx);
        assert!(sent.iter().any(|c| matches!(
            c,
            Command::UpdateVideoSettings(s) if s.screen_quality == VideoQuality::Hd1080p60
        )));
    }

    #[test]
    fn pinning_toggles_and_only_known_tracks_pin() {
        let (mut app, _rx) = in_call();
        let id = remote_camera(&mut app);
        let _ = app.update(Message::Video(VideoMessage::Pin(id.clone())));
        assert_eq!(app.calls.video.pinned, Some(id.clone()));
        let _ = app.update(Message::Video(VideoMessage::Pin(id)));
        assert_eq!(app.calls.video.pinned, None);
        let _ = app.update(Message::Video(VideoMessage::Pin(VideoTrackId::from(
            "nope",
        ))));
        assert_eq!(app.calls.video.pinned, None);
        let _ = app.update(Message::Video(VideoMessage::Fullscreen(
            VideoTrackId::from("nope"),
        )));
        assert_eq!(app.calls.video.fullscreen, None);
    }

    #[test]
    fn the_end_of_the_call_empties_the_stage() {
        let (mut app, _rx) = in_call();
        let _ = remote_camera(&mut app);
        assert!(app.stage_visible());
        let _ = app.update(core(CoreEvent::CallState(CallState::Left {
            room_id: RoomId::from("!a:x"),
            reason: twicecat_core::api::CallEndReason::Hangup,
        })));
        assert!(app.calls.video.tracks.is_empty() && !app.stage_visible());
    }

    #[test]
    fn a_video_join_asks_for_a_video_call_and_starts_the_camera_once_connected() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(core(CoreEvent::CallState(CallState::Idle)));
        drain(&mut rx);
        let _ = app.update(Message::Call(crate::calls::CallMessage::JoinWithVideo(
            RoomId::from("!a:x"),
        )));
        let sent = drain(&mut rx);
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::JoinCall {
                    intent: CallIntent::Video,
                    ..
                }
            )),
            "{sent:?}"
        );
        // Connected: the camera starts, once.
        let _ = app.update(core(CoreEvent::CallState(CallState::Joining {
            room_id: RoomId::from("!a:x"),
        })));
        drain(&mut rx);
        let _ = app.update(core(CoreEvent::CallState(CallState::Connected {
            room_id: RoomId::from("!a:x"),
            encrypted: true,
        })));
        let starts = drain(&mut rx)
            .iter()
            .filter(|c| matches!(c, Command::StartCamera { .. }))
            .count();
        assert_eq!(starts, 1);
        let _ = app.update(core(CoreEvent::CallState(CallState::Reconnecting {
            room_id: RoomId::from("!a:x"),
            attempt: 1,
        })));
        let _ = app.update(core(CoreEvent::CallState(CallState::Connected {
            room_id: RoomId::from("!a:x"),
            encrypted: true,
        })));
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::StartCamera { .. }))
        );
    }

    #[test]
    fn a_plain_join_keeps_the_camera_off_and_a_refused_video_join_is_forgotten() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(core(CoreEvent::CallState(CallState::Idle)));
        drain(&mut rx);
        let _ = app.update(Message::Call(crate::calls::CallMessage::Join(
            RoomId::from("!a:x"),
        )));
        assert!(drain(&mut rx).iter().any(|c| matches!(
            c,
            Command::JoinCall {
                intent: CallIntent::Audio,
                ..
            }
        )));
        // Refused: the wish for a camera goes with it.
        let _ = app.update(core(CoreEvent::CallState(CallState::Idle)));
        let _ = app.update(Message::Call(crate::calls::CallMessage::JoinWithVideo(
            RoomId::from("!a:x"),
        )));
        assert!(app.calls.camera_on_connect.is_some());
        let _ = app.update(core(CoreEvent::Error(CoreError::Call("no".into()))));
        assert!(app.calls.camera_on_connect.is_none());
        // A join that was not even sent (one is pending) asks for no camera.
        let _ = app.update(Message::Call(crate::calls::CallMessage::Join(
            RoomId::from("!a:x"),
        )));
        let _ = app.update(Message::Call(crate::calls::CallMessage::JoinWithVideo(
            RoomId::from("!a:x"),
        )));
        assert!(app.calls.camera_on_connect.is_none());
    }

    #[test]
    fn the_window_is_full_screen_exactly_while_the_full_screen_view_is_open() {
        let (mut app, _rx) = in_call();
        let id = remote_camera(&mut app);
        assert!(!app.calls.video.window_fullscreen);
        let _ = app.update(Message::Video(VideoMessage::Fullscreen(id.clone())));
        assert!(app.calls.video.window_fullscreen);
        let _ = app.update(Message::Video(VideoMessage::ExitFullscreen));
        assert!(!app.calls.video.window_fullscreen);
        // The track ending under the view closes it, and the window with it.
        let _ = app.update(Message::Video(VideoMessage::Fullscreen(id)));
        assert!(app.calls.video.window_fullscreen);
        let _ = app.update(core(CoreEvent::VideoTracks {
            room_id: RoomId::from("!a:x"),
            tracks: Vec::new(),
        }));
        assert!(app.calls.video.fullscreen.is_none());
        assert!(!app.calls.video.window_fullscreen);
    }

    #[test]
    fn esc_closes_the_picker_then_the_full_screen_view_and_never_starts_a_share() {
        let (mut app, mut rx) = in_call();
        let id = remote_camera(&mut app);
        let _ = app.update(Message::Video(VideoMessage::Fullscreen(id)));
        let _ = app.update(Message::Video(VideoMessage::ToggleShare));
        let _ = app.update(core(CoreEvent::ScreenSources(ScreenSources {
            system_picker: true,
            sources: Vec::new(),
        })));
        drain(&mut rx);
        let _ = app.on_escape(false);
        assert!(app.calls.video.picker.is_none());
        assert!(app.calls.video.fullscreen.is_some());
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::StartScreenShare { .. }))
        );
        let _ = app.on_escape(false);
        assert!(app.calls.video.fullscreen.is_none());
    }

    #[test]
    fn the_scale_of_the_window_is_kept_only_when_it_is_a_sane_number() {
        let (mut app, _rx) = in_call();
        let _ = app.update(Message::WindowScale(2.0));
        assert_eq!(app.window_scale, 2.0);
        for bad in [f32::NAN, 0.0, -1.0, 100.0, f32::INFINITY] {
            let _ = app.update(Message::WindowScale(bad));
            assert_eq!(app.window_scale, 2.0, "{bad}");
        }
    }

    #[test]
    fn what_the_core_changed_is_kept_until_the_next_echo_and_per_kind_until_the_track_ends() {
        let adjustment = |setting: &str| VideoAdjustment {
            setting: setting.to_owned(),
            asked: "900".into(),
            used: "120".into(),
        };
        let mut model = VideoModel::default();
        model.on_event(CoreEvent::VideoSettings(VideoSettings::default()));
        model.on_event(CoreEvent::VideoSettingsAdjusted(vec![adjustment(
            "screen.fps",
        )]));
        assert_eq!(model.adjusted.len(), 1);
        // A later echo is about a later change: the old report goes.
        model.on_event(CoreEvent::VideoSettings(VideoSettings::default()));
        assert!(model.adjusted.is_empty());
        model.on_event(CoreEvent::VideoSending {
            kind: VideoKind::Camera,
            changes: vec![adjustment("camera.size")],
        });
        model.on_event(CoreEvent::VideoSending {
            kind: VideoKind::Screen,
            changes: vec![adjustment("screen.codec")],
        });
        model.on_event(CoreEvent::LocalVideo(LocalVideo {
            screen: Some(VideoTrackId::from("local-screen-1")),
            ..LocalVideo::default()
        }));
        assert!(!model.sending.contains_key(&VideoKind::Camera));
        assert!(model.sending.contains_key(&VideoKind::Screen));
        // Reports are capped.
        let many = (0..100)
            .map(|i| adjustment(&format!("camera.layer{i}")))
            .collect();
        model.on_event(CoreEvent::VideoSettingsAdjusted(many));
        assert_eq!(model.adjusted.len(), MAX_ADJUSTMENTS);
        model.end_call();
        assert!(model.sending.is_empty());
    }

    #[test]
    fn the_stage_shows_only_in_the_calls_room() {
        let (mut app, _rx) = in_call();
        assert!(!app.stage_visible(), "no pictures yet");
        let _ = remote_camera(&mut app);
        assert!(app.stage_visible());
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
            value: RoomSummary::new(RoomId::from("!z:x"), "z".to_owned()),
        }])));
        let _ = app.update(Message::SelectRoom(RoomId::from("!z:x")));
        assert!(!app.stage_visible());
        let _ = CallIntent::Video;
    }
}
