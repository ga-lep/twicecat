//! Four colour sets ("families"), each with a light and a dark palette, that
//! follow the system setting (plan Q13).
//!
//! The mode comes from `iced::system::theme()` at boot and from
//! `iced::system::theme_changes()`; `Mode::None` (unknown) maps to light.
//! `ICED_THEME` only overrides iced's own default theme, which the app does
//! not use, so it has no effect here.

use iced::theme::{Mode, Palette};
use iced::{Color, Theme, color};
use twicecat_core::api::{PaletteChoice, ThemeChoice};

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
    /// Background of a panel or card resting on the window background.
    pub surface: Color,
    /// Background of what floats above a surface: dialogs, menus, banners.
    pub surface_raised: Color,
    /// Lines and outlines (dividers, card borders).
    pub border: Color,
    /// The keyboard focus ring.
    pub focus: Color,
    /// Background of the spaces sidebar.
    pub sidebar: Color,
    /// Soft background of an error banner.
    pub danger_soft: Color,
    /// The ring around a person who is talking (a non-text mark).
    pub speaking: Color,
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
    surface: color!(0xF6F8FA),
    surface_raised: color!(0xEDF0F4),
    border: color!(0xD0D7DE),
    focus: color!(0x3B4CCA),
    sidebar: color!(0xE6EAF0),
    danger_soft: color!(0xFFEBE9),
    speaking: color!(0x1A7F37),
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
    surface: color!(0x25272B),
    surface_raised: color!(0x2F3136),
    border: color!(0x41444B),
    focus: color!(0x8C99F7),
    sidebar: color!(0x17181A),
    danger_soft: color!(0x2D1517),
    speaking: color!(0x3FB950),
};

const FROST_LIGHT: Palette = Palette {
    background: color!(0xFBFCFD),
    text: color!(0x1B2A33),
    primary: color!(0x0B7A75),
    success: color!(0x1A7F37),
    warning: color!(0x915F00),
    danger: color!(0xC62828),
};

const FROST_DARK: Palette = Palette {
    background: color!(0x14181B),
    text: color!(0xE3EAEE),
    primary: color!(0x4FD1C5),
    success: color!(0x3FB950),
    warning: color!(0xD29922),
    danger: color!(0xFF6B6B),
};

const FROST_LIGHT_SEMANTIC: Semantic = Semantic {
    badge: color!(0x4F5F69),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xC62828),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0x55656F),
    selection: color!(0xCFEDEA),
    message_highlight: color!(0xFFF1B8),
    message_hover: color!(0xEEF2F4),
    surface: color!(0xF1F5F7),
    surface_raised: color!(0xE6ECEF),
    border: color!(0xCBD5DA),
    focus: color!(0x0B7A75),
    sidebar: color!(0xDCE4E8),
    danger_soft: color!(0xFDECEC),
    speaking: color!(0x0B7A75),
};

const FROST_DARK_SEMANTIC: Semantic = Semantic {
    badge: color!(0x3A464D),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xB3261E),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0x9AA8B1),
    selection: color!(0x1F4A4A),
    message_highlight: color!(0x3A3217),
    message_hover: color!(0x1D2327),
    surface: color!(0x1B2124),
    surface_raised: color!(0x242C30),
    border: color!(0x38444A),
    focus: color!(0x4FD1C5),
    sidebar: color!(0x0E1214),
    danger_soft: color!(0x2E1618),
    speaking: color!(0x4FD1C5),
};

const EMBER_LIGHT: Palette = Palette {
    background: color!(0xFFFCF9),
    text: color!(0x2B211B),
    primary: color!(0xB34700),
    success: color!(0x2E7D32),
    warning: color!(0x8A5A00),
    danger: color!(0xC62828),
};

const EMBER_DARK: Palette = Palette {
    background: color!(0x1C1815),
    text: color!(0xEDE4DC),
    primary: color!(0xFF9A52),
    success: color!(0x5CC060),
    warning: color!(0xE0A030),
    danger: color!(0xFF6B5E),
};

const EMBER_LIGHT_SEMANTIC: Semantic = Semantic {
    badge: color!(0x6B5A4E),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xC62828),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0x6B5D53),
    selection: color!(0xFBE0CC),
    message_highlight: color!(0xFFEDB0),
    message_hover: color!(0xF6EFE9),
    surface: color!(0xF7F1EB),
    surface_raised: color!(0xEEE6DE),
    border: color!(0xD9CCC0),
    focus: color!(0xB34700),
    sidebar: color!(0xE6DCD2),
    danger_soft: color!(0xFDECEA),
    speaking: color!(0x2E7D32),
};

const EMBER_DARK_SEMANTIC: Semantic = Semantic {
    badge: color!(0x5A4C42),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0xB3261E),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0xAA9C90),
    selection: color!(0x4A2E1C),
    message_highlight: color!(0x3F3314),
    message_hover: color!(0x26201C),
    surface: color!(0x241E1A),
    surface_raised: color!(0x2E2722),
    border: color!(0x463C34),
    focus: color!(0xFF9A52),
    sidebar: color!(0x131010),
    danger_soft: color!(0x321A17),
    speaking: color!(0xFF9A52),
};

const HIGH_CONTRAST_LIGHT: Palette = Palette {
    background: color!(0xFFFFFF),
    text: color!(0x000000),
    primary: color!(0x0000CC),
    success: color!(0x006400),
    warning: color!(0x7A4A00),
    danger: color!(0x9E0016),
};

const HIGH_CONTRAST_DARK: Palette = Palette {
    background: color!(0x000000),
    text: color!(0xFFFFFF),
    primary: color!(0x7DBBFF),
    success: color!(0x5CFF7A),
    warning: color!(0xFFD33D),
    danger: color!(0xFF7B7B),
};

const HIGH_CONTRAST_LIGHT_SEMANTIC: Semantic = Semantic {
    badge: color!(0x333333),
    badge_text: color!(0xFFFFFF),
    highlight: color!(0x9E0016),
    highlight_text: color!(0xFFFFFF),
    muted: color!(0x3D3D3D),
    selection: color!(0xCCD9FF),
    message_highlight: color!(0xFFF2A8),
    message_hover: color!(0xEBEBEB),
    surface: color!(0xF2F2F2),
    surface_raised: color!(0xE4E4E4),
    border: color!(0x000000),
    focus: color!(0x0000CC),
    sidebar: color!(0xD9D9D9),
    danger_soft: color!(0xFFF0F0),
    speaking: color!(0x006400),
};

const HIGH_CONTRAST_DARK_SEMANTIC: Semantic = Semantic {
    badge: color!(0xFFFFFF),
    badge_text: color!(0x000000),
    highlight: color!(0xFF6E6E),
    highlight_text: color!(0x000000),
    muted: color!(0xE0E0E0),
    selection: color!(0x00305F),
    message_highlight: color!(0x4D4000),
    message_hover: color!(0x1A1A1A),
    surface: color!(0x0D0D0D),
    surface_raised: color!(0x1F1F1F),
    border: color!(0xFFFFFF),
    focus: color!(0xFFFF00),
    sidebar: color!(0x141414),
    danger_soft: color!(0x3A0008),
    speaking: color!(0x5CFF7A),
};

/// A colour set. Each has a light and a dark palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// The original blue-violet colours.
    Twicecat,
    /// Cool greys with a teal accent.
    Frost,
    /// Warm neutrals with an orange accent.
    Ember,
    /// Black and white with saturated accents (7:1 for text).
    HighContrast,
}

/// The family `palette: None` stands for: the one place the app's default
/// colour set is decided.
pub const DEFAULT_FAMILY: Family = Family::Twicecat;

impl Family {
    /// Every family, in the order Settings shows them.
    pub const ALL: [Self; 4] = [Self::Twicecat, Self::Frost, Self::Ember, Self::HighContrast];

    /// The family's name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Twicecat => "Twicecat",
            Self::Frost => "Frost",
            Self::Ember => "Ember",
            Self::HighContrast => "High contrast",
        }
    }

    /// The family a stored choice names.
    pub const fn from_choice(choice: PaletteChoice) -> Self {
        match choice {
            PaletteChoice::Twicecat => Self::Twicecat,
            PaletteChoice::Frost => Self::Frost,
            PaletteChoice::Ember => Self::Ember,
            PaletteChoice::HighContrast => Self::HighContrast,
        }
    }

    /// The stored choice for the family.
    pub const fn choice(self) -> PaletteChoice {
        match self {
            Self::Twicecat => PaletteChoice::Twicecat,
            Self::Frost => PaletteChoice::Frost,
            Self::Ember => PaletteChoice::Ember,
            Self::HighContrast => PaletteChoice::HighContrast,
        }
    }

    const fn theme_name(self, dark: bool) -> &'static str {
        match (self, dark) {
            (Self::Twicecat, false) => "Twicecat Light",
            (Self::Twicecat, true) => "Twicecat Dark",
            (Self::Frost, false) => "Frost Light",
            (Self::Frost, true) => "Frost Dark",
            (Self::Ember, false) => "Ember Light",
            (Self::Ember, true) => "Ember Dark",
            (Self::HighContrast, false) => "High contrast Light",
            (Self::HighContrast, true) => "High contrast Dark",
        }
    }
}

/// The family for a stored choice; `None` is [`DEFAULT_FAMILY`].
pub const fn resolve_family(choice: Option<PaletteChoice>) -> Family {
    match choice {
        Some(choice) => Family::from_choice(choice),
        None => DEFAULT_FAMILY,
    }
}

/// The stock palette of a family in light or dark.
pub const fn palette(family: Family, dark: bool) -> Palette {
    match (family, dark) {
        (Family::Twicecat, false) => LIGHT,
        (Family::Twicecat, true) => DARK,
        (Family::Frost, false) => FROST_LIGHT,
        (Family::Frost, true) => FROST_DARK,
        (Family::Ember, false) => EMBER_LIGHT,
        (Family::Ember, true) => EMBER_DARK,
        (Family::HighContrast, false) => HIGH_CONTRAST_LIGHT,
        (Family::HighContrast, true) => HIGH_CONTRAST_DARK,
    }
}

/// The semantic colours of a family in light or dark.
pub const fn semantic_of(family: Family, dark: bool) -> Semantic {
    match (family, dark) {
        (Family::Twicecat, false) => LIGHT_SEMANTIC,
        (Family::Twicecat, true) => DARK_SEMANTIC,
        (Family::Frost, false) => FROST_LIGHT_SEMANTIC,
        (Family::Frost, true) => FROST_DARK_SEMANTIC,
        (Family::Ember, false) => EMBER_LIGHT_SEMANTIC,
        (Family::Ember, true) => EMBER_DARK_SEMANTIC,
        (Family::HighContrast, false) => HIGH_CONTRAST_LIGHT_SEMANTIC,
        (Family::HighContrast, true) => HIGH_CONTRAST_DARK_SEMANTIC,
    }
}

/// The theme of a family in light or dark.
pub fn theme_for_family(family: Family, dark: bool) -> Theme {
    Theme::custom(family.theme_name(dark), palette(family, dark))
}

/// The default family's theme for a system mode. `Mode::None` is light.
pub fn theme_for(mode: Mode) -> Theme {
    theme_for_family(DEFAULT_FAMILY, mode == Mode::Dark)
}

/// The mode the window uses: the user's override, else the system's.
pub fn resolve_mode(system: Mode, choice: ThemeChoice) -> Mode {
    match choice {
        ThemeChoice::System => system,
        ThemeChoice::Light => Mode::Light,
        ThemeChoice::Dark => Mode::Dark,
    }
}

/// The theme for the system mode, the user's choice and the colour set (plan
/// Q17): an override wins over the system, `System` follows it.
pub fn theme_for_choice(system: Mode, choice: ThemeChoice, family: Family) -> Theme {
    theme_for_family(family, resolve_mode(system, choice) == Mode::Dark)
}

/// The semantic colours that go with `theme`: those of the family whose
/// palette it carries, else the default family's for its mode.
pub fn semantic(theme: &Theme) -> Semantic {
    let palette_of_theme = theme.palette();
    for family in Family::ALL {
        for dark in [false, true] {
            if palette(family, dark) == palette_of_theme {
                return semantic_of(family, dark);
            }
        }
    }
    semantic_of(DEFAULT_FAMILY, theme.extended_palette().is_dark)
}

/// Least contrast of text against its background, as every app colour meets.
pub const MIN_TEXT_CONTRAST: f32 = 4.5;

/// A colour set by a sender, made readable on `background`: kept as sent when
/// it reaches [`MIN_TEXT_CONTRAST`], otherwise mixed toward white (on a dark
/// background) or black (on a light one) in 5 % steps until it does.
pub fn readable_on(color: Color, background: Color) -> Color {
    readable_at(color, background, MIN_TEXT_CONTRAST)
}

/// Like [`readable_on`], for a contrast of at least `least`.
pub fn readable_at(color: Color, background: Color, least: f32) -> Color {
    if contrast(color, background) >= least {
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
        if contrast(mixed, background) >= least {
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
                theme_for_choice(system, ThemeChoice::System, DEFAULT_FAMILY).to_string(),
                theme_for(system).to_string(),
                "System follows the system ({system:?})"
            );
            let light = theme_for_choice(system, ThemeChoice::Light, DEFAULT_FAMILY);
            assert_eq!(
                light.to_string(),
                theme_for(Mode::Light).to_string(),
                "{system:?}"
            );
            assert!(!light.extended_palette().is_dark);
            let dark = theme_for_choice(system, ThemeChoice::Dark, DEFAULT_FAMILY);
            assert_eq!(
                dark.to_string(),
                theme_for(Mode::Dark).to_string(),
                "{system:?}"
            );
            assert!(dark.extended_palette().is_dark);
        }
    }

    #[test]
    fn theme_for_names_and_darkness() {
        let cases = [
            (Mode::Dark, format!("{} Dark", DEFAULT_FAMILY.label()), true),
            (
                Mode::Light,
                format!("{} Light", DEFAULT_FAMILY.label()),
                false,
            ),
            (
                Mode::None,
                format!("{} Light", DEFAULT_FAMILY.label()),
                false,
            ),
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
        assert_eq!(
            semantic(&theme_for(Mode::Dark)),
            semantic_of(DEFAULT_FAMILY, true)
        );
        assert_eq!(
            semantic(&theme_for(Mode::Light)),
            semantic_of(DEFAULT_FAMILY, false)
        );
    }

    #[test]
    fn the_new_surfaces_differ_and_their_text_is_readable() {
        for (name, palette, sem) in [
            ("light", LIGHT, LIGHT_SEMANTIC),
            ("dark", DARK, DARK_SEMANTIC),
        ] {
            let bg = palette.background;
            let surfaces = [bg, sem.surface, sem.surface_raised, sem.sidebar];
            for (i, a) in surfaces.iter().enumerate() {
                for b in &surfaces[i + 1..] {
                    assert_ne!(a, b, "{name}: two surfaces are the same colour");
                }
            }
            for (what, fg, back) in [
                ("text/surface", palette.text, sem.surface),
                ("text/surface_raised", palette.text, sem.surface_raised),
                ("text/sidebar", palette.text, sem.sidebar),
                ("muted/surface", sem.muted, sem.surface),
                ("muted/surface_raised", sem.muted, sem.surface_raised),
                ("muted/sidebar", sem.muted, sem.sidebar),
                ("text/danger_soft", palette.text, sem.danger_soft),
                ("danger/danger_soft", palette.danger, sem.danger_soft),
                ("focus/background", sem.focus, bg),
            ] {
                let ratio = contrast(fg, back);
                assert!(ratio >= 4.5, "{name} {what}: {ratio}");
            }
            // Non-text: the speaking ring and the focus ring (WCAG 1.4.11).
            let ring = contrast(sem.speaking, bg);
            assert!(ring >= 3.0, "{name} speaking/background: {ring}");
            assert!(contrast(sem.border, bg) > 1.0, "{name} border");
        }
    }

    const MODES: [Mode; 3] = [Mode::Dark, Mode::Light, Mode::None];
    const CHOICES: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    fn all_sets() -> Vec<(Family, bool)> {
        Family::ALL
            .into_iter()
            .flat_map(|f| [(f, false), (f, true)])
            .collect()
    }

    /// Every pair of text and background a screen draws, as (what, fg, bg).
    fn text_pairs(p: &Palette, s: &Semantic) -> Vec<(&'static str, Color, Color)> {
        let bg = p.background;
        vec![
            ("text/background", p.text, bg),
            ("badge text/badge", s.badge_text, s.badge),
            ("highlight text/highlight", s.highlight_text, s.highlight),
            ("muted/background", s.muted, bg),
            ("text/selection", p.text, s.selection),
            ("text/message highlight", p.text, s.message_highlight),
            ("muted/message highlight", s.muted, s.message_highlight),
            ("text/message hover", p.text, s.message_hover),
            ("muted/message hover", s.muted, s.message_hover),
            ("primary/background", p.primary, bg),
            ("text/surface", p.text, s.surface),
            ("text/surface_raised", p.text, s.surface_raised),
            ("text/sidebar", p.text, s.sidebar),
            ("muted/surface", s.muted, s.surface),
            ("muted/surface_raised", s.muted, s.surface_raised),
            ("muted/sidebar", s.muted, s.sidebar),
            ("text/danger_soft", p.text, s.danger_soft),
            ("danger/danger_soft", p.danger, s.danger_soft),
            ("focus/background", s.focus, bg),
        ]
    }

    #[test]
    fn every_family_in_both_modes_passes_the_contrast_table() {
        for (family, dark) in all_sets() {
            let (p, s) = (palette(family, dark), semantic_of(family, dark));
            let least = if family == Family::HighContrast {
                7.0
            } else {
                4.5
            };
            for (what, fg, bg) in text_pairs(&p, &s) {
                let ratio = contrast(fg, bg);
                assert!(ratio >= least, "{family:?} dark={dark} {what}: {ratio}");
            }
            let ring = contrast(s.speaking, p.background);
            assert!(ring >= 3.0, "{family:?} dark={dark} speaking: {ring}");
            let surfaces = [p.background, s.surface, s.surface_raised, s.sidebar];
            for (i, a) in surfaces.iter().enumerate() {
                for b in &surfaces[i + 1..] {
                    assert_ne!(a, b, "{family:?} dark={dark}: two surfaces match");
                }
            }
        }
    }

    #[test]
    fn the_eight_palettes_are_distinct() {
        let sets = all_sets();
        for (i, a) in sets.iter().enumerate() {
            for b in &sets[i + 1..] {
                assert_ne!(palette(a.0, a.1), palette(b.0, b.1), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn sender_colours_stay_readable_on_all_eight_backgrounds() {
        for (family, dark) in all_sets() {
            let bg = palette(family, dark).background;
            for v in (0..=255u8).step_by(17) {
                for c in [
                    Color::from_rgb8(v, 0, 255 - v),
                    Color::from_rgb8(255, v, 0),
                    Color::from_rgb8(0, 255 - v, v),
                    bg,
                ] {
                    assert!(contrast(readable_on(c, bg), bg) >= 4.5, "{family:?} {c:?}");
                }
            }
        }
    }

    #[test]
    fn semantic_follows_the_family_of_the_theme() {
        for (family, dark) in all_sets() {
            let theme = theme_for_family(family, dark);
            assert_eq!(semantic(&theme), semantic_of(family, dark), "{family:?}");
            assert_eq!(theme.extended_palette().is_dark, dark, "{family:?}");
        }
    }

    #[test]
    fn all_twenty_four_combinations_name_and_darken_correctly() {
        for family in Family::ALL {
            for choice in CHOICES {
                for system in MODES {
                    let theme = theme_for_choice(system, choice, family);
                    let dark = match choice {
                        ThemeChoice::System => system == Mode::Dark,
                        ThemeChoice::Light => false,
                        ThemeChoice::Dark => true,
                    };
                    let name =
                        format!("{} {}", family.label(), if dark { "Dark" } else { "Light" });
                    assert_eq!(theme.to_string(), name, "{family:?} {choice:?} {system:?}");
                    assert_eq!(theme.extended_palette().is_dark, dark);
                }
            }
        }
    }

    #[test]
    fn no_palette_resolves_to_the_default_family() {
        assert_eq!(resolve_family(None), DEFAULT_FAMILY);
        for family in Family::ALL {
            assert_eq!(resolve_family(Some(family.choice())), family);
            assert_eq!(Family::from_choice(family.choice()), family);
        }
    }

    /// The default family is decided in one place: no other code names one.
    #[test]
    fn only_theme_rs_names_the_default_family() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("readable source dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, hits);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("theme.rs")
                {
                    let text = std::fs::read_to_string(&path).expect("readable source");
                    // Skip everything from the tests module on.
                    let code = text.split("#[cfg(test)]").next().unwrap_or("");
                    if code.contains("Family::Twicecat") {
                        hits.push(path.display().to_string());
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut hits,
        );
        assert!(hits.is_empty(), "names Family::Twicecat: {hits:?}");
    }
}
