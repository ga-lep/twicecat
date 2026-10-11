//! Room avatars: bounded decoding of untrusted thumbnails, the per-session
//! request bookkeeping, and the initials placeholder (plan Q11, Task 11).
//!
//! Avatar bytes come from other users through the homeserver, so they are
//! never handed to the renderer as they are. [`decode_avatar`] decodes them
//! here, with explicit limits on dimensions and memory, into plain pixels;
//! anything that fails shows the placeholder.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use iced::Color;
use iced::widget::image::Handle;
use image::{ImageFormat, ImageReader, Limits};
use twicecat_core::api::{AvatarId, AvatarImage, RoomId};

use crate::text_safety;

/// The widest or tallest avatar accepted, in pixels. The core asks for 96 px
/// thumbnails; this leaves room for servers that return larger ones.
pub const MAX_AVATAR_SIDE: u32 = 512;

/// The most memory the decoder may use for one avatar, in bytes. A
/// `MAX_AVATAR_SIDE` square in RGBA is 1 MiB; the rest is decoder scratch.
pub const MAX_DECODE_ALLOC: u64 = 8 * 1024 * 1024;

/// The most avatar IDs sent in one `FetchAvatars`. Rows beyond it stay
/// unrequested and are sent by a later list change or scroll.
pub const MAX_REQUEST_BATCH: usize = 128;

/// What the placeholder shows for an empty or letterless name.
pub const NEUTRAL_INITIALS: &str = "?";

/// Fills of the initials placeholder; white text reads on every one.
pub const PLACEHOLDER_COLORS: [Color; 8] = [
    Color::from_rgb(0.76, 0.22, 0.20),
    Color::from_rgb(0.80, 0.38, 0.05),
    Color::from_rgb(0.55, 0.45, 0.02),
    Color::from_rgb(0.12, 0.50, 0.25),
    Color::from_rgb(0.05, 0.47, 0.50),
    Color::from_rgb(0.20, 0.35, 0.75),
    Color::from_rgb(0.48, 0.30, 0.74),
    Color::from_rgb(0.72, 0.22, 0.52),
];

/// What is known about one avatar.
#[derive(Clone)]
pub enum AvatarState {
    /// Decoded and ready to draw.
    Ready(Handle),
    /// The core had no image, or the bytes failed validation. Not retried
    /// this session.
    Unavailable,
}

impl std::fmt::Debug for AvatarState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready(_) => f.write_str("Ready"),
            Self::Unavailable => f.write_str("Unavailable"),
        }
    }
}

/// The avatar bookkeeping of one session.
///
/// Every avatar ID is requested at most once per session: an ID that is in
/// flight, answered, or answered with `None` is never asked for again, so
/// scrolling cannot cause a retry storm against a failing homeserver.
#[derive(Default)]
pub struct Avatars {
    requested: HashSet<AvatarId>,
    states: HashMap<AvatarId, AvatarState>,
}

// Manual impl: counts only, no IDs (they are MXC URIs of other users).
impl std::fmt::Debug for Avatars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Avatars")
            .field("requested", &self.requested.len())
            .field("answered", &self.states.len())
            .finish()
    }
}

impl Avatars {
    /// Of `ids`, those not requested yet (each once, in order, at most
    /// [`MAX_REQUEST_BATCH`]), now marked as requested.
    pub fn take_unrequested(&mut self, ids: impl IntoIterator<Item = AvatarId>) -> Vec<AvatarId> {
        let mut fresh = Vec::new();
        for id in ids {
            if fresh.len() >= MAX_REQUEST_BATCH {
                break;
            }
            if self.requested.insert(id.clone()) {
                fresh.push(id);
            }
        }
        fresh
    }

    /// Stores the core's answer for `id`. An answer for an ID that was never
    /// requested is ignored. Bytes that fail [`decode_avatar`] count as
    /// unavailable.
    pub fn answer(&mut self, id: &AvatarId, image: Option<AvatarImage>) {
        if !self.requested.contains(id) {
            return;
        }
        let state = match image.as_ref().and_then(|i| decode_avatar(i.as_bytes())) {
            Some(handle) => AvatarState::Ready(handle),
            None => AvatarState::Unavailable,
        };
        self.states.insert(id.clone(), state);
    }

    /// What is known about `id`, if it was answered.
    pub fn state(&self, id: &AvatarId) -> Option<&AvatarState> {
        self.states.get(id)
    }

    /// How many IDs were requested this session.
    pub fn requested_len(&self) -> usize {
        self.requested.len()
    }

    /// Forgets everything (logout, expired session).
    pub fn clear(&mut self) {
        self.requested.clear();
        self.states.clear();
    }
}

/// Decodes untrusted avatar bytes into an image handle of plain RGBA pixels,
/// or `None` when the bytes are not a PNG, JPEG or WebP image, are corrupt,
/// or exceed [`MAX_AVATAR_SIDE`] or [`MAX_DECODE_ALLOC`]. The limits are
/// checked from the header, before the pixel buffer is allocated.
pub fn decode_avatar(bytes: &[u8]) -> Option<Handle> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format()?,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_AVATAR_SIDE);
    limits.max_image_height = Some(MAX_AVATAR_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    Some(Handle::from_rgba(width, height, rgba.into_raw()))
}

/// Up to two uppercase initials for the placeholder, from the first two words
/// of `name`, which must already be sanitised (see
/// [`crate::room_list::row_name`]). A leading `#`, `@` or `!` is skipped. A
/// word counts only if it starts with a letter or digit, so emoji and marks
/// never produce a half glyph; with no such word the result is
/// [`NEUTRAL_INITIALS`].
///
/// ```
/// use twicecat_app::avatars::initials;
/// assert_eq!(initials("alice smith"), "AS");
/// assert_eq!(initials("#room:server"), "R");
/// assert_eq!(initials(""), "?");
/// ```
pub fn initials(name: &str) -> String {
    let name = name.trim_start_matches(['#', '@', '!']);
    let out: String = name
        .split(|c: char| c.is_whitespace() || c == text_safety::LINE_BREAK_MARK)
        .filter_map(|word| {
            let first = word
                .trim_start_matches(text_safety::REPLACEMENT)
                .chars()
                .next()?;
            if !first.is_alphanumeric() {
                return None;
            }
            first.to_uppercase().next()
        })
        .take(2)
        .collect();
    if out.is_empty() {
        NEUTRAL_INITIALS.to_owned()
    } else {
        out
    }
}

/// The placeholder fill for a room: stable for the same ID, from
/// [`PLACEHOLDER_COLORS`].
pub fn placeholder_color(room_id: &RoomId) -> Color {
    // FNV-1a, so the choice does not depend on the standard hasher.
    let hash = room_id
        .as_str()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    let index = usize::try_from(hash % PLACEHOLDER_COLORS.len() as u64)
        .expect("an index below 8 fits in usize");
    PLACEHOLDER_COLORS[index]
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{ImageFormat, Rgba, RgbaImage};

    use super::*;

    fn encode(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let img = RgbaImage::from_pixel(width, height, Rgba([10, 200, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).expect("encode a test image");
        out.into_inner()
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A PNG whose IHDR claims `width` x `height` (CRC fixed up); the pixel
    /// data is that of a 1x1 image, so it is a header-only lie.
    fn png_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut png = encode(1, 1, ImageFormat::Png);
        // 8 bytes signature, 4 length, 4 "IHDR", then 13 bytes of data.
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    fn dims(handle: &Handle) -> (u32, u32) {
        match handle {
            Handle::Rgba { width, height, .. } => (*width, *height),
            other => panic!("expected decoded pixels, got {other:?}"),
        }
    }

    #[test]
    fn a_tiny_valid_png_decodes() {
        let handle = decode_avatar(&encode(2, 3, ImageFormat::Png)).expect("valid png");
        assert_eq!(dims(&handle), (2, 3));
    }

    #[test]
    fn a_96_pixel_thumbnail_and_the_limit_edge_decode() {
        assert!(decode_avatar(&encode(96, 96, ImageFormat::Png)).is_some());
        let edge = MAX_AVATAR_SIDE;
        assert!(decode_avatar(&encode(edge, 1, ImageFormat::Png)).is_some());
        assert!(decode_avatar(&encode(edge + 1, 1, ImageFormat::Png)).is_none());
    }

    #[test]
    fn jpeg_and_webp_decode_too() {
        let jpeg = RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(jpeg)
            .to_rgb8()
            .write_to(&mut out, ImageFormat::Jpeg)
            .expect("encode jpeg");
        assert!(decode_avatar(&out.into_inner()).is_some());
        assert!(decode_avatar(&encode(8, 8, ImageFormat::WebP)).is_some());
    }

    #[test]
    fn corrupt_and_empty_blobs_are_refused() {
        assert!(decode_avatar(&[]).is_none());
        assert!(decode_avatar(b"definitely not an image").is_none());
        let png = encode(4, 4, ImageFormat::Png);
        assert!(decode_avatar(&png[..png.len() / 2]).is_none(), "truncated");
        let mut flipped = png;
        let mid = flipped.len() / 2;
        for b in &mut flipped[mid..] {
            *b ^= 0xFF;
        }
        assert!(decode_avatar(&flipped).is_none(), "damaged");
    }

    #[test]
    fn a_header_claiming_huge_dimensions_is_refused_before_decoding() {
        // 60000 x 60000 RGBA would be 14 GB.
        assert!(decode_avatar(&png_claiming(60_000, 60_000)).is_none());
        assert!(decode_avatar(&png_claiming(u32::MAX, 1)).is_none());
        assert!(decode_avatar(&png_claiming(1, MAX_AVATAR_SIDE + 1)).is_none());
    }

    #[test]
    fn formats_other_than_png_jpeg_and_webp_are_refused() {
        assert!(decode_avatar(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").is_none());
        assert!(decode_avatar(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_none());
    }

    fn id(n: &str) -> AvatarId {
        AvatarId::from(format!("mxc://example.org/{n}"))
    }

    #[test]
    fn requests_are_deduplicated_within_and_across_calls() {
        let mut avatars = Avatars::default();
        let first = avatars.take_unrequested([id("a"), id("b"), id("a")]);
        assert_eq!(first, [id("a"), id("b")]);
        assert!(avatars.take_unrequested([id("a"), id("b")]).is_empty());
        assert_eq!(avatars.take_unrequested([id("b"), id("c")]), [id("c")]);
    }

    #[test]
    fn a_batch_is_capped_and_the_rest_stays_unrequested() {
        let mut avatars = Avatars::default();
        let many: Vec<_> = (0..MAX_REQUEST_BATCH + 5)
            .map(|n| id(&n.to_string()))
            .collect();
        let sent = avatars.take_unrequested(many.clone());
        assert_eq!(sent.len(), MAX_REQUEST_BATCH);
        let rest = avatars.take_unrequested(many);
        assert_eq!(rest.len(), 5);
    }

    #[test]
    fn an_image_answer_stores_pixels_and_none_stores_unavailable() {
        let mut avatars = Avatars::default();
        let _ = avatars.take_unrequested([id("a"), id("b"), id("c")]);
        avatars.answer(
            &id("a"),
            Some(AvatarImage::from(encode(2, 2, ImageFormat::Png))),
        );
        avatars.answer(&id("b"), None);
        avatars.answer(&id("c"), Some(AvatarImage::from(b"junk".to_vec())));
        assert!(matches!(
            avatars.state(&id("a")),
            Some(AvatarState::Ready(_))
        ));
        assert!(matches!(
            avatars.state(&id("b")),
            Some(AvatarState::Unavailable)
        ));
        assert!(matches!(
            avatars.state(&id("c")),
            Some(AvatarState::Unavailable)
        ));
        assert!(avatars.state(&id("never")).is_none());
    }

    #[test]
    fn answered_and_failed_ids_are_never_requested_again() {
        let mut avatars = Avatars::default();
        let _ = avatars.take_unrequested([id("a"), id("b")]);
        avatars.answer(&id("a"), None);
        assert!(avatars.take_unrequested([id("a"), id("b")]).is_empty());
    }

    #[test]
    fn an_unsolicited_answer_is_ignored() {
        let mut avatars = Avatars::default();
        avatars.answer(
            &id("x"),
            Some(AvatarImage::from(encode(2, 2, ImageFormat::Png))),
        );
        assert!(avatars.state(&id("x")).is_none());
        assert_eq!(avatars.take_unrequested([id("x")]), [id("x")]);
    }

    #[test]
    fn clear_forgets_requests_and_images() {
        let mut avatars = Avatars::default();
        let _ = avatars.take_unrequested([id("a")]);
        avatars.answer(&id("a"), None);
        avatars.clear();
        assert!(avatars.state(&id("a")).is_none());
        assert_eq!(avatars.take_unrequested([id("a")]), [id("a")]);
    }

    #[test]
    fn debug_does_not_dump_pixels() {
        let mut avatars = Avatars::default();
        let _ = avatars.take_unrequested([id("secret-looking")]);
        avatars.answer(
            &id("secret-looking"),
            Some(AvatarImage::from(encode(2, 2, ImageFormat::Png))),
        );
        let text = format!("{avatars:?}");
        assert!(!text.contains("secret-looking"), "{text}");
    }

    #[test]
    fn initials_of_plain_names() {
        assert_eq!(initials("Alice Smith"), "AS");
        assert_eq!(initials("alice smith jones"), "AS");
        assert_eq!(initials("  alice   smith "), "AS");
        assert_eq!(initials("Alice"), "A");
        assert_eq!(initials("alice"), "A");
    }

    #[test]
    fn initials_skip_a_leading_sigil() {
        assert_eq!(initials("#room:server"), "R");
        assert_eq!(initials("@user:server"), "U");
        assert_eq!(initials("!abc:server"), "A");
        assert_eq!(initials("#dev team"), "DT");
        assert_eq!(initials("##x"), "X");
    }

    #[test]
    fn initials_are_unicode_safe() {
        assert_eq!(initials("élodie ñandú"), "ÉÑ");
        assert_eq!(initials("Привет мир"), "ПМ");
        assert_eq!(initials("你好 世界"), "你世");
        assert_eq!(initials("ßeta"), "S");
        // An emoji start does not make a letter; the next word still does.
        assert_eq!(initials("\u{1F389} Party"), "P");
        assert_eq!(initials("\u{1F468}\u{200D}\u{1F4BB} dev"), "D");
    }

    #[test]
    fn blank_and_symbol_only_names_get_the_neutral_placeholder() {
        for name in [
            "",
            "   ",
            "#",
            "@!",
            "\u{1F389}",
            "\u{FFFD}\u{FFFD}",
            "\u{23CE}",
        ] {
            assert_eq!(initials(name), NEUTRAL_INITIALS, "{name:?}");
        }
    }

    #[test]
    fn initials_of_a_sanitised_hostile_name() {
        let name = crate::room_list::row_name("\u{202E}evil\nname");
        // The sanitiser's marks are not letters: they never become initials.
        assert_eq!(initials(&name), "EN");
        assert_eq!(initials(&crate::room_list::row_name("a\r\nb")), "AB");
    }

    #[test]
    fn a_huge_name_is_cheap_and_bounded() {
        let name = "x ".repeat(100_000);
        assert_eq!(initials(&name), "XX");
    }

    #[test]
    fn placeholder_colour_is_stable_and_from_the_palette() {
        let a = RoomId::from("!a:x");
        assert_eq!(placeholder_color(&a), placeholder_color(&a.clone()));
        assert!(PLACEHOLDER_COLORS.contains(&placeholder_color(&a)));
        let distinct: std::collections::HashSet<_> = (0..200)
            .map(|n| {
                let c = placeholder_color(&RoomId::from(format!("!room{n}:x")));
                assert!(PLACEHOLDER_COLORS.contains(&c));
                PLACEHOLDER_COLORS.iter().position(|p| *p == c)
            })
            .collect();
        assert!(distinct.len() > 1, "all rooms got one colour");
    }
}
