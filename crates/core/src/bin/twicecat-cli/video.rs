//! Video commands of `twicecat-cli`: send the core's test pattern as the
//! camera or a screen share, list the call's video tracks, pick their
//! layers, print their statistics and the encoders this machine has. A
//! terminal shows no pictures, so received frames are only counted by the
//! core's statistics.

use twicecat_core::api::{
    Command, CoreEvent, LocalVideo, VideoDirection, VideoEncoders, VideoKind, VideoQuality,
    VideoStats, VideoTrackId, VideoTrackView,
};

use super::sanitize;

pub(crate) const USAGE: &str = "/video | /video pattern <720p|1080p|1440p|2160p> <30|60> [screen] \
     | /video off [screen] | /video view <n> <width>x<height>|hide | /video stats \
     | /video encoders";

/// A parsed `/video` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum VideoCmd {
    /// List the call's video tracks.
    List,
    /// Send the test pattern as our camera or screen share.
    Pattern {
        kind: VideoKind,
        quality: VideoQuality,
    },
    /// Stop sending our camera or screen share.
    Off(VideoKind),
    /// Show track `n` (from `/video`) at this size, or hide it.
    View { n: usize, size: Option<(u32, u32)> },
    /// Print the next statistics.
    Stats,
    /// List the encoders this machine has.
    Encoders,
}

/// The quality of a size and rate, as the core names them.
fn quality(res: &str, fps: &str) -> Option<VideoQuality> {
    match (res, fps) {
        ("720p", "30") => Some(VideoQuality::Hd720p30),
        ("1080p", "60") => Some(VideoQuality::Hd1080p60),
        ("1440p", "60") => Some(VideoQuality::Qhd1440p60),
        ("2160p" | "4k", "30") => Some(VideoQuality::Uhd2160p30),
        ("2160p" | "4k", "60") => Some(VideoQuality::Uhd2160p60),
        _ => None,
    }
}

fn kind(word: Option<&str>) -> Option<VideoKind> {
    match word {
        None => Some(VideoKind::Camera),
        Some("screen") => Some(VideoKind::Screen),
        Some(_) => None,
    }
}

/// Parses the arguments of `/video`.
pub(crate) fn parse(arg: &str) -> Result<VideoCmd, &'static str> {
    let words: Vec<&str> = arg.split_whitespace().collect();
    match words.as_slice() {
        [] => Ok(VideoCmd::List),
        ["stats"] => Ok(VideoCmd::Stats),
        ["encoders"] => Ok(VideoCmd::Encoders),
        ["pattern", res, fps, rest @ ..] if rest.len() <= 1 => Ok(VideoCmd::Pattern {
            kind: kind(rest.first().copied()).ok_or(USAGE)?,
            quality: quality(res, fps).ok_or(USAGE)?,
        }),
        ["off", rest @ ..] if rest.len() <= 1 => {
            Ok(VideoCmd::Off(kind(rest.first().copied()).ok_or(USAGE)?))
        }
        ["view", n, "hide"] => Ok(VideoCmd::View {
            n: n.parse().map_err(|_| USAGE)?,
            size: None,
        }),
        ["view", n, size] => {
            let (w, h) = size.split_once('x').ok_or(USAGE)?;
            Ok(VideoCmd::View {
                n: n.parse().map_err(|_| USAGE)?,
                size: Some((w.parse().map_err(|_| USAGE)?, h.parse().map_err(|_| USAGE)?)),
            })
        }
        _ => Err(USAGE),
    }
}

/// What the CLI knows about the call's video.
#[derive(Default)]
pub(crate) struct Video {
    tracks: Vec<VideoTrackView>,
    show_stats: bool,
}

fn kind_name(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => "camera",
        VideoKind::Screen => "screen share",
    }
}

fn track_line(n: usize, t: &VideoTrackView) -> String {
    let who = if t.is_local {
        "you".to_owned()
    } else {
        sanitize(t.user_id.as_str())
    };
    let mut line = format!(
        "  {n}. {who} ({}) {}",
        sanitize(t.device_id.as_str()),
        kind_name(t.kind)
    );
    if t.width > 0 {
        line.push_str(&format!(" {}x{}", t.width, t.height));
    }
    if let Some(fps) = t.fps {
        line.push_str(&format!(" {fps} fps"));
    }
    if let Some(codec) = t.codec {
        line.push_str(&format!(" {codec:?}"));
    }
    if t.muted {
        line.push_str(" (off)");
    }
    line
}

impl Video {
    /// Handles one command: the commands to send and the lines to print.
    pub(crate) fn command(&mut self, cmd: VideoCmd) -> (Vec<Command>, Vec<String>) {
        match cmd {
            VideoCmd::List if self.tracks.is_empty() => {
                (Vec::new(), vec!["No video in the call.".into()])
            }
            VideoCmd::List => (
                Vec::new(),
                self.tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| track_line(i + 1, t))
                    .collect(),
            ),
            VideoCmd::Pattern { kind, quality } => {
                let command = match kind {
                    VideoKind::Camera => Command::StartCamera {
                        device: None,
                        quality,
                    },
                    VideoKind::Screen => Command::StartScreenShare {
                        source: None,
                        quality,
                        audio: false,
                    },
                };
                (vec![command], Vec::new())
            }
            VideoCmd::Off(VideoKind::Camera) => (vec![Command::StopCamera], Vec::new()),
            VideoCmd::Off(VideoKind::Screen) => (vec![Command::StopScreenShare], Vec::new()),
            VideoCmd::View { n, size } => match n.checked_sub(1).and_then(|i| self.tracks.get(i)) {
                Some(t) if !t.is_local => {
                    let (width, height) = size.unwrap_or((0, 0));
                    (
                        vec![Command::SetVideoView {
                            track: t.id.clone(),
                            width,
                            height,
                            visible: size.is_some(),
                        }],
                        Vec::new(),
                    )
                }
                Some(_) => (Vec::new(), vec!["That is your own video.".into()]),
                None => (
                    Vec::new(),
                    vec!["No such video track. Use /video to list them.".into()],
                ),
            },
            VideoCmd::Stats => {
                self.show_stats = true;
                (Vec::new(), Vec::new())
            }
            VideoCmd::Encoders => (vec![Command::ProbeVideoEncoders], Vec::new()),
        }
    }

    /// Handles a video event: the lines to print.
    pub(crate) fn event(&mut self, event: CoreEvent) -> Vec<String> {
        match event {
            CoreEvent::VideoTracks { tracks, .. } => {
                let mut lines = Vec::new();
                for t in tracks.iter().filter(|t| !self.has(&t.id)) {
                    lines.push(format!(
                        "[video] + {} {}",
                        if t.is_local {
                            "your".to_owned()
                        } else {
                            sanitize(t.user_id.as_str())
                        },
                        kind_name(t.kind)
                    ));
                }
                for t in self
                    .tracks
                    .iter()
                    .filter(|t| !tracks.iter().any(|n| n.id == t.id))
                {
                    lines.push(format!(
                        "[video] - {} {}",
                        if t.is_local {
                            "your".to_owned()
                        } else {
                            sanitize(t.user_id.as_str())
                        },
                        kind_name(t.kind)
                    ));
                }
                self.tracks = tracks;
                lines
            }
            CoreEvent::LocalVideo(LocalVideo { camera, screen, .. }) => vec![format!(
                "[video] camera {}, screen share {}",
                if camera.is_some() { "on" } else { "off" },
                if screen.is_some() { "on" } else { "off" },
            )],
            CoreEvent::VideoStats(stats) if self.show_stats => {
                self.show_stats = false;
                self.stats_lines(&stats)
            }
            CoreEvent::VideoEncoders(encoders) => encoder_lines(&encoders),
            _ => Vec::new(),
        }
    }

    fn has(&self, id: &VideoTrackId) -> bool {
        self.tracks.iter().any(|t| t.id == *id)
    }

    fn stats_lines(&self, stats: &VideoStats) -> Vec<String> {
        stats
            .tracks
            .iter()
            .map(|s| {
                let n = self
                    .tracks
                    .iter()
                    .position(|t| t.id == s.track)
                    .map_or_else(|| "?".to_owned(), |i| (i + 1).to_string());
                let way = match s.direction {
                    VideoDirection::Sent => "sent",
                    VideoDirection::Received => "received",
                };
                let mut line = format!(
                    "[video] {n}. {way} {}x{} {:.1} fps {} kb/s",
                    s.width, s.height, s.fps, s.bitrate_kbps
                );
                if let Some(ms) = s.frame_ms {
                    line.push_str(&format!(", {ms:.1} ms a frame"));
                }
                if s.direction == VideoDirection::Sent {
                    line.push_str(&format!(", limited by {:?}", s.limitation));
                } else {
                    line.push_str(&format!(", {} freezes", s.freezes));
                }
                if let Some(name) = &s.implementation {
                    let place = if s.hardware { "hardware" } else { "software" };
                    line.push_str(&format!(", {} ({place})", sanitize(name)));
                }
                line
            })
            .collect()
    }
}

/// The encoders, best first, and the one in use.
fn encoder_lines(encoders: &VideoEncoders) -> Vec<String> {
    let mut lines: Vec<String> = encoders
        .encoders
        .iter()
        .map(|e| {
            let mut line = format!("[video] encoder {:?}", e.backend);
            if let Some(gpu) = &e.gpu {
                line.push_str(&format!(" on {}", sanitize(gpu)));
            }
            line.push_str(if e.hardware {
                " (hardware):"
            } else {
                " (software):"
            });
            for c in &e.codecs {
                line.push_str(&format!(
                    " {:?} {}x{}@{}",
                    c.codec, c.max_width, c.max_height, c.max_fps
                ));
            }
            line
        })
        .collect();
    if let Some(backend) = encoders.in_use {
        lines.push(format!("[video] sending with {backend:?}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{DeviceId, ParticipantId, UserId, VideoTrackStats};

    use super::*;

    fn track(id: &str, local: bool) -> VideoTrackView {
        VideoTrackView {
            id: VideoTrackId::from(id),
            participant: ParticipantId::from("p"),
            user_id: UserId::from("@bob:x"),
            device_id: DeviceId::from("DEV"),
            is_local: local,
            kind: VideoKind::Camera,
            width: 640,
            height: 360,
            fps: None,
            codec: None,
            muted: false,
        }
    }

    #[test]
    fn video_commands_parse() {
        assert_eq!(parse(""), Ok(VideoCmd::List));
        assert_eq!(
            parse("pattern 720p 30"),
            Ok(VideoCmd::Pattern {
                kind: VideoKind::Camera,
                quality: VideoQuality::Hd720p30
            })
        );
        assert_eq!(
            parse("pattern 4k 60 screen"),
            Ok(VideoCmd::Pattern {
                kind: VideoKind::Screen,
                quality: VideoQuality::Uhd2160p60
            })
        );
        assert_eq!(parse("pattern 720p 60"), Err(USAGE), "no such quality");
        assert_eq!(parse("off screen"), Ok(VideoCmd::Off(VideoKind::Screen)));
        assert_eq!(
            parse("view 2 1280x720"),
            Ok(VideoCmd::View {
                n: 2,
                size: Some((1280, 720))
            })
        );
        assert_eq!(
            parse("view 2 hide"),
            Ok(VideoCmd::View { n: 2, size: None })
        );
        assert_eq!(parse("view 2 big"), Err(USAGE));
        assert_eq!(parse("stats"), Ok(VideoCmd::Stats));
        assert_eq!(parse("encoders"), Ok(VideoCmd::Encoders));
        assert_eq!(parse("off camera"), Err(USAGE));
    }

    #[test]
    fn tracks_are_listed_and_viewed_by_number() {
        let mut video = Video::default();
        let lines = video.event(CoreEvent::VideoTracks {
            room_id: "!r:x".into(),
            tracks: vec![track("mine", true), track("theirs", false)],
        });
        assert_eq!(lines, ["[video] + your camera", "[video] + @bob:x camera"]);
        let (_, listed) = video.command(VideoCmd::List);
        assert_eq!(listed[1], "  2. @bob:x (DEV) camera 640x360");
        let (sent, _) = video.command(VideoCmd::View {
            n: 2,
            size: Some((320, 180)),
        });
        assert!(matches!(
            sent.as_slice(),
            [Command::SetVideoView {
                width: 320,
                height: 180,
                visible: true,
                ..
            }]
        ));
        let (sent, lines) = video.command(VideoCmd::View { n: 1, size: None });
        assert!(sent.is_empty() && lines == ["That is your own video."]);
        let gone = video.event(CoreEvent::VideoTracks {
            room_id: "!r:x".into(),
            tracks: vec![track("mine", true)],
        });
        assert_eq!(gone, ["[video] - @bob:x camera"]);
    }

    #[test]
    fn stats_print_once_when_asked() {
        let mut video = Video::default();
        let stats = || {
            CoreEvent::VideoStats(VideoStats {
                tracks: vec![VideoTrackStats {
                    track: VideoTrackId::from("x"),
                    direction: VideoDirection::Received,
                    width: 1280,
                    height: 720,
                    fps: 29.5,
                    bitrate_kbps: 1500,
                    frame_ms: Some(2.25),
                    limitation: Default::default(),
                    freezes: 0,
                    implementation: Some("NVIDIA H264 Decoder".into()),
                    hardware: true,
                }],
            })
        };
        assert!(video.event(stats()).is_empty());
        video.command(VideoCmd::Stats);
        assert_eq!(
            video.event(stats()),
            [
                "[video] ?. received 1280x720 29.5 fps 1500 kb/s, 2.2 ms a frame, 0 freezes, \
              NVIDIA H264 Decoder (hardware)"
            ]
        );
        assert!(video.event(stats()).is_empty());
    }
}
