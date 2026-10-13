//! The quality ladder: from a bitrate to the resolution and framerate of
//! each layer we send (plan Q12). Pure functions, so that every choice is
//! tested without hardware.
//!
//! A screen share keeps its resolution and drops framerate first (text must
//! stay sharp); a camera keeps its framerate and drops resolution first
//! (motion must stay smooth). The bitrates are the plan's starting values
//! for H.264 (tuned in Task 14); a size and rate the plan does not list is
//! estimated from the nearest listed ones. AV1 needs 0.6 × these.

/// What is sent: decides the degradation order and the bitrates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Content {
    /// A screen or window: resolution first.
    Screen,
    /// A camera: framerate first.
    Camera,
}

/// The codec a layer is sent in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// H.264 Constrained Baseline, the default (Q4).
    H264,
    /// AV1, opt-in where every receiver can decode it (Q4).
    Av1,
    /// VP8, libwebrtc's own encoder on the no-hardware path (Q3).
    Vp8,
}

/// One encoded layer: its picture size, framerate and bitrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layer {
    /// Width in pixels (even).
    pub width: u32,
    /// Height in pixels (even).
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
    /// Target bitrate, bits per second.
    pub bitrate_bps: u32,
}

/// The source's size and rate: no layer goes above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
}

/// Heights a layer is made at, largest first.
const HEIGHTS: [u32; 7] = [2160, 1440, 1080, 720, 540, 360, 180];
/// Framerates a layer is made at, highest first.
const RATES: [u32; 3] = [60, 30, 15];
/// The most layers the SFU forwards (livekit-server's limit).
pub const MAX_LAYERS: usize = 3;

/// The plan's H.264 starting bitrates (Q12), by content, height and fps.
const LISTED: [(Content, u32, u32, u32); 10] = [
    (Content::Screen, 2160, 60, 20_000_000),
    (Content::Screen, 1440, 60, 12_000_000),
    (Content::Screen, 1080, 60, 8_000_000),
    (Content::Screen, 1080, 30, 4_000_000),
    (Content::Screen, 540, 15, 800_000),
    (Content::Camera, 2160, 60, 25_000_000),
    (Content::Camera, 1440, 60, 12_000_000),
    (Content::Camera, 1080, 60, 8_000_000),
    (Content::Camera, 720, 30, 1_700_000),
    (Content::Camera, 360, 30, 450_000),
];

/// The bitrate a layer of `height` (16:9) at `fps` needs.
///
/// A listed value is used as is. Otherwise it scales from the listed value
/// of the same content nearest in pixel rate, by (pixel rate ratio)^0.75:
/// bits per pixel fall as the pixel rate grows, which the listed values
/// follow within 15 %.
pub fn bitrate_bps(content: Content, codec: Codec, height: u32, fps: u32) -> u32 {
    let h264 = LISTED
        .iter()
        .find(|(c, h, f, _)| *c == content && *h == height && *f == fps)
        .map(|(.., b)| f64::from(*b))
        .unwrap_or_else(|| {
            let rate = pixel_rate(height, fps);
            let (_, h, f, b) = LISTED
                .iter()
                .filter(|(c, ..)| *c == content)
                .min_by(|a, b| {
                    let d = |x: &(Content, u32, u32, u32)| (pixel_rate(x.1, x.2) / rate).ln().abs();
                    d(a).total_cmp(&d(b))
                })
                .expect("each content has listed values");
            f64::from(*b) * (rate / pixel_rate(*h, *f)).powf(0.75)
        });
    let factor = match codec {
        Codec::H264 => 1.0,
        Codec::Av1 => 0.6,
        // libwebrtc's VP8 needs about what H.264 does at these sizes.
        Codec::Vp8 => 1.0,
    };
    // Rounded to 10 kb/s.
    ((h264 * factor / 10_000.0).round() * 10_000.0) as u32
}

fn pixel_rate(height: u32, fps: u32) -> f64 {
    let width = f64::from(height) * 16.0 / 9.0;
    width * f64::from(height) * f64::from(fps)
}

/// `source` scaled to `height`, keeping its aspect, both sides even.
fn sized(source: Source, height: u32) -> (u32, u32) {
    let height = height.min(source.height) & !1;
    let width =
        (u64::from(source.width) * u64::from(height) / u64::from(source.height.max(1))) as u32 & !1;
    (width.max(2), height.max(2))
}

/// The candidate (height, fps) pairs for `source`, in the order `content`
/// gives up quality: a screen walks down the framerates at each height, a
/// camera walks down the heights at each framerate. Heights above the
/// source's are skipped; the source's own height is the first.
fn candidates(content: Content, source: Source) -> Vec<(u32, u32)> {
    let mut heights: Vec<u32> = HEIGHTS
        .iter()
        .copied()
        .filter(|h| *h < source.height)
        .collect();
    heights.insert(0, source.height);
    let rates: Vec<u32> = RATES
        .iter()
        .copied()
        .filter(|f| *f <= source.fps)
        .chain((source.fps < 15).then_some(source.fps))
        .collect();
    let mut out = Vec::new();
    match content {
        Content::Screen => {
            for h in &heights {
                for f in &rates {
                    out.push((*h, *f));
                }
            }
        }
        Content::Camera => {
            for f in &rates {
                for h in &heights {
                    out.push((*h, *f));
                }
            }
        }
    }
    out
}

/// The best single layer that fits `target_bps`: the first candidate in
/// `content`'s degradation order whose bitrate fits, or the smallest one
/// when none does.
pub fn best_layer(content: Content, codec: Codec, source: Source, target_bps: u32) -> Layer {
    let all = candidates(content, source);
    let pick = all
        .iter()
        .copied()
        .find(|(h, f)| bitrate_bps(content, codec, *h, *f) <= target_bps)
        .unwrap_or_else(|| {
            all.iter()
                .copied()
                .min_by_key(|(h, f)| bitrate_bps(content, codec, *h, *f))
                .expect("there is always the source itself")
        });
    let (width, height) = sized(source, pick.0);
    Layer {
        width,
        height,
        fps: pick.1,
        bitrate_bps: bitrate_bps(content, codec, pick.0, pick.1),
    }
}

/// The simulcast layers under `top`, top first, at most [`MAX_LAYERS`]: a
/// screen adds 1080p30 and 540p15, a camera 720p30 and 360p30 (Q12), each
/// only when smaller than the layer above it.
pub fn layers(content: Content, codec: Codec, source: Source, top: Layer) -> Vec<Layer> {
    let lower: [(u32, u32); 2] = match content {
        Content::Screen => [(1080, 30), (540, 15)],
        Content::Camera => [(720, 30), (360, 30)],
    };
    let mut out = vec![top];
    for (height, fps) in lower {
        let above = out[out.len() - 1];
        let fps = fps.min(source.fps);
        if height >= above.height || out.len() == MAX_LAYERS {
            continue;
        }
        let (width, height) = sized(source, height);
        out.push(Layer {
            width,
            height,
            fps,
            bitrate_bps: bitrate_bps(content, codec, height, fps),
        });
    }
    out
}

/// Shares a total `target_bps` (WebRTC's estimate) among `layers` (top
/// first): the smallest layers are served first, and a layer that cannot
/// get its bitrate is turned off (0) with every layer above it, as the SFU
/// pauses a layer a viewer cannot take. Whatever is left after the layers
/// that fit goes to the top layer that is on, up to 1.5 × its bitrate.
pub fn split(target_bps: u32, layers: &[Layer]) -> Vec<u32> {
    let mut out = vec![0; layers.len()];
    let mut left = target_bps;
    let mut top_on = None;
    for (i, layer) in layers.iter().enumerate().rev() {
        if layer.bitrate_bps > left {
            break;
        }
        out[i] = layer.bitrate_bps;
        left -= layer.bitrate_bps;
        top_on = Some(i);
    }
    if let Some(i) = top_on {
        out[i] += left.min(layers[i].bitrate_bps / 2);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const UHD60: Source = Source {
        width: 3840,
        height: 2160,
        fps: 60,
    };

    #[test]
    fn listed_bitrates_are_the_plans() {
        for (content, h, f, b) in LISTED {
            assert_eq!(
                bitrate_bps(content, Codec::H264, h, f),
                b,
                "{content:?} {h}p{f}"
            );
        }
        assert_eq!(
            bitrate_bps(Content::Screen, Codec::Av1, 2160, 60),
            12_000_000
        );
    }

    #[test]
    fn estimated_bitrates_grow_with_size_and_rate() {
        for content in [Content::Screen, Content::Camera] {
            for f in RATES {
                let rates: Vec<u32> = HEIGHTS
                    .iter()
                    .map(|h| bitrate_bps(content, Codec::H264, *h, f))
                    .collect();
                assert!(
                    rates.windows(2).all(|w| w[0] > w[1]),
                    "{content:?} {f}: {rates:?}"
                );
            }
            for h in HEIGHTS {
                let rates: Vec<u32> = RATES
                    .iter()
                    .map(|f| bitrate_bps(content, Codec::H264, h, *f))
                    .collect();
                assert!(
                    rates.windows(2).all(|w| w[0] > w[1]),
                    "{content:?} {h}: {rates:?}"
                );
            }
        }
        // An estimate between listed values: 2160p30 screen, under 2160p60.
        let uhd30 = bitrate_bps(Content::Screen, Codec::H264, 2160, 30);
        assert!((10_000_000..14_000_000).contains(&uhd30), "{uhd30}");
    }

    #[test]
    fn a_screen_keeps_its_resolution_and_drops_framerate_first() {
        let pick = |target| best_layer(Content::Screen, Codec::H264, UHD60, target);
        assert_eq!(
            pick(25_000_000),
            Layer {
                width: 3840,
                height: 2160,
                fps: 60,
                bitrate_bps: 20_000_000
            }
        );
        let l = pick(15_000_000);
        assert_eq!((l.height, l.fps), (2160, 30));
        let l = pick(8_000_000);
        assert_eq!((l.height, l.fps), (2160, 15));
        // Under 4K at 15 fps: the next size down, at its best rate that fits.
        let l = pick(5_000_000);
        assert_eq!((l.height, l.fps), (1440, 15));
        let l = pick(3_000_000);
        assert_eq!((l.width, l.height, l.fps), (1920, 1080, 15));
    }

    #[test]
    fn a_camera_keeps_its_framerate_and_drops_resolution_first() {
        let pick = |target| best_layer(Content::Camera, Codec::H264, UHD60, target);
        let l = pick(30_000_000);
        assert_eq!((l.height, l.fps), (2160, 60));
        let l = pick(12_000_000);
        assert_eq!((l.height, l.fps), (1440, 60));
        let l = pick(3_000_000);
        assert_eq!((l.height, l.fps), (720, 60));
        // Below the smallest size at 60 fps: 30 fps at the largest that fits.
        let small_60 = bitrate_bps(Content::Camera, Codec::H264, 180, 60);
        let l = pick(small_60 - 1);
        assert_eq!(l.fps, 30);
        // Nothing fits: the cheapest layer.
        let l = pick(1);
        assert_eq!((l.height, l.fps), (180, 15));
    }

    #[test]
    fn layers_never_exceed_the_source_and_keep_its_aspect() {
        let source = Source {
            width: 2560,
            height: 1600,
            fps: 30,
        };
        let l = best_layer(Content::Screen, Codec::H264, source, 100_000_000);
        assert_eq!((l.width, l.height, l.fps), (2560, 1600, 30));
        let l = best_layer(Content::Screen, Codec::H264, source, 4_000_000);
        assert_eq!((l.width, l.height, l.fps), (2304, 1440, 15));
        assert!(l.width.is_multiple_of(2) && l.height.is_multiple_of(2));
    }

    #[test]
    fn simulcast_layers_follow_the_plan() {
        let top = best_layer(Content::Screen, Codec::H264, UHD60, 20_000_000);
        let screen = layers(Content::Screen, Codec::H264, UHD60, top);
        let shape: Vec<_> = screen
            .iter()
            .map(|l| (l.height, l.fps, l.bitrate_bps))
            .collect();
        assert_eq!(
            shape,
            [
                (2160, 60, 20_000_000),
                (1080, 30, 4_000_000),
                (540, 15, 800_000)
            ]
        );

        let top = best_layer(Content::Camera, Codec::H264, UHD60, 25_000_000);
        let camera = layers(Content::Camera, Codec::H264, UHD60, top);
        let shape: Vec<_> = camera
            .iter()
            .map(|l| (l.height, l.fps, l.bitrate_bps))
            .collect();
        assert_eq!(
            shape,
            [
                (2160, 60, 25_000_000),
                (720, 30, 1_700_000),
                (360, 30, 450_000)
            ]
        );

        // A 720p30 camera only gets the 360p layer below it.
        let source = Source {
            width: 1280,
            height: 720,
            fps: 30,
        };
        let top = best_layer(Content::Camera, Codec::H264, source, 10_000_000);
        let shape: Vec<_> = layers(Content::Camera, Codec::H264, source, top)
            .iter()
            .map(|l| (l.width, l.height, l.fps))
            .collect();
        assert_eq!(shape, [(1280, 720, 30), (640, 360, 30)]);
    }

    #[test]
    fn split_serves_the_small_layers_first() {
        let top = best_layer(Content::Screen, Codec::H264, UHD60, 20_000_000);
        let all = layers(Content::Screen, Codec::H264, UHD60, top);
        assert_eq!(split(24_800_000, &all), [20_000_000, 4_000_000, 800_000]);
        // Extra goes to the top layer, up to half again.
        assert_eq!(split(40_000_000, &all), [30_000_000, 4_000_000, 800_000]);
        // The top does not fit: off, and the leftover lifts the next one.
        assert_eq!(split(6_000_000, &all), [0, 5_200_000, 800_000]);
        assert_eq!(split(900_000, &all), [0, 0, 900_000]);
        assert_eq!(split(100_000, &all), [0, 0, 0]);
    }
}
