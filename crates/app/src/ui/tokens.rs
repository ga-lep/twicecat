//! Design tokens: spacing, radii, type sizes and the two densities.
//!
//! Every screen takes its sizes from here instead of writing numbers inline.

/// Spacing: the smallest gap, inside a tight group.
pub const XS: f32 = 4.0;
/// Spacing: between related controls.
pub const S: f32 = 8.0;
/// Spacing: the default padding.
pub const M: f32 = 12.0;
/// Spacing: between sections of a card.
pub const L: f32 = 16.0;
/// Spacing: around a page or a dialog.
pub const XL: f32 = 24.0;

/// Corner radius of tags and small fields.
pub const SMALL: f32 = 4.0;
/// Corner radius of buttons and banners.
pub const MEDIUM: f32 = 8.0;
/// Corner radius of cards and dialogs.
pub const LARGE: f32 = 12.0;
/// A radius that rounds a box into a pill or a circle, whatever its size.
pub const ROUND: f32 = 999.0;

/// Type size of captions, timestamps and hints.
pub const CAPTION: f32 = 12.0;
/// Type size of running text.
pub const BODY: f32 = 14.0;
/// Type size of emphasised running text.
pub const BODY_LARGE: f32 = 16.0;
/// Type size of a section title.
pub const TITLE: f32 = 18.0;
/// Type size of a page heading.
pub const HEADLINE: f32 = 24.0;
/// Type size of the biggest display text.
pub const DISPLAY: f32 = 32.0;

/// How much room the lists and bars take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    /// The default: roomy rows.
    #[default]
    Comfortable,
    /// Tighter rows and gaps, to show more at once.
    Compact,
}

impl Density {
    /// Padding of panels and bars.
    pub const fn pad(self) -> f32 {
        match self {
            Self::Comfortable => M,
            Self::Compact => S,
        }
    }

    /// Gap between the items of a list or a column.
    pub const fn gap(self) -> f32 {
        match self {
            Self::Comfortable => S,
            Self::Compact => XS,
        }
    }

    /// Height of a room list row, in pixels.
    pub const fn room_row_height(self) -> f32 {
        match self {
            Self::Comfortable => 64.0,
            Self::Compact => 44.0,
        }
    }

    /// Vertical padding of a timeline row. Compact trims this only; the
    /// row's structure does not change.
    pub const fn timeline_row_pad(self) -> f32 {
        match self {
            Self::Comfortable => 4.0,
            Self::Compact => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scales_are_the_documented_ones() {
        assert_eq!([XS, S, M, L, XL], [4.0, 8.0, 12.0, 16.0, 24.0]);
        assert_eq!([SMALL, MEDIUM, LARGE], [4.0, 8.0, 12.0]);
        assert_eq!(
            [CAPTION, BODY, BODY_LARGE, TITLE, HEADLINE, DISPLAY],
            [12.0, 14.0, 16.0, 18.0, 24.0, 32.0]
        );
        const { assert!(ROUND > LARGE * 10.0) };
    }

    #[test]
    fn density_sets_the_row_heights_and_trims_only_the_vertical_padding() {
        assert_eq!(Density::Comfortable.room_row_height(), 64.0);
        assert_eq!(Density::Compact.room_row_height(), 44.0);
        assert_eq!(
            Density::Comfortable.room_row_height(),
            crate::room_list::ROW_HEIGHT
        );
        assert!(Density::Compact.pad() < Density::Comfortable.pad());
        assert!(Density::Compact.gap() < Density::Comfortable.gap());
        assert!(Density::Compact.timeline_row_pad() < Density::Comfortable.timeline_row_pad());
        assert_eq!(Density::default(), Density::Comfortable);
    }
}
