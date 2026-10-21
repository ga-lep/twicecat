//! The built-in colour theme, "Twicecat", from the project's design colours.
//!
//! Twicecat has one look. A theme with one look is shown in its own mode
//! whatever the system or the stored [`ThemeChoice`] says; only a theme that
//! comes as a light and a dark file of one name (a pair, see
//! [`Resolved::both_modes`]) follows that choice. `ROADMAP.md:99` asked for
//! light and dark from day one; see "Deviation from ROADMAP.md:99" in
//! `PROGRESS.md`. The stored choice is kept untouched.
//!
//! A theme is 15 colour [`Tokens`] under neutral names (`themes.rs` also reads
//! the design's original names). [`map_tokens`] turns them into iced's
//! [`Palette`] and the app's [`Semantic`] colours through one fixed table, so
//! a screen never names a token:
//!
//! | App field | Token |
//! |---|---|
//! | `Palette.background`, `Semantic.sidebar` | `void` |
//! | `Semantic.surface` | `surface` |
//! | `Semantic.surface_raised` | `surface-raised` |
//! | `Semantic.message_hover` | `surface-overlay` |
//! | `Semantic.selection`, `Semantic.message_highlight` | `accent-wash` |
//! | `Palette.text` | `ink` |
//! | `Semantic.muted` | `ink-muted` |
//! | `Palette.primary`, `Semantic.highlight` | `accent-bright` |
//! | `Semantic.badge`, `Semantic.speaking` | `accent` |
//! | `Semantic.badge_text`, `Semantic.highlight_text` | `on-accent` |
//! | `Semantic.border` | `line-strong` |
//! | `Semantic.focus` | `focus` |
//! | `Palette.danger` | `live` |
//! | `Palette.success` | `ok` |
//! | `Palette.warning` | `warn` |
//! | `Semantic.shield_red` | `live` (glyph: `on-accent`, kept only if it reaches 4.5:1) |
//! | `Semantic.shield_grey` | `ink-muted` (glyph: `on-accent`, kept only if it reaches 4.5:1) |
//! | `Semantic.danger_soft` | derived: `surface-raised` mixed 15 % toward `live` |
//!
//! The design has no sidebar token, so the sidebar is `void` too
//! (`sidebar == background` is intended). It has no tinted background either,
//! hence the derived `danger_soft`, which keeps the error banner's tint.
//!
//! Two departures from the design's components (plan Q9):
//! - primary buttons fill with `accent-bright`, not `accent`: the app uses
//!   `primary` both as text (links, the info severity, the quote bar), where
//!   the design allows only `accent-bright`, and as the button fill. The label
//!   is black from `kit::on_color` (8.69:1);
//! - the mention badge is `accent-bright`, not the unread pill with an "@",
//!   because the app tells mentions from unread counts by colour.
//!
//! [`resolve`] checks a set of tokens before it is shown (rules V1 to V5, see
//! its docs): custom themes from `themes.rs` go through it, and so does a
//! test of the built-in theme.
//!
//! [`semantic`] finds the semantic colours of the theme the window shows;
//! [`theme_of`] is the only place the app builds an iced `Theme` (a test checks
//! that no other code calls `Theme::custom`).

use iced::theme::{Mode, Palette};
use iced::{Color, Theme, color};
use std::sync::{PoisonError, RwLock};
use twicecat_core::api::ThemeChoice;

/// The 15 colours a theme sets, under neutral names. All are opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// `void`: the window background.
    pub void: Color,
    /// `surface`: main panels.
    pub surface: Color,
    /// `surface-raised`: cards, menus, secondary buttons.
    pub surface_raised: Color,
    /// `surface-overlay`: the row under the pointer.
    pub surface_overlay: Color,
    /// `accent-wash`: the selected row.
    pub accent_wash: Color,
    /// `line-strong`: outlines of controls.
    pub line_strong: Color,
    /// `ink`: text.
    pub ink: Color,
    /// `ink-muted`: secondary text.
    pub ink_muted: Color,
    /// `accent`: the unread pill and the speaking ring.
    pub accent: Color,
    /// `on-accent`: text on the accent.
    pub on_accent: Color,
    /// `accent-bright`: accent text, primary buttons and mentions.
    pub accent_bright: Color,
    /// `focus`: the keyboard focus ring.
    pub focus: Color,
    /// `live`: errors, muted, leave, identity changed.
    pub live: Color,
    /// `ok`: verified, connected.
    pub ok: Color,
    /// `warn`: unverified, unstable.
    pub warn: Color,
}

impl Tokens {
    /// The 15 colours in the order of [`TOKEN_NAMES`].
    pub fn as_array(&self) -> [Color; 15] {
        [
            self.void,
            self.surface,
            self.surface_raised,
            self.surface_overlay,
            self.accent_wash,
            self.line_strong,
            self.ink,
            self.ink_muted,
            self.accent,
            self.on_accent,
            self.accent_bright,
            self.focus,
            self.live,
            self.ok,
            self.warn,
        ]
    }

    /// Sets the token called `name` (one of [`TOKEN_NAMES`]); false for any
    /// other name.
    pub fn set(&mut self, name: &str, color: Color) -> bool {
        let slot = match name {
            "void" => &mut self.void,
            "surface" => &mut self.surface,
            "surface-raised" => &mut self.surface_raised,
            "surface-overlay" => &mut self.surface_overlay,
            "accent-wash" => &mut self.accent_wash,
            "line-strong" => &mut self.line_strong,
            "ink" => &mut self.ink,
            "ink-muted" => &mut self.ink_muted,
            "accent" => &mut self.accent,
            "on-accent" => &mut self.on_accent,
            "accent-bright" => &mut self.accent_bright,
            "focus" => &mut self.focus,
            "live" => &mut self.live,
            "ok" => &mut self.ok,
            "warn" => &mut self.warn,
            _ => return false,
        };
        *slot = color;
        true
    }
}

/// The built-in "Twicecat" colours: the design's values.
pub const BUILT_IN: Tokens = Tokens {
    void: color!(0x0A, 0x08, 0x12),
    surface: color!(0x11, 0x0E, 0x1C),
    surface_raised: color!(0x18, 0x14, 0x28),
    surface_overlay: color!(0x21, 0x1B, 0x36),
    accent_wash: color!(0x24, 0x1A, 0x45),
    line_strong: color!(0x6B, 0x61, 0x94),
    ink: color!(0xEF, 0xEB, 0xFA),
    ink_muted: color!(0xA6, 0x9F, 0xC4),
    accent: color!(0x8E, 0x5C, 0xFF),
    on_accent: color!(0x0A, 0x08, 0x12),
    accent_bright: color!(0xB7, 0x94, 0xFF),
    focus: color!(0xC9, 0xAE, 0xFF),
    live: color!(0xFF, 0x6B, 0x86),
    ok: color!(0x5E, 0xE0, 0xC8),
    warn: color!(0xFF, 0xB5, 0x47),
};

/// Share of `live` in the error banner's tint.
const DANGER_SOFT_SHARE: f32 = 0.15;

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
    /// Fill of the red message shield (a warning about authenticity).
    pub shield_red: Color,
    /// The `!` on [`Semantic::shield_red`].
    pub shield_red_glyph: Color,
    /// Fill of the grey message shield (a caution). A neutral of its own:
    /// not the accent, which already means unread and speaking.
    pub shield_grey: Color,
    /// The `!` on [`Semantic::shield_grey`].
    pub shield_grey_glyph: Color,
}

/// The palette and the semantic colours of `tokens`, by the table in the
/// module docs.
pub fn map_tokens(tokens: &Tokens) -> (Palette, Semantic) {
    let palette = Palette {
        background: tokens.void,
        text: tokens.ink,
        primary: tokens.accent_bright,
        success: tokens.ok,
        warning: tokens.warn,
        danger: tokens.live,
    };
    let semantic = Semantic {
        badge: tokens.accent,
        badge_text: tokens.on_accent,
        highlight: tokens.accent_bright,
        highlight_text: tokens.on_accent,
        muted: tokens.ink_muted,
        selection: tokens.accent_wash,
        message_highlight: tokens.accent_wash,
        message_hover: tokens.surface_overlay,
        surface: tokens.surface,
        surface_raised: tokens.surface_raised,
        border: tokens.line_strong,
        focus: tokens.focus,
        sidebar: tokens.void,
        danger_soft: mix(tokens.surface_raised, tokens.live, DANGER_SOFT_SHARE),
        speaking: tokens.accent,
        shield_red: tokens.live,
        shield_red_glyph: readable_on(tokens.on_accent, tokens.live),
        shield_grey: tokens.ink_muted,
        shield_grey_glyph: readable_on(tokens.on_accent, tokens.ink_muted),
    };
    (palette, semantic)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

/// A theme ready to show: its name, its side and its colours.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// What the user sees.
    pub name: String,
    /// Whether the backgrounds are dark.
    pub dark: bool,
    /// Whether the theme comes as a light and dark pair, so that the
    /// System/Light/Dark choice applies. A single-mode theme is shown in its
    /// own mode whatever that choice is.
    pub both_modes: bool,
    /// The stock palette.
    pub palette: Palette,
    /// The app's own colours.
    pub semantic: Semantic,
    /// The tokens `resolve` had to change (replaced or made more readable),
    /// by name. Empty for a theme that passed as it came.
    pub report: Vec<&'static str>,
}

/// The built-in theme's colours.
pub fn built_in_resolved() -> Resolved {
    let (palette, semantic) = map_tokens(&BUILT_IN);
    Resolved {
        name: BUILT_IN_NAME.to_owned(),
        dark: luminance(palette.background) < 0.18,
        both_modes: false,
        palette,
        semantic,
        report: Vec::new(),
    }
}

/// Which look of a theme to show. `looks` are the accepted looks of one theme
/// name. A theme with one look returns it whatever `choice` and `system` say.
/// A pair (more than one look) follows `choice`: `Light` and `Dark` force that
/// look, `System` takes the dark look only when `system` is `Dark` (an unknown
/// mode, `Mode::None`, gives the light look). If the pair lacks the wanted
/// look, the first one is returned. `None` only for an empty `looks`.
pub fn pick_look<'a>(
    looks: &[&'a Resolved],
    choice: ThemeChoice,
    system: Mode,
) -> Option<&'a Resolved> {
    let first = looks.first().copied()?;
    if looks.len() == 1 {
        return Some(first);
    }
    let want_dark = match choice {
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
        ThemeChoice::System => system == Mode::Dark,
    };
    Some(
        looks
            .iter()
            .copied()
            .find(|look| look.dark == want_dark)
            .unwrap_or(first),
    )
}

/// The name of the built-in theme.
pub const BUILT_IN_NAME: &str = "Twicecat";

/// The iced theme of `resolved`: the only `Theme::custom` call in the app.
pub fn theme_of(resolved: &Resolved) -> Theme {
    register(resolved);
    Theme::custom(resolved.name.clone(), resolved.palette)
}

/// The built-in theme, "Twicecat".
pub fn built_in() -> Theme {
    theme_of(&built_in_resolved())
}

/// Every theme built by [`theme_of`], so that [`semantic`] can find the
/// semantic colours that go with an iced `Theme` (which keeps only a name and a
/// palette). A light and a dark theme of one name differ in their palette.
static REGISTRY: RwLock<Vec<(String, Palette, Semantic)>> = RwLock::new(Vec::new());

/// Most themes kept: the built-in plus 32 files of 2 modes. The oldest goes
/// first, so repeated reloads cannot grow the list.
const REGISTRY_CAP: usize = 65;

fn register(resolved: &Resolved) {
    let mut list = REGISTRY.write().unwrap_or_else(PoisonError::into_inner);
    push_bounded(&mut list, REGISTRY_CAP, resolved);
}

/// Adds `resolved` to `list`, or replaces its entry (same name and palette);
/// when the list is full, the oldest entry goes.
fn push_bounded(list: &mut Vec<(String, Palette, Semantic)>, cap: usize, resolved: &Resolved) {
    if let Some(entry) = list
        .iter_mut()
        .find(|(name, palette, _)| *name == resolved.name && *palette == resolved.palette)
    {
        entry.2 = resolved.semantic;
        return;
    }
    if list.len() >= cap {
        list.remove(0);
    }
    list.push((resolved.name.clone(), resolved.palette, resolved.semantic));
}

/// The semantic colours that go with `theme`: those registered by
/// [`theme_of`] for its name and palette, else the built-in theme's.
pub fn semantic(theme: &Theme) -> Semantic {
    let palette = theme.palette();
    let list = REGISTRY.read().unwrap_or_else(PoisonError::into_inner);
    if list.iter().any(|(_, p, _)| *p == palette) {
        let name = theme.to_string();
        if let Some((_, _, semantic)) = list
            .iter()
            .rev()
            .find(|(n, p, _)| *p == palette && *n == name)
        {
            return *semantic;
        }
    }
    map_tokens(&BUILT_IN).1
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

/// `color` made readable over every one of `backgrounds` at `least`:
/// kept as it is when it already reaches `least` on all of them, otherwise
/// mixed in 5 % steps toward white (when most of the backgrounds are dark) or
/// black until it does, trying the other end when that one cannot. When
/// neither end reaches `least` on all of them (a set of light and dark
/// surfaces together), the result is the end with the higher lowest contrast,
/// which is still short somewhere: `resolve()` refuses surface sets like
/// that, and checks its result again.
pub fn readable_over(color: Color, backgrounds: &[Color], least: f32) -> Color {
    let all = |c: Color| backgrounds.iter().all(|&bg| contrast(c, bg) >= least);
    let dark = backgrounds.iter().filter(|&&b| luminance(b) < 0.18).count();
    let targets = if dark * 2 >= backgrounds.len() {
        [Color::WHITE, Color::BLACK]
    } else {
        [Color::BLACK, Color::WHITE]
    };
    if let Some(found) = lift(color, targets, all) {
        return found;
    }
    // Neither end works for every surface: the end that is least bad, by its
    // lowest contrast over them.
    let worst = |c: Color| {
        backgrounds
            .iter()
            .map(|&bg| contrast(c, bg))
            .fold(f32::MAX, f32::min)
    };
    if worst(targets[1]) > worst(targets[0]) {
        targets[1]
    } else {
        targets[0]
    }
}

/// `color` itself if `ok` holds, else the first of its mixes (5 % steps
/// toward each of `targets` in turn) for which it does.
fn lift(color: Color, targets: [Color; 2], ok: impl Fn(Color) -> bool) -> Option<Color> {
    if ok(color) {
        return Some(color);
    }
    for target in targets {
        for step in 1..=20u8 {
            let mixed = mix(color, target, f32::from(step) / 20.0);
            if ok(mixed) {
                return Some(mixed);
            }
        }
    }
    None
}

/// A colour set by a sender, made readable on every one of `backgrounds` at
/// [`MIN_TEXT_CONTRAST`] (see [`readable_over`]).
pub fn readable_on_every(color: Color, backgrounds: &[Color]) -> Color {
    readable_over(color, backgrounds, MIN_TEXT_CONTRAST)
}

/// Every surface a sender's name or coloured text can be drawn on: the window
/// background, the panels, the selected, mentioned and hovered rows, the
/// error tint, and the code pill (`code_background`, which iced derives).
pub fn sender_surfaces(palette: &Palette, sem: &Semantic, code_background: Color) -> [Color; 8] {
    [
        palette.background,
        sem.surface,
        sem.surface_raised,
        sem.selection,
        sem.message_highlight,
        sem.message_hover,
        sem.danger_soft,
        code_background,
    ]
}

/// A sender's colour as drawn in `theme`: readable on all of
/// [`sender_surfaces`], so it does not matter which one the row is on.
pub fn sender_colour(color: Color, theme: &Theme) -> Color {
    let weak = theme.extended_palette().background.weak.color;
    let surfaces = sender_surfaces(&theme.palette(), &semantic(theme), weak);
    readable_on_every(color, &surfaces)
}

/// The names of the 15 tokens, in the design's order.
pub const TOKEN_NAMES: [&str; 15] = [
    "void",
    "surface",
    "surface-raised",
    "surface-overlay",
    "accent-wash",
    "line-strong",
    "ink",
    "ink-muted",
    "accent",
    "on-accent",
    "accent-bright",
    "focus",
    "live",
    "ok",
    "warn",
];

/// Why a theme is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Warning, error, success, info, focus or the accent are too close to
    /// each other or to the text colours (V4).
    StatusAlike,
    /// The backgrounds are not all dark or all light (V2).
    MixedBackgrounds,
    /// The selected or hovered row would not show against its neighbours
    /// (V5).
    SelectionHidden,
    /// After all adjustments some colour is still below its least contrast:
    /// the last check refuses what the steps before could not fix.
    Unreadable,
}

impl Refusal {
    /// The fixed words for the report.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::StatusAlike => "status colours look too alike",
            Refusal::MixedBackgrounds => "backgrounds mix light and dark",
            Refusal::SelectionHidden => "selected rows would not show",
            Refusal::Unreadable => "colours cannot be made readable",
        }
    }
}

/// Luminance below which a background counts as dark.
const DARK_BELOW: f32 = 0.18;
/// V3: least contrast of text.
const V3_TEXT: f32 = MIN_TEXT_CONTRAST;
/// V3: least contrast of a mark that is not text (WCAG 1.4.11).
const V3_NON_TEXT: f32 = 3.0;
/// V4: least CIE76 distance between colours that must not be mistaken.
const V4_DISTINCT: f32 = 20.0;
/// V3: how far the pressed and hovered states of the kit's buttons and chips
/// may move a background toward the text colour (the largest mix they use);
/// `ink` must keep [`V3_TEXT`] on what it makes of each background.
const V3_STATE_MIX: f32 = 0.15;
/// V5: least distance of the selected row from the surfaces it sits on.
const V5_SELECTED: f32 = 8.0;
/// V5: least distance of the hovered row from its neighbours.
const V5_HOVERED: f32 = 5.0;

fn is_dark(color: Color) -> bool {
    luminance(color) < DARK_BELOW
}

/// A colour a theme may carry: finite, within range and opaque (V1).
fn sane(c: Color) -> bool {
    [c.r, c.g, c.b]
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        && c.a == 1.0
}

/// The derived error tint for `live`.
fn tint(tokens: &Tokens) -> Color {
    mix(tokens.surface_raised, tokens.live, DANGER_SOFT_SHARE)
}

/// The five backgrounds text is drawn on.
fn backgrounds_of(t: &Tokens) -> [Color; 5] {
    [
        t.void,
        t.surface,
        t.surface_raised,
        t.surface_overlay,
        t.accent_wash,
    ]
}

/// The surfaces a ring, a badge or an outline sits on.
fn cards_of(t: &Tokens) -> [Color; 3] {
    [t.void, t.surface, t.surface_raised]
}

/// V2 on the five backgrounds and the tint: all dark or all light.
fn one_side(t: &Tokens) -> bool {
    let dark = is_dark(t.void);
    backgrounds_of(t)
        .into_iter()
        .chain([tint(t)])
        .all(|c| is_dark(c) == dark)
}

/// V3: moves each colour that is too faint just far enough, in an order that
/// lets later colours depend on earlier ones (`on-accent` on the accents).
fn make_readable(t: &mut Tokens) {
    let bgs = backgrounds_of(t);
    let cards = cards_of(t);
    let dark = is_dark(t.void);
    let ends = if dark {
        [Color::WHITE, Color::BLACK]
    } else {
        [Color::BLACK, Color::WHITE]
    };
    // Marks that are not text.
    t.line_strong = readable_over(t.line_strong, &cards, V3_NON_TEXT);
    let ring_on = [cards[0], cards[1], cards[2], disabled_box(t)];
    t.accent = readable_over(t.accent, &ring_on, V3_NON_TEXT);
    t.focus = readable_over(t.focus, &bgs, V3_NON_TEXT);
    // `live` is also drawn on its own tint, which moves with it.
    let base = *t;
    let live_ok = |c: Color| {
        let soft = tint(&Tokens { live: c, ..base });
        bgs.iter()
            .chain([&soft])
            .all(|&bg| contrast(c, bg) >= V3_TEXT)
    };
    if let Some(live) = lift(t.live, ends, live_ok) {
        t.live = live;
    }
    t.ok = readable_over(t.ok, &bgs, V3_TEXT);
    t.warn = readable_over(t.warn, &bgs, V3_TEXT);
    t.accent_bright = readable_over(t.accent_bright, &bgs, V3_TEXT);
    let mut on_tint: Vec<Color> = bgs.to_vec();
    on_tint.push(tint(t));
    // `ink` also sits on the backgrounds as the pressed and hovered states
    // of buttons and chips make them (mixed toward the text colour).
    let ink_ok = |c: Color| ink_readable(c, &on_tint);
    t.ink = lift(t.ink, ends, ink_ok).unwrap_or(t.ink);
    t.ink_muted = readable_over(t.ink_muted, &on_tint, V3_TEXT);
    // The label on both accents; when no single colour serves both, the ring
    // colour moves toward the end of its side until one does.
    for step in 0..=20u8 {
        let ring = if step == 0 {
            t.accent
        } else {
            mix(t.accent, ends[0], f32::from(step) / 20.0)
        };
        let label = readable_over(t.on_accent, &[ring, t.accent_bright], V3_TEXT);
        if contrast(label, ring) >= V3_TEXT && contrast(label, t.accent_bright) >= V3_TEXT {
            t.accent = ring;
            t.on_accent = label;
            break;
        }
    }
}

/// Whether `ink` reaches [`V3_TEXT`] on each of `backgrounds` and on each of
/// them mixed [`V3_STATE_MIX`] toward `ink`.
fn ink_readable(ink: Color, backgrounds: &[Color]) -> bool {
    backgrounds.iter().all(|&bg| {
        contrast(ink, bg) >= V3_TEXT && contrast(ink, mix(bg, ink, V3_STATE_MIX)) >= V3_TEXT
    })
}

/// The grey box of a disabled, unchecked checkbox (the kit mixes
/// `surface-raised` 35 % toward the outline colour). The ring colour is also
/// the fill of the disabled, checked box, so it must stand out from this.
fn disabled_box(t: &Tokens) -> Color {
    mix(t.surface_raised, t.line_strong, 0.35)
}

/// V4: whether two colours that must not be mistaken for one another are
/// closer than [`V4_DISTINCT`].
fn status_alike(t: &Tokens) -> bool {
    let close = |a: Color, b: Color| delta_e76(a, b) < V4_DISTINCT;
    let status = [t.live, t.ok, t.warn, t.accent_bright];
    for (i, &a) in status.iter().enumerate() {
        if status[i + 1..].iter().any(|&b| close(a, b)) {
            return true;
        }
        if close(a, t.ink) || close(a, t.ink_muted) {
            return true;
        }
    }
    // The focus ring is not a status, not text and not the idle outline. The
    // accents (`accent-bright`, `accent`) are its siblings by design: the
    // built-in ring is only 17 from the bright accent, so no bound there.
    let ring_against = [t.live, t.ok, t.warn, t.ink, t.ink_muted, t.line_strong];
    // The unread pill and the speaking ring against the error and the grey
    // shield (`live`, `ink-muted`).
    ring_against.iter().any(|&c| close(t.focus, c))
        || close(t.accent, t.live)
        || close(t.accent, t.ink_muted)
}

/// V5: whether the selected or the hovered row would not show.
fn selection_hidden(t: &Tokens) -> bool {
    let far = |a: Color, b: Color, least: f32| delta_e76(a, b) >= least;
    let wash_ok = [t.void, t.surface, t.surface_raised]
        .into_iter()
        .all(|b| far(t.accent_wash, b, V5_SELECTED));
    let hover_ok = [t.surface, t.surface_raised, t.accent_wash]
        .into_iter()
        .all(|b| far(t.surface_overlay, b, V5_HOVERED));
    !(wash_ok && hover_ok)
}

/// The last check, on the colours as they will be shown: the sides (also of
/// the code background iced derives from the palette), V4, V5, then every V3
/// pair and the shield marks.
fn verify(t: &Tokens) -> Result<(), Refusal> {
    let (palette, sem) = map_tokens(t);
    let dark = is_dark(t.void);
    let weak = iced::theme::palette::Extended::generate(palette)
        .background
        .weak
        .color;
    if !one_side(t) || is_dark(weak) != dark {
        return Err(Refusal::MixedBackgrounds);
    }
    if status_alike(t) {
        return Err(Refusal::StatusAlike);
    }
    if selection_hidden(t) {
        return Err(Refusal::SelectionHidden);
    }
    let bgs = backgrounds_of(t);
    let cards = cards_of(t);
    let mut on_tint: Vec<Color> = bgs.to_vec();
    on_tint.push(sem.danger_soft);
    let text_on_all = [t.ink, t.ink_muted, t.accent_bright, t.live, t.ok, t.warn];
    let mut short = text_on_all
        .iter()
        .any(|&fg| bgs.iter().any(|&bg| contrast(fg, bg) < V3_TEXT));
    short |= !ink_readable(t.ink, &on_tint);
    short |= [t.ink, t.ink_muted, t.live]
        .iter()
        .any(|&fg| contrast(fg, sem.danger_soft) < V3_TEXT);
    short |= [t.accent, t.accent_bright]
        .iter()
        .any(|&fill| contrast(t.on_accent, fill) < V3_TEXT);
    short |= bgs.iter().any(|&bg| contrast(t.focus, bg) < V3_NON_TEXT);
    short |= cards.iter().any(|&bg| {
        contrast(t.accent, bg) < V3_NON_TEXT || contrast(t.line_strong, bg) < V3_NON_TEXT
    });
    short |= contrast(t.accent, disabled_box(t)) < V3_NON_TEXT;
    // The shields: the mark on its fill, the fill on every row.
    for (fill, glyph) in [
        (sem.shield_red, sem.shield_red_glyph),
        (sem.shield_grey, sem.shield_grey_glyph),
    ] {
        short |= contrast(glyph, fill) < V3_TEXT;
        short |= bgs.iter().any(|&bg| contrast(fill, bg) < V3_NON_TEXT);
    }
    if short {
        return Err(Refusal::Unreadable);
    }
    Ok(())
}

/// Checks `tokens` and makes them safe to show (rules V1 to V5 of the plan).
///
/// - **V1** a colour that is not finite, in range and opaque takes the
///   built-in value;
/// - **V2** the five backgrounds and the error tint must be all dark or all
///   light, else the theme is refused (which makes V3 always solvable);
/// - **V3** a colour below its least contrast on a surface it is drawn on is
///   mixed toward white (dark side) or black just far enough: 4.5:1 for
///   text, 3:1 for the ring, the badge, the outline and the focus colour;
/// - **V4** the error, success, warning and info colours, the text colours,
///   the focus ring and the accent must stay apart (CIE76 20), else refused;
/// - **V5** the selected and hovered rows must show (8 and 5), else refused.
///
/// A last pass checks everything on the final colours and refuses what is
/// still short. The returned `report` names the tokens that were changed.
/// The name of the result is empty and `both_modes` false: the caller sets
/// them.
pub fn resolve(tokens: &Tokens) -> Result<Resolved, Refusal> {
    let mut t = *tokens;
    for (i, name) in TOKEN_NAMES.iter().enumerate() {
        if !sane(tokens.as_array()[i]) {
            t.set(name, BUILT_IN.as_array()[i]);
        }
    }
    if !one_side(&t) {
        return Err(Refusal::MixedBackgrounds);
    }
    make_readable(&mut t);
    verify(&t)?;
    let report: Vec<&'static str> = TOKEN_NAMES
        .iter()
        .zip(tokens.as_array().into_iter().zip(t.as_array()))
        .filter(|(_, (before, after))| before != after)
        .map(|(name, _)| *name)
        .collect();
    let (palette, semantic) = map_tokens(&t);
    Ok(Resolved {
        name: String::new(),
        dark: is_dark(palette.background),
        both_modes: false,
        palette,
        semantic,
        report,
    })
}

/// A small seeded generator (std only): the same sets on every run.
#[cfg(test)]
pub(crate) struct Lcg(pub u64);

#[cfg(test)]
impl Lcg {
    pub fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn rgb(&mut self, lo: u8, hi: u8) -> Color {
        let span = u32::from(hi - lo) + 1;
        let mut c = || lo + (self.next() % span) as u8;
        let (r, g, b) = (c(), c(), c());
        Color::from_rgb8(r, g, b)
    }

    /// Random tokens: backgrounds on a random side, anything else anywhere,
    /// and some collisions on purpose.
    pub fn tokens(&mut self) -> Tokens {
        let dark = self.next().is_multiple_of(2);
        let (lo, hi) = if dark { (0, 0x40) } else { (0xc0, 0xff) };
        let mut t = BUILT_IN;
        for (i, name) in TOKEN_NAMES.iter().enumerate() {
            let c = if i < 5 {
                self.rgb(lo, hi)
            } else {
                self.rgb(0, 255)
            };
            t.set(name, c);
        }
        match self.next() % 6 {
            0 => t.live = t.ink,
            1 => t.focus = t.ink_muted,
            2 => t.accent_wash = t.surface,
            _ => {}
        }
        t
    }
}

/// `count` accepted random themes (registered), for the kit's form-control
/// tests.
#[cfg(test)]
pub(crate) fn random_accepted_themes(seed: u64, count: usize) -> Vec<Theme> {
    let mut rng = Lcg(seed);
    let mut out = Vec::new();
    while out.len() < count {
        if let Ok(mut r) = resolve(&rng.tokens()) {
            r.name = format!("Random {}", out.len());
            out.push(theme_of(&r));
        }
    }
    out
}

/// The themes the kit, rich text and call tests run over: Twicecat, then the
/// fixtures `Daylight`, `Duo` (both looks), `Faint` (after its adjustment) and `Paper`.
#[cfg(test)]
pub(crate) fn test_themes() -> Vec<Theme> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/themes");
    let loaded = crate::themes::load_dir(&dir);
    assert!(loaded.report.len() <= 1, "fixtures: {:?}", loaded.report);
    std::iter::once(built_in())
        .chain(loaded.themes.iter().map(theme_of))
        .collect()
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

/// CIE76 colour difference in CIELAB (D65), for the distinctness tests.
pub fn delta_e76(a: Color, b: Color) -> f32 {
    fn lab(c: Color) -> [f32; 3] {
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f32| {
            if t > 0.008_856 {
                t.cbrt()
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }
    let (a, b) = (lab(a), lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_scan;

    /// The design's 15 colours, as written in its `tokens.json`.
    const DESIGN: [(&str, u32); 15] = [
        ("void", 0x0a0812),
        ("surface", 0x110e1c),
        ("surface-raised", 0x181428),
        ("surface-overlay", 0x211b36),
        ("accent-wash", 0x241a45),
        ("line-strong", 0x6b6194),
        ("ink", 0xefebfa),
        ("ink-muted", 0xa69fc4),
        ("accent", 0x8e5cff),
        ("on-accent", 0x0a0812),
        ("accent-bright", 0xb794ff),
        ("focus", 0xc9aeff),
        ("live", 0xff6b86),
        ("ok", 0x5ee0c8),
        ("warn", 0xffb547),
    ];

    fn hex(c: Color) -> u32 {
        let [r, g, b, a] = c.into_rgba8();
        assert_eq!(a, 255, "opaque");
        u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    }

    fn tokens_in_design_order(t: &Tokens) -> [Color; 15] {
        [
            t.void,
            t.surface,
            t.surface_raised,
            t.surface_overlay,
            t.accent_wash,
            t.line_strong,
            t.ink,
            t.ink_muted,
            t.accent,
            t.on_accent,
            t.accent_bright,
            t.focus,
            t.live,
            t.ok,
            t.warn,
        ]
    }

    fn built() -> (Palette, Semantic) {
        map_tokens(&BUILT_IN)
    }

    #[test]
    fn the_built_in_tokens_are_the_design_colours() {
        let got = tokens_in_design_order(&BUILT_IN);
        for ((name, want), got) in DESIGN.iter().zip(got) {
            assert_eq!(hex(got), *want, "{name}");
        }
    }

    #[test]
    fn the_derived_error_tint_is_the_documented_colour() {
        assert_eq!(hex(built().1.danger_soft), 0x3b2136);
    }

    #[test]
    fn every_token_lands_where_the_table_says() {
        let (p, s) = built();
        let t = BUILT_IN;
        assert_eq!(p.background, t.void);
        assert_eq!(s.sidebar, t.void);
        assert_eq!(s.surface, t.surface);
        assert_eq!(s.surface_raised, t.surface_raised);
        assert_eq!(s.message_hover, t.surface_overlay);
        assert_eq!(s.selection, t.accent_wash);
        assert_eq!(s.message_highlight, t.accent_wash);
        assert_eq!(p.text, t.ink);
        assert_eq!(s.muted, t.ink_muted);
        // Q9: the accent text, the primary fill and the mention badge are the
        // bright accent; the pill and the ring are the plain one.
        assert_eq!(p.primary, t.accent_bright);
        assert_eq!(s.highlight, t.accent_bright);
        assert_eq!(s.badge, t.accent);
        assert_eq!(s.speaking, t.accent);
        assert_eq!(s.badge_text, t.on_accent);
        assert_eq!(s.highlight_text, t.on_accent);
        assert_eq!(s.border, t.line_strong);
        assert_eq!(s.focus, t.focus);
        assert_eq!(p.danger, t.live);
        assert_eq!(p.success, t.ok);
        assert_eq!(p.warning, t.warn);
    }

    #[test]
    fn the_built_in_theme_is_named_twicecat_and_has_one_look() {
        let theme = built_in();
        assert_eq!(theme.to_string(), "Twicecat");
        assert!(theme.extended_palette().is_dark);
        let resolved = built_in_resolved();
        assert!(resolved.dark);
        assert!(!resolved.both_modes, "one mode only");
        assert_eq!(theme_of(&resolved).to_string(), "Twicecat");
    }

    #[test]
    fn semantic_colours_are_the_built_in_ones_for_any_theme() {
        let want = built().1;
        assert_eq!(semantic(&built_in()), want);
        assert_eq!(semantic(&Theme::Light), want, "an unknown theme");
    }

    #[test]
    fn contrast_matches_the_wcag_reference_values() {
        assert!((contrast(Color::BLACK, Color::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color::WHITE, Color::WHITE) - 1.0).abs() < 0.001);
        let (p, _) = built();
        assert_eq!(
            contrast(p.text, p.background),
            contrast(p.background, p.text)
        );
        // #767676 on white is the well-known 4.54:1 boundary grey.
        let grey = color!(0x767676);
        assert!((contrast(grey, Color::WHITE) - 4.54).abs() < 0.01);
    }

    /// The five backgrounds and the tint that text is drawn on.
    fn backgrounds() -> [(&'static str, Color); 5] {
        let t = BUILT_IN;
        [
            ("void", t.void),
            ("surface", t.surface),
            ("surface-raised", t.surface_raised),
            ("surface-overlay", t.surface_overlay),
            ("accent-wash", t.accent_wash),
        ]
    }

    #[test]
    fn every_text_pair_of_the_built_in_theme_is_at_least_4_5_to_1() {
        let t = BUILT_IN;
        let soft = built().1.danger_soft;
        for (bg_name, bg) in backgrounds() {
            for (name, fg) in [
                ("ink", t.ink),
                ("ink-muted", t.ink_muted),
                ("accent-bright", t.accent_bright),
                ("live", t.live),
                ("ok", t.ok),
                ("warn", t.warn),
            ] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{name} on {bg_name}: {ratio}");
            }
        }
        for (name, fg) in [("ink", t.ink), ("ink-muted", t.ink_muted), ("live", t.live)] {
            let ratio = contrast(fg, soft);
            assert!(ratio >= 4.5, "{name} on danger_soft: {ratio}");
        }
        for (name, bg) in [("accent", t.accent), ("accent-bright", t.accent_bright)] {
            let ratio = contrast(t.on_accent, bg);
            assert!(ratio >= 4.5, "on-accent on {name}: {ratio}");
        }
        // The tightest text pair, as planned.
        assert!(contrast(t.on_accent, t.accent) >= 4.85 - 0.01);
        assert!(contrast(t.ink_muted, soft) >= 5.75 - 0.01);
    }

    #[test]
    fn every_non_text_pair_of_the_built_in_theme_is_at_least_3_to_1() {
        let t = BUILT_IN;
        for (bg_name, bg) in backgrounds() {
            let ratio = contrast(t.focus, bg);
            assert!(ratio >= 3.0, "focus on {bg_name}: {ratio}");
        }
        for (bg_name, bg) in [
            ("void", t.void),
            ("surface", t.surface),
            ("surface-raised", t.surface_raised),
        ] {
            let ring = contrast(t.accent, bg);
            assert!(ring >= 3.0, "accent on {bg_name}: {ring}");
        }
        // The outline is drawn on the panel surface and on the cards.
        for (name, bg) in [("surface", t.surface), ("surface-raised", t.surface_raised)] {
            let line = contrast(t.line_strong, bg);
            assert!(line >= 3.22 - 0.01, "line-strong on {name}: {line}");
        }
    }

    #[test]
    fn warning_error_success_and_info_stay_distinct_from_each_other_and_from_text() {
        let t = BUILT_IN;
        let status = [
            ("live", t.live),
            ("ok", t.ok),
            ("warn", t.warn),
            ("accent-bright", t.accent_bright),
        ];
        let mut closest = f32::MAX;
        for (i, (an, a)) in status.iter().enumerate() {
            for (bn, b) in &status[i + 1..] {
                let d = delta_e76(*a, *b);
                closest = closest.min(d);
                assert!(d >= 20.0, "{an} vs {bn}: dE {d}");
            }
            for (tn, text) in [("ink", t.ink), ("ink-muted", t.ink_muted)] {
                let d = delta_e76(*a, text);
                closest = closest.min(d);
                assert!(d >= 20.0, "{an} vs {tn}: dE {d}");
            }
        }
        assert!(closest >= 39.0, "planned minimum 39.5, got {closest}");
    }

    #[test]
    fn the_selected_row_and_the_hover_row_show() {
        let t = BUILT_IN;
        for (name, other) in [
            ("void", t.void),
            ("surface", t.surface),
            ("surface-raised", t.surface_raised),
        ] {
            let d = delta_e76(t.accent_wash, other);
            assert!(d >= 8.0, "accent-wash vs {name}: dE {d}");
        }
        for (name, other) in [
            ("surface", t.surface),
            ("surface-raised", t.surface_raised),
            ("accent-wash", t.accent_wash),
        ] {
            let d = delta_e76(t.surface_overlay, other);
            assert!(d >= 5.0, "surface-overlay vs {name}: dE {d}");
        }
    }

    #[test]
    fn the_error_tint_is_visible_on_the_card() {
        let (_, s) = built();
        assert!(delta_e76(s.danger_soft, s.surface_raised) >= 8.0);
        assert!(delta_e76(s.danger_soft, s.surface) >= 8.0);
    }

    #[test]
    fn the_built_in_backgrounds_are_all_on_one_side() {
        for (name, bg) in backgrounds() {
            assert!(luminance(bg) < 0.18, "{name}");
        }
        assert!(luminance(built().1.danger_soft) < 0.18);
    }

    #[test]
    fn the_hover_background_differs_from_the_others() {
        let (p, s) = built();
        assert_ne!(s.message_hover, p.background);
        assert_ne!(s.message_hover, s.message_highlight);
        assert_ne!(s.message_hover, s.selection);
    }

    #[test]
    fn the_surfaces_differ_and_the_sidebar_is_the_window_background() {
        let (p, s) = built();
        assert_ne!(p.background, s.surface);
        assert_ne!(s.surface, s.surface_raised);
        assert_ne!(s.sidebar, s.surface);
        // The design has no sidebar token: it is `void` (documented).
        assert_eq!(s.sidebar, p.background);
    }

    #[test]
    fn sender_colours_are_made_readable_on_every_surface_they_are_drawn_on() {
        let theme = built_in();
        let (p, s) = built();
        let weak = theme.extended_palette().background.weak.color;
        let surfaces = sender_surfaces(&p, &s, weak);
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
        samples.extend([Color::WHITE, Color::BLACK]);
        samples.extend(surfaces);
        for c in samples {
            // Through the production function, then against each surface.
            let shown = sender_colour(c, &theme);
            for bg in surfaces {
                assert!(contrast(shown, bg) >= 4.5, "{c:?} on {bg:?} -> {shown:?}");
            }
        }
        // A colour that is already readable on all of them is kept exactly.
        let light_red = Color::from_rgb8(0xff, 0x9b, 0x9b);
        assert_eq!(sender_colour(light_red, &theme), light_red);
        // A faint one is lifted toward white.
        let faint = Color::from_rgb8(0x20, 0x18, 0x30);
        assert!(sender_colour(faint, &theme).r > faint.r);
        // The case a background-only check missed: a colour that is readable
        // on the window but not on the accent wash is lifted further.
        let mauve = Color::from_rgb8(0x8a, 0x6f, 0xe0);
        assert!(contrast(mauve, p.background) >= 4.5);
        assert!(contrast(mauve, s.selection) < 4.5);
        assert!(contrast(sender_colour(mauve, &theme), s.selection) >= 4.5);
    }

    /// Every background a message header (and so its shield) can sit on: the
    /// window, the panels, the hovered row, the selected and mentioned rows.
    fn shield_backgrounds(p: &Palette, s: &Semantic) -> [(&'static str, Color); 6] {
        [
            ("void", p.background),
            ("surface", s.surface),
            ("surface-raised", s.surface_raised),
            ("surface-overlay", s.message_hover),
            ("selection", s.selection),
            ("message-highlight", s.message_highlight),
        ]
    }

    #[test]
    fn the_shield_marks_are_readable_and_stay_visible_on_every_row() {
        let (p, s) = built();
        for (name, fill, glyph) in [
            ("red", s.shield_red, s.shield_red_glyph),
            ("grey", s.shield_grey, s.shield_grey_glyph),
        ] {
            let on_fill = contrast(glyph, fill);
            assert!(on_fill >= 4.5, "{name} shield glyph on its fill: {on_fill}");
            for (bg_name, bg) in shield_backgrounds(&p, &s) {
                let c = contrast(fill, bg);
                assert!(c >= 3.0, "{name} shield on {bg_name}: {c}");
            }
        }
    }

    #[test]
    fn the_two_shields_are_told_apart_from_each_other_and_from_the_accent() {
        let (p, s) = built();
        assert_eq!(s.shield_red, p.danger);
        let d = |a, b| delta_e76(a, b);
        assert!(d(s.shield_grey, s.badge) >= 20.0, "grey vs badge");
        assert!(d(s.shield_grey, s.speaking) >= 20.0, "grey vs speaking");
        assert!(d(s.shield_red, s.badge) >= 20.0, "red vs badge");
        assert!(d(s.shield_red, s.shield_grey) >= 20.0, "red vs grey");
        assert!(d(s.shield_grey, s.highlight) >= 20.0, "grey vs mention");
    }

    #[test]
    fn readable_on_every_takes_the_other_end_when_one_end_cannot() {
        let dark = Color::from_rgb8(0x10, 0x10, 0x10);
        let light = Color::from_rgb8(0xf0, 0xf0, 0xf0);
        let mid = Color::from_rgb8(0x80, 0x80, 0x80);
        let shown = readable_on_every(light, &[light]);
        assert!(contrast(shown, light) >= 4.5 && shown.r < light.r);
        assert!(contrast(readable_on_every(mid, &[dark]), dark) >= 4.5);
    }

    #[test]
    fn readable_on_every_falls_back_to_the_better_end_when_neither_works() {
        // A mixed set: no colour reaches 4.5:1 on a near-black and a light
        // grey at once. Most are light, so black is tried first, but white
        // does better over the set (1.7:1 against 1.2:1 at worst).
        let dark = Color::from_rgb8(0x10, 0x10, 0x10);
        let light = Color::from_rgb8(0xc8, 0xc8, 0xc8);
        let set = [dark, light, light];
        let worst = |c: Color| set.iter().map(|&b| contrast(c, b)).fold(f32::MAX, f32::min);
        assert!(worst(Color::WHITE) > worst(Color::BLACK));
        let shown = readable_on_every(Color::from_rgb8(0x80, 0x80, 0x80), &set);
        assert!(worst(shown) < MIN_TEXT_CONTRAST, "the set has no answer");
        assert_eq!(shown, Color::WHITE);
        // The other way round: mostly dark, but black does better (a near
        // white is worse for white than a near black is for black).
        let white = Color::from_rgb8(0xf8, 0xf8, 0xf8);
        let set = [white, dark, dark];
        let shown = readable_on_every(Color::from_rgb8(0x80, 0x80, 0x80), &set);
        assert_eq!(shown, Color::BLACK);
    }

    /// What in `code` builds a theme: `Theme::custom`, `Theme::Custom`,
    /// `<Theme>::custom`, and each of them through an alias
    /// (`use iced::Theme as X;`, `type X = iced::Theme;`). Comments and
    /// strings are not code.
    fn theme_builders(code: &str) -> Vec<String> {
        let masked = source_scan::mask(code);
        let mut names = vec!["Theme".to_string()];
        let ident = |s: &str| -> String {
            s.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect()
        };
        let mut rest = masked.as_str();
        while let Some(at) = rest.find("Theme as ") {
            rest = &rest[at + "Theme as ".len()..];
            let alias = ident(rest);
            if !alias.is_empty() {
                names.push(alias);
            }
        }
        // `type Skin = iced::Theme;`
        let mut rest = masked.as_str();
        while let Some(at) = rest.find("type ") {
            let before = rest[..at].chars().last();
            rest = &rest[at + "type ".len()..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let alias = ident(rest.trim_start());
            let after = rest.trim_start()[alias.len()..].trim_start();
            let Some(target) = after.strip_prefix('=') else {
                continue;
            };
            let target = target.split(';').next().unwrap_or("").trim();
            let last = target.rsplit("::").next().unwrap_or("").trim();
            if !alias.is_empty() && names.iter().any(|n| n == last) {
                names.push(alias);
            }
        }
        // Whitespace next to a `:` goes, so `iced::Theme\n    ::custom` reads
        // as `iced::Theme::custom`.
        let chars: Vec<char> = masked.chars().collect();
        let mut tight = String::new();
        let mut i = 0;
        while i < chars.len() {
            if !chars[i].is_whitespace() {
                tight.push(chars[i]);
                i += 1;
                continue;
            }
            // One whitespace run, looked at once.
            let end = chars[i..]
                .iter()
                .position(|c| !c.is_whitespace())
                .map_or(chars.len(), |p| i + p);
            let glued = tight.ends_with([':', '<']) || chars.get(end) == Some(&':');
            if !glued && end < chars.len() {
                tight.push(' ');
            }
            i = end;
        }
        let mut hits = Vec::new();
        for name in &names {
            for pattern in [format!("{name}::custom"), format!("{name}::Custom")] {
                if tight.contains(&pattern) {
                    hits.push(pattern);
                }
            }
        }
        // `<Theme>::custom`, `<iced::Theme as Default>::Custom`.
        for variant in [">::custom", ">::Custom"] {
            for (at, _) in tight.match_indices(variant) {
                let inner = tight[..at].rsplit('<').next().unwrap_or("");
                let tokens = inner.split(|c: char| !(c.is_alphanumeric() || c == '_'));
                if tokens.into_iter().any(|t| names.iter().any(|n| n == t)) {
                    hits.push(format!("<..>{}", &variant[1..]));
                }
            }
        }
        // `iced::theme::Custom` is the type behind `Theme::Custom`.
        for pattern in ["theme::Custom", "Custom::new"] {
            if tight.contains(pattern) {
                hits.push(pattern.to_string());
            }
        }
        hits
    }

    #[test]
    fn the_theme_scan_sees_aliases_and_the_enum_variant() {
        assert!(!theme_builders("let t = Theme::custom(n, p);").is_empty());
        assert!(!theme_builders("let t = iced::Theme::Custom(Arc::new(x));").is_empty());
        let aliased = "use iced::Theme as Skin;\nlet t = Skin::custom(n, p);";
        assert!(!theme_builders(aliased).is_empty());
        let aliased = "use iced::Theme as Skin;\nlet t = Skin::Custom(c);";
        assert!(!theme_builders(aliased).is_empty());
        assert!(theme_builders("let t = Theme::Dark; Theme::ALL;").is_empty());
    }

    #[test]
    fn the_theme_scan_has_no_holes() {
        for bad in [
            "type Skin = iced::Theme;\nlet t = Skin::custom(n, p);",
            "type Skin = Theme;\nlet t = Skin::Custom(c);",
            "let t = <Theme>::custom(n, p);",
            "let t = <iced::Theme>::Custom(c);",
            "let t = <iced::Theme as Default>::custom(n, p);",
            "let t = iced::Theme\n    ::custom(n, p);",
        ] {
            assert!(!theme_builders(bad).is_empty(), "missed: {bad}");
        }
        // Comments and strings are not code.
        let quiet = "// Theme::custom(n, p)\nlet s = \"Theme::Custom\";\n/* Theme::custom */";
        assert!(
            theme_builders(quiet).is_empty(),
            "{:?}",
            theme_builders(quiet)
        );
        // An unrelated alias is not a builder.
        assert!(theme_builders("type Id = u64;\nlet x = Id::custom(1);").is_empty());
    }

    /// The app builds its themes in one place: no other code builds a
    /// `Theme::custom` / `Theme::Custom`.
    #[test]
    fn only_theme_rs_builds_themes() {
        let (mut hits, mut scanned, mut skipped) = (Vec::new(), 0, 0);
        // The full path, not a suffix: `ui/theme.rs` would not be exempt.
        let theme_rs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/theme.rs");
        for (path, text) in source_scan::sources() {
            if path == theme_rs {
                skipped += 1;
                continue;
            }
            let code = source_scan::production_part(&text);
            scanned += 1;
            for found in theme_builders(&code) {
                hits.push(format!("{}: {found}", path.display()));
            }
        }
        assert!(hits.is_empty(), "builds a theme: {hits:?}");
        assert_eq!(skipped, 1, "exactly crates/app/src/theme.rs is exempt");

        // The scan really covered the code: many files, and the big ones
        // nearly whole (a cut at an early `#[cfg(test)]` would fail these).
        assert!(scanned >= 40, "only {scanned} files scanned");
        let pane = include_str!("screens/timeline_pane.rs");
        let code = source_scan::production_part(pane);
        // Test modules are blanked, not cut, so count what is left.
        let kept = code.chars().filter(|c| !c.is_whitespace()).count();
        assert!(kept > 30_000, "timeline_pane scanned {kept}");
        assert!(
            code.contains("Echo::Failed { recoverable: true } =>"),
            "the end of the production code is scanned"
        );
        assert!(!code.contains("fn the_upload_percent_is_a_whole_number"));
    }

    // ---- Task 2: resolve, readable_over, the registry ----------------------

    fn with(changes: &[(&str, u32)]) -> Tokens {
        let mut t = BUILT_IN;
        for (name, v) in changes {
            assert!(
                t.set(
                    name,
                    Color::from_rgb8((*v >> 16) as u8, (*v >> 8) as u8, *v as u8)
                ),
                "{name}"
            );
        }
        t
    }

    fn assert_same_side_and_audit(r: &Resolved, what: &str) {
        // An independent check of what every accepted theme promises, with
        // the numbers written out (not the constants `resolve` uses).
        let (p, s) = (&r.palette, &r.semantic);
        let bgs = [
            p.background,
            s.surface,
            s.surface_raised,
            s.message_hover,
            s.selection,
        ];
        let dark = luminance(p.background) < 0.18;
        for bg in bgs.iter().chain([&s.danger_soft]) {
            assert_eq!(luminance(*bg) < 0.18, dark, "{what}: sides");
        }
        let text = [
            ("ink", p.text),
            ("muted", s.muted),
            ("primary", p.primary),
            ("danger", p.danger),
            ("success", p.success),
            ("warning", p.warning),
        ];
        for (n, fg) in text {
            for bg in bgs {
                let c = contrast(fg, bg);
                assert!(c >= 4.5, "{what}: {n} on {bg:?}: {c}");
            }
        }
        for bg in bgs {
            assert!(
                contrast(p.text, mix(bg, p.text, 0.15)) >= 4.5,
                "{what}: pressed"
            );
        }
        for (n, fg) in [("ink", p.text), ("muted", s.muted), ("danger", p.danger)] {
            assert!(
                contrast(fg, s.danger_soft) >= 4.5,
                "{what}: {n} on the tint"
            );
        }
        for fill in [s.badge, s.highlight] {
            assert!(
                contrast(s.badge_text, fill) >= 4.5,
                "{what}: label on accent"
            );
        }
        for bg in bgs {
            assert!(contrast(s.focus, bg) >= 3.0, "{what}: focus");
        }
        let grey_box = mix(s.surface_raised, s.border, 0.35);
        assert!(contrast(s.badge, grey_box) >= 3.0, "{what}: disabled box");
        for bg in &bgs[..3] {
            assert!(contrast(s.badge, *bg) >= 3.0, "{what}: ring");
            assert!(contrast(s.border, *bg) >= 3.0, "{what}: outline {:?}", bg);
        }
        let status = [p.danger, p.success, p.warning, p.primary];
        for (i, a) in status.iter().enumerate() {
            for b in &status[i + 1..] {
                assert!(delta_e76(*a, *b) >= 20.0, "{what}: status pair");
            }
            for t in [p.text, s.muted] {
                assert!(delta_e76(*a, t) >= 20.0, "{what}: status vs text");
            }
        }
        for other in [p.danger, p.success, p.warning, p.text, s.muted, s.border] {
            assert!(delta_e76(s.focus, other) >= 20.0, "{what}: focus");
        }
        for other in [p.danger, s.muted] {
            assert!(delta_e76(s.badge, other) >= 20.0, "{what}: badge");
        }
        for bg in &bgs[..3] {
            assert!(delta_e76(s.selection, *bg) >= 8.0, "{what}: selection");
        }
        for other in [s.surface, s.surface_raised, s.selection] {
            assert!(delta_e76(s.message_hover, other) >= 5.0, "{what}: hover");
        }
        // The shields.
        for (fill, glyph) in [
            (s.shield_red, s.shield_red_glyph),
            (s.shield_grey, s.shield_grey_glyph),
        ] {
            assert!(contrast(glyph, fill) >= 4.5, "{what}: shield glyph");
            for bg in bgs {
                assert!(contrast(fill, bg) >= 3.0, "{what}: shield fill");
            }
        }
        assert!(
            delta_e76(s.shield_red, s.shield_grey) >= 20.0,
            "{what}: shields"
        );
        // Sender colours reach 4.5:1 on every surface they sit on.
        let weak = iced::theme::palette::Extended::generate(*p)
            .background
            .weak
            .color;
        let surfaces = sender_surfaces(p, s, weak);
        for probe in [
            Color::WHITE,
            Color::BLACK,
            color!(0x808080),
            color!(0xff0000),
            color!(0x3060ff),
        ] {
            let shown = readable_on_every(probe, &surfaces);
            for bg in surfaces {
                assert!(
                    contrast(shown, bg) >= 4.5,
                    "{what}: sender colour {probe:?}"
                );
            }
        }
    }

    #[test]
    fn the_built_in_theme_resolves_without_adjustment() {
        let r = resolve(&BUILT_IN).expect("accepted");
        assert!(r.report.is_empty(), "{:?}", r.report);
        let b = built_in_resolved();
        assert_eq!(
            (r.palette, r.semantic, r.dark),
            (b.palette, b.semantic, b.dark)
        );
        assert_same_side_and_audit(&r, "built-in");
    }

    #[test]
    fn a_faint_text_colour_is_lifted_and_named() {
        let r = resolve(&with(&[("ink", 0x7a7590)])).expect("accepted");
        assert_eq!(r.report, ["ink"]);
        assert!(contrast(r.palette.text, r.palette.background) >= 4.5);
        assert!(r.palette.text.r > color!(0x7a7590).r);
        assert_same_side_and_audit(&r, "faint");
    }

    #[test]
    fn colours_that_cannot_be_read_are_refused_when_the_backgrounds_mix() {
        let r = resolve(&with(&[("void", 0xfaf8ff)]));
        assert_eq!(r, Err(Refusal::MixedBackgrounds));
        // A tint that crosses the line from the other side.
        let r = resolve(&with(&[("surface-overlay", 0xe0e0e0)]));
        assert_eq!(r, Err(Refusal::MixedBackgrounds));
    }

    #[test]
    fn distinctness_covers_the_status_colours_the_text_and_the_focus_ring() {
        let refused =
            |changes: &[(&str, u32)]| resolve(&with(changes)) == Err(Refusal::StatusAlike);
        assert!(refused(&[("live", 0xefebfa)]), "live == ink");
        assert!(refused(&[("live", 0x5ee0c8)]), "live == ok");
        assert!(refused(&[("warn", 0xff7a8c)]), "warn near live");
        assert!(refused(&[("ok", 0xa69fc4)]), "ok == ink-muted");
        assert!(refused(&[("accent-bright", 0xff6b86)]), "info == live");
        assert!(refused(&[("focus", 0xefebfa)]), "focus == ink");
        assert!(refused(&[("focus", 0xff6b86)]), "focus == live");
        assert!(refused(&[("focus", 0x5ee0c8)]), "focus == ok");
        assert!(refused(&[("focus", 0xffb547)]), "focus == warn");
        assert!(refused(&[("focus", 0xa69fc4)]), "focus == ink-muted");
        assert!(
            refused(&[("focus", 0x8a80c0)]),
            "focus close to the outline"
        );
        assert!(refused(&[("accent", 0xff6b86)]), "the badge == live");
        assert!(refused(&[("accent", 0xa69fc4)]), "the badge == ink-muted");
        // The accents are siblings by design: the built-in focus is 17 from
        // the bright accent, and Paper uses one colour for all three.
        assert!(
            resolve(&with(&[("focus", 0xb794ff)])).is_ok(),
            "focus == accent-bright"
        );
    }

    #[test]
    fn the_selected_and_hovered_rows_must_show() {
        let refused =
            |changes: &[(&str, u32)]| resolve(&with(changes)) == Err(Refusal::SelectionHidden);
        assert!(refused(&[("accent-wash", 0x110e1c)]));
        assert!(refused(&[("accent-wash", 0x0a0812)]));
        assert!(refused(&[("surface-overlay", 0x110e1c)]));
        assert!(refused(&[("surface-overlay", 0x241a45)]));
    }

    #[test]
    fn the_outline_the_ring_and_the_focus_colour_are_lifted_when_faint() {
        for (name, faint) in [
            ("line-strong", 0x2e2748),
            ("accent", 0x5a3fb0),
            ("focus", 0x2040c0),
        ] {
            let r = resolve(&with(&[(name, faint)])).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(r.report, [name], "{name}");
            assert_same_side_and_audit(&r, name);
        }
    }

    #[test]
    fn on_accent_is_solved_even_when_one_label_colour_cannot_serve_both_fills() {
        // A dark ring colour and a bright accent: no single label reaches
        // 4.5:1 on both until the ring is lifted.
        let r =
            resolve(&with(&[("accent", 0x4a3a90), ("accent-bright", 0xd8b4fe)])).expect("accepted");
        assert!(contrast(r.semantic.badge_text, r.semantic.badge) >= 4.5);
        assert!(contrast(r.semantic.badge_text, r.semantic.highlight) >= 4.5);
        assert!(r.report.contains(&"accent"), "{:?}", r.report);
        assert_same_side_and_audit(&r, "on-accent");
    }

    #[test]
    fn readable_over_lifts_toward_the_side_of_the_backgrounds() {
        let dark = [color!(0x0a0812), color!(0x241a45)];
        let kept = color!(0xefebfa);
        assert_eq!(readable_over(kept, &dark, 4.5), kept, "already readable");
        let lifted = readable_over(color!(0x404060), &dark, 4.5);
        assert!(lifted.r > color!(0x404060).r);
        assert!(dark.iter().all(|&b| contrast(lifted, b) >= 4.5));
        let light = [color!(0xfaf8ff), color!(0xddd2fb)];
        let lowered = readable_over(color!(0xb0b0c0), &light, 4.5);
        assert!(lowered.r < color!(0xb0b0c0).r);
        assert!(light.iter().all(|&b| contrast(lowered, b) >= 4.5));
        // A lower least contrast moves less.
        let some = readable_over(color!(0x404060), &dark, 3.0);
        assert!(some.r < lifted.r);
        // Neither end can serve a mixed set: the better end, still short.
        let mixed = [color!(0x101010), color!(0xc8c8c8)];
        let shown = readable_over(color!(0x808080), &mixed, 4.5);
        assert!(mixed.iter().any(|&b| contrast(shown, b) < 4.5));
        assert_eq!(readable_over(color!(0x808080), &[], 4.5), color!(0x808080));
    }

    #[test]
    fn a_non_opaque_or_broken_colour_is_replaced_by_the_built_in_one() {
        let mut t = BUILT_IN;
        t.ink = Color { a: 0.5, ..t.ink };
        t.ok = Color {
            r: f32::NAN,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        };
        let r = resolve(&t).expect("accepted");
        assert_eq!(r.palette.text, BUILT_IN.ink);
        assert_eq!(r.palette.success, BUILT_IN.ok);
        assert_eq!(r.report, ["ink", "ok"]);
    }

    #[test]
    fn semantic_finds_the_colours_of_each_registered_theme() {
        let paper = resolve(&with(&[
            ("void", 0xfaf8ff),
            ("surface", 0xf3f0fb),
            ("surface-raised", 0xebe6f7),
            ("surface-overlay", 0xdcd4f1),
            ("accent-wash", 0xddd2fb),
            ("ink", 0x0a0812),
            ("ink-muted", 0x4a4366),
            ("accent", 0x4b23b8),
            ("on-accent", 0xffffff),
            ("accent-bright", 0x4b23b8),
            ("focus", 0x4b23b8),
            ("live", 0xa3102f),
            ("ok", 0x005a4f),
            ("warn", 0x7a4200),
        ]))
        .expect("Paper-like");
        assert!(paper.report.is_empty(), "{:?}", paper.report);
        let mut light = paper.clone();
        light.name = "Pair".to_owned();
        let mut dark = built_in_resolved();
        dark.name = "Pair".to_owned();
        let (tl, td) = (theme_of(&light), theme_of(&dark));
        assert_eq!(semantic(&tl), light.semantic);
        assert_eq!(semantic(&td), dark.semantic, "same name, other mode");
        assert_ne!(semantic(&tl), semantic(&td));
        // Registering again with other colours replaces the entry.
        let mut again = light.clone();
        again.semantic.surface = color!(0x123456);
        let t2 = theme_of(&again);
        assert_eq!(semantic(&t2).surface, color!(0x123456));
        // An unknown theme gets the built-in colours.
        assert_eq!(semantic(&Theme::Light), built_in_resolved().semantic);
    }

    #[test]
    fn the_registry_is_bounded_and_the_oldest_goes_first() {
        // On a local list: the real one is shared by tests running together.
        assert_eq!(
            REGISTRY_CAP,
            1 + 32 * 2,
            "the built-in plus 32 files of 2 modes"
        );
        let mut list = Vec::new();
        for i in 0..500u32 {
            let mut r = built_in_resolved();
            r.name = format!("Spam {i}");
            r.palette.primary = Color::from_rgb8((i % 256) as u8, (i / 256) as u8, 7);
            push_bounded(&mut list, REGISTRY_CAP, &r);
            assert!(list.len() <= REGISTRY_CAP);
        }
        assert_eq!(list.len(), REGISTRY_CAP);
        assert_eq!(list.last().map(|e| e.0.as_str()), Some("Spam 499"));
        assert_eq!(list.first().map(|e| e.0.as_str()), Some("Spam 435"));
        // The built-in colours never depend on the list.
        assert_eq!(semantic(&built_in()), built_in_resolved().semantic);
    }

    #[test]
    fn the_test_themes_are_twicecat_daylight_the_duo_pair_faint_and_paper() {
        let themes = test_themes();
        let names: Vec<String> = themes.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            ["Twicecat", "Daylight", "Duo", "Duo", "Faint", "Paper"]
        );
        let light = themes
            .iter()
            .filter(|t| !t.extended_palette().is_dark)
            .count();
        assert_eq!(light, 3, "Daylight, Paper and the light Duo");
        for t in &themes {
            let sem = semantic(t);
            assert!(contrast(t.palette().text, sem.surface) >= 4.5, "{t}");
        }
        let faint = themes
            .iter()
            .find(|t| t.to_string() == "Faint")
            .expect("Faint");
        assert!(contrast(faint.palette().text, faint.palette().background) >= 4.5);
    }

    #[test]
    fn every_accepted_random_theme_keeps_every_promise() {
        let mut rng = Lcg(0x5eed_1234_abcd_ef01);
        let (mut accepted, mut refused) = (0, 0);
        let mut adjusted = 0;
        for _ in 0..2000 {
            let t = rng.tokens();
            match resolve(&t) {
                Ok(r) => {
                    accepted += 1;
                    adjusted += usize::from(!r.report.is_empty());
                    assert_same_side_and_audit(&r, &format!("{t:?}"));
                }
                Err(_) => refused += 1,
            }
        }
        assert!(accepted >= 100, "accepted {accepted}, refused {refused}");
        assert!(adjusted >= 50, "the adjustments were exercised: {adjusted}");
        assert!(refused >= 50, "the refusals were exercised: {refused}");
    }

    #[test]
    fn every_kit_independent_shield_of_a_test_theme_is_readable() {
        for theme in test_themes() {
            let (p, s) = (theme.palette(), semantic(&theme));
            for (fill, glyph) in [
                (s.shield_red, s.shield_red_glyph),
                (s.shield_grey, s.shield_grey_glyph),
            ] {
                assert!(contrast(glyph, fill) >= 4.5, "{theme}");
                assert!(contrast(fill, p.background) >= 3.0, "{theme}");
            }
        }
    }

    // ---- pick_look ----

    fn look(name: &str, dark: bool) -> Resolved {
        Resolved {
            name: name.to_owned(),
            dark,
            both_modes: true,
            ..built_in_resolved()
        }
    }

    #[test]
    fn a_pair_follows_the_choice_and_the_system_mode() {
        let (light, dark) = (look("Duo", false), look("Duo", true));
        let looks = [&dark, &light];
        let cases = [
            (ThemeChoice::Light, Mode::Light, false),
            (ThemeChoice::Light, Mode::Dark, false),
            (ThemeChoice::Light, Mode::None, false),
            (ThemeChoice::Dark, Mode::Light, true),
            (ThemeChoice::Dark, Mode::Dark, true),
            (ThemeChoice::Dark, Mode::None, true),
            (ThemeChoice::System, Mode::Light, false),
            (ThemeChoice::System, Mode::Dark, true),
            (ThemeChoice::System, Mode::None, false),
        ];
        for (choice, system, want_dark) in cases {
            let got = pick_look(&looks, choice, system).expect("a look");
            assert_eq!(got.dark, want_dark, "{choice:?} {system:?}");
        }
    }

    #[test]
    fn a_single_look_is_returned_whatever_the_arguments_are() {
        for dark in [false, true] {
            let only = look("Solo", dark);
            for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                for system in [Mode::Light, Mode::Dark, Mode::None] {
                    let got = pick_look(&[&only], choice, system).expect("a look");
                    assert_eq!(got.dark, dark, "{choice:?} {system:?}");
                }
            }
        }
        assert!(pick_look(&[], ThemeChoice::System, Mode::Dark).is_none());
    }
}
