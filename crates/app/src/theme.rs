//! Light and dark palettes that follow the system setting (plan Q13).
//!
//! The mode comes from `iced::system::theme()` at boot and from
//! `iced::system::theme_changes()`; `Mode::None` (unknown) maps to light.
//! `ICED_THEME` only overrides iced's own default theme, which the app does
//! not use, so it has no effect here.

use iced::theme::{Mode, Palette};
use iced::{Color, Theme, color};
use tessera_core::api::ThemeChoice;

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
    /// Background of a timeline message that mentions the user.
    pub message_highlight: Color,
    /// Background of the timeline message under the pointer.
    pub message_hover: Color,
}

/// Semantic colours of the light theme.
pub const LIGHT_SEMANTIC: Semantic = Semantic {
    badge: color!(0x57606A),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xCF222E),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0x59636E),
    selection: color!(0xDDE1FA),
    message_highlight: color!(0xFFF1B8),
    message_hover: color!(0xF3F4F6),
};

/// Semantic colours of the dark theme.
pub const DARK_SEMANTIC: Semantic = Semantic {
    badge: color!(0x4E5058),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xB62324),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0xA0A4AB),
    selection: color!(0x35385A),
    message_highlight: color!(0x3D3519),
    message_hover: color!(0x2B2D31),
};

/// The theme for a system mode. `Mode::None` is light.
pub fn theme_for(mode: Mode) -> Theme {
    match mode {
        Mode::Dark => Theme::custom(DARK_NAME, DARK),
        Mode::Light | Mode::None => Theme::custom(LIGHT_NAME, LIGHT),
    }
}

/// The mode the window uses: the user's override, else the system's.
pub fn resolve_mode(system: Mode, choice: ThemeChoice) -> Mode {
    match choice {
        ThemeChoice::System => system,
        ThemeChoice::Light => Mode::Light,
        ThemeChoice::Dark => Mode::Dark,
    }
}

/// The theme for the system mode and the user's choice (plan Q17): an
/// override wins over the system, `System` follows it.
pub fn theme_for_choice(system: Mode, choice: ThemeChoice) -> Theme {
    theme_for(resolve_mode(system, choice))
}

/// The semantic colours that go with `theme`.
pub fn semantic(theme: &Theme) -> Semantic {
    if theme.extended_palette().is_dark {
        DARK_SEMANTIC
    } else {
        LIGHT_SEMANTIC
    }
}

/// Least contrast of text against its background, as every app colour meets.
pub const MIN_TEXT_CONTRAST: f32 = 4.5;

/// A colour set by a sender, made readable on `background`: kept as sent when
/// it reaches [`MIN_TEXT_CONTRAST`], otherwise mixed toward white (on a dark
/// background) or black (on a light one) in 5 % steps until it does.
pub fn readable_on(color: Color, background: Color) -> Color {
    if contrast(color, background) >= MIN_TEXT_CONTRAST {
        return color;
    }
    let target = if luminance(background) < 0.18 {
        Color::WHITE
    } else {
        Color::BLACK
    };
    for step in 1..=20u8 {
        let t = f32::from(step) / 20.0;
        let mixed = Color::from_rgb(
            color.r + (target.r - color.r) * t,
            color.g + (target.g - color.g) * t,
            color.b + (target.b - color.b) * t,
        );
        if contrast(mixed, background) >= MIN_TEXT_CONTRAST {
            return mixed;
        }
    }
    target
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
    fn the_override_wins_over_the_system_mode() {
        let modes = [Mode::Dark, Mode::Light, Mode::None];
        for system in modes {
            assert_eq!(
                theme_for_choice(system, ThemeChoice::System).to_string(),
                theme_for(system).to_string(),
                "System follows the system ({system:?})"
            );
            let light = theme_for_choice(system, ThemeChoice::Light);
            assert_eq!(light.to_string(), LIGHT_NAME, "{system:?}");
            assert!(!light.extended_palette().is_dark);
            let dark = theme_for_choice(system, ThemeChoice::Dark);
            assert_eq!(dark.to_string(), DARK_NAME, "{system:?}");
            assert!(dark.extended_palette().is_dark);
        }
    }

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
                (
                    "text/message highlight",
                    palette.text,
                    sem.message_highlight,
                ),
                ("muted/message highlight", sem.muted, sem.message_highlight),
                ("text/message hover", palette.text, sem.message_hover),
                ("muted/message hover", sem.muted, sem.message_hover),
                ("primary/background", palette.primary, palette.background),
            ];
            for (what, fg, bg) in checks {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{name} {what}: {ratio}");
            }
        }
    }

    #[test]
    fn the_hover_background_differs_from_the_others() {
        for (palette, sem) in [(LIGHT, LIGHT_SEMANTIC), (DARK, DARK_SEMANTIC)] {
            assert_ne!(sem.message_hover, palette.background);
            assert_ne!(sem.message_hover, sem.message_highlight);
            assert_ne!(sem.message_hover, sem.selection);
        }
    }

    #[test]
    fn sender_colours_are_made_readable_on_both_themes() {
        for palette in [LIGHT, DARK] {
            let bg = palette.background;
            // Every rainbow hue plus the worst cases: white, black, near-bg.
            let mut samples: Vec<Color> = (0..=255u8)
                .step_by(15)
                .flat_map(|v| {
                    [
                        Color::from_rgb8(v, 0, 255 - v),
                        Color::from_rgb8(255, v, 0),
                        Color::from_rgb8(0, 255 - v, v),
                    ]
                })
                .collect();
            samples.extend([Color::WHITE, Color::BLACK, bg]);
            for c in samples {
                let shown = readable_on(c, bg);
                assert!(contrast(shown, bg) >= 4.5, "{c:?} on {bg:?} -> {shown:?}");
            }
        }
        // A colour that is already readable is kept exactly.
        let dark_red = Color::from_rgb8(0x9b, 0x00, 0x00);
        assert_eq!(readable_on(dark_red, LIGHT.background), dark_red);
    }

    #[test]
    fn semantic_colours_follow_the_theme() {
        assert_eq!(semantic(&theme_for(Mode::Dark)), DARK_SEMANTIC);
        assert_eq!(semantic(&theme_for(Mode::Light)), LIGHT_SEMANTIC);
    }
}
