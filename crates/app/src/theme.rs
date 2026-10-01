//! Light and dark palettes that follow the system setting (plan Q13).
//!
//! The mode comes from `iced::system::theme()` at boot and from
//! `iced::system::theme_changes()`; `Mode::None` (unknown) maps to light.
//! `ICED_THEME` only overrides iced's own default theme, which the app does
//! not use, so it has no effect here.

use iced::theme::{Mode, Palette};
use iced::{Color, Theme, color};

/// Palette of the light theme.
pub const LIGHT: Palette = Palette {
    background: color!(0xFFFFFF),
    text: color!(0x1F2328),
    primary: color!(0x3B4CCA),
    success: color!(0x1A7F37),
    warning: color!(0x9A6700),
    danger: color!(0xCF222E),
};

/// Palette of the dark theme.
pub const DARK: Palette = Palette {
    background: color!(0x1E1F22),
    text: color!(0xE6E6E6),
    primary: color!(0x8C99F7),
    success: color!(0x3FB950),
    warning: color!(0xD29922),
    danger: color!(0xF85149),
};

/// Name of the light theme.
pub const LIGHT_NAME: &str = "Tessera Light";
/// Name of the dark theme.
pub const DARK_NAME: &str = "Tessera Dark";

/// Colours the screens use beyond the stock palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Semantic {
    /// Background of an unread-count badge.
    pub badge: Color,
    /// Text on [`Semantic::badge`].
    pub badge_text: Color,
    /// Background of a highlight (mention) badge.
    pub highlight: Color,
    /// Text on [`Semantic::highlight`].
    pub highlight_text: Color,
    /// Secondary text, such as previews and timestamps.
    pub muted: Color,
    /// Background of the selected row.
    pub selection: Color,
}

/// Semantic colours of the light theme.
pub const LIGHT_SEMANTIC: Semantic = Semantic {
    badge: color!(0x57606A),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xCF222E),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0x59636E),
    selection: color!(0xDDE1FA),
};

/// Semantic colours of the dark theme.
pub const DARK_SEMANTIC: Semantic = Semantic {
    badge: color!(0x4E5058),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xB62324),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0xA0A4AB),
    selection: color!(0x35385A),
};

/// The theme for a system mode. `Mode::None` is light.
pub fn theme_for(mode: Mode) -> Theme {
    match mode {
        Mode::Dark => Theme::custom(DARK_NAME, DARK),
        Mode::Light | Mode::None => Theme::custom(LIGHT_NAME, LIGHT),
    }
}

/// The semantic colours that go with `theme`.
pub fn semantic(theme: &Theme) -> Semantic {
    if theme.extended_palette().is_dark {
        DARK_SEMANTIC
    } else {
        LIGHT_SEMANTIC
    }
}

/// WCAG 2.1 contrast ratio of two opaque colours, from 1.0 to 21.0.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn luminance(color: Color) -> f32 {
    let lin = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(color.r) + 0.7152 * lin(color.g) + 0.0722 * lin(color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_for_names_and_darkness() {
        let cases = [
            (Mode::Dark, DARK_NAME, true),
            (Mode::Light, LIGHT_NAME, false),
            (Mode::None, LIGHT_NAME, false),
        ];
        for (mode, name, dark) in cases {
            let theme = theme_for(mode);
            assert_eq!(theme.to_string(), name, "{mode:?}");
            assert_eq!(theme.extended_palette().is_dark, dark, "{mode:?}");
        }
    }

    #[test]
    fn contrast_matches_the_wcag_reference_values() {
        assert!((contrast(Color::BLACK, Color::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color::WHITE, Color::WHITE) - 1.0).abs() < 0.001);
        // Symmetric.
        assert_eq!(
            contrast(LIGHT.text, LIGHT.background),
            contrast(LIGHT.background, LIGHT.text)
        );
        // #767676 on white is the well-known 4.54:1 boundary grey.
        let grey = color!(0x767676);
        assert!((contrast(grey, Color::WHITE) - 4.54).abs() < 0.01);
    }

    #[test]
    fn colours_are_readable_in_both_palettes() {
        for (name, palette, sem) in [
            ("light", LIGHT, LIGHT_SEMANTIC),
            ("dark", DARK, DARK_SEMANTIC),
        ] {
            let checks = [
                ("text/background", palette.text, palette.background),
                ("badge text/badge", sem.badge_text, sem.badge),
                (
                    "highlight text/highlight",
                    sem.highlight_text,
                    sem.highlight,
                ),
                ("muted/background", sem.muted, palette.background),
                ("text/selection", palette.text, sem.selection),
            ];
            for (what, fg, bg) in checks {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{name} {what}: {ratio}");
            }
        }
    }

    #[test]
    fn semantic_colours_follow_the_theme() {
        assert_eq!(semantic(&theme_for(Mode::Dark)), DARK_SEMANTIC);
        assert_eq!(semantic(&theme_for(Mode::Light)), LIGHT_SEMANTIC);
    }
}
