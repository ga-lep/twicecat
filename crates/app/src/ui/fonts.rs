//! The bundled typeface: Inter 4.1 (SIL OFL 1.1, see `THIRD-PARTY-NOTICES.md`).
//!
//! Four static faces are embedded. Text uses the family through [`BODY`] and
//! its weight and style variants; monospace stays the system's face. Glyphs
//! Inter lacks (emoji, CJK) fall back to system fonts.

use iced::Font;
use iced::font::{Style, Weight};

/// Inter Regular.
pub const REGULAR: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-Regular.ttf");
/// Inter Italic.
pub const ITALIC: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-Italic.ttf");
/// Inter SemiBold.
pub const SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-SemiBold.ttf");
/// Inter Bold.
pub const BOLD: &[u8] = include_bytes!("../../assets/fonts/inter/Inter-Bold.ttf");

/// Every bundled face; `main` loads exactly these.
pub const ALL: [&[u8]; 4] = [REGULAR, ITALIC, SEMIBOLD, BOLD];

/// The body font: Inter, regular weight.
pub const BODY: Font = Font::with_name("Inter");
/// Inter SemiBold.
pub const BODY_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..BODY
};
/// Inter Bold.
pub const BODY_BOLD: Font = Font {
    weight: Weight::Bold,
    ..BODY
};
/// Inter Italic.
pub const BODY_ITALIC: Font = Font {
    style: Style::Italic,
    ..BODY
};
/// The system monospace font.
pub const MONO: Font = Font::MONOSPACE;

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::graphics::text::{Paragraph, font_system};
    use iced::advanced::text::{self, Paragraph as _};
    use iced::{Font, Pixels, Size};

    #[test]
    fn every_face_is_truetype_and_has_its_size() {
        let sizes = [411_640, 417_388, 419_744, 420_428];
        assert_eq!(ALL.len(), 4);
        for (bytes, size) in ALL.iter().zip(sizes) {
            assert_eq!(bytes.len(), size);
            assert_eq!(bytes[..4], [0x00, 0x01, 0x00, 0x00]);
        }
        assert_eq!(ALL, [REGULAR, ITALIC, SEMIBOLD, BOLD]);
    }

    #[test]
    fn main_loads_exactly_all() {
        let main = include_str!("../main.rs");
        assert!(main.contains("ui::fonts::ALL"));
        assert!(main.contains(".default_font(ui::fonts::BODY)"));
        assert!(!main.contains("include_bytes!"));
    }

    #[test]
    fn no_default_font_outside_tests() {
        let needle = format!("{}::{}", "Font", "DEFAULT");
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("read src") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let source = std::fs::read_to_string(&path).expect("read file");
                    let production = source.split("#[cfg(test)]").next().unwrap_or_default();
                    assert!(!production.contains(&needle), "{} uses it", path.display());
                }
            }
        }
    }

    #[test]
    fn families_use_inter() {
        assert_eq!(BODY.family, iced::font::Family::Name("Inter"));
        assert_eq!(BODY_BOLD.family, BODY.family);
        assert_eq!(BODY_BOLD.weight, iced::font::Weight::Bold);
        assert_eq!(BODY_SEMIBOLD.weight, iced::font::Weight::Semibold);
        assert_eq!(BODY_ITALIC.style, iced::font::Style::Italic);
        assert_eq!(MONO, Font::MONOSPACE);
    }

    fn width(font: Font) -> f32 {
        let paragraph = Paragraph::with_text(text::Text {
            content: "Twicecat",
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size: Pixels(16.0),
            line_height: text::LineHeight::default(),
            font,
            align_x: text::Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: text::Shaping::Basic,
            wrapping: text::Wrapping::None,
        });
        paragraph.min_bounds().width
    }

    #[test]
    fn bold_face_is_found() {
        {
            let mut system = font_system().write().expect("font system");
            for face in ALL {
                system.load_font(std::borrow::Cow::Borrowed(face));
            }
        }
        let regular = width(BODY);
        let bold = width(BODY_BOLD);
        assert_ne!(regular, bold);
        assert_ne!(regular, width(Font::DEFAULT));
        assert_ne!(bold, width(Font::DEFAULT));
    }
}
