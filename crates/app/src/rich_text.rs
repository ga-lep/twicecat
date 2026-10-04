//! Formatted message bodies as Iced widgets (plan Q19, Q20).
//!
//! The core hands over an SDK-free [`RichText`]. [`spans_for`] turns inline
//! runs into plain [`SpanModel`]s (testable without a renderer), [`to_span`]
//! gives them colours, and [`blocks_view`] builds the widgets.
//!
//! Every text passes through `text_safety` and a per-message budget
//! ([`Walk`]), so a hostile message cannot grow without bound. Rich text can
//! only pick bold, italic, strike, underline, code, link and pill looks. None
//! of them is the muted colour or the small size of the time next to a
//! sender name, so a body cannot pass for a message header line.

use std::collections::HashSet;
use std::fmt;

use iced::font::{Style as FontStyle, Weight};
use iced::widget::text::{Span, Wrapping};
use iced::widget::{Column, container, rich_text, row, rule, text};
use iced::{Background, Border, Color, Element, Fill, Font, Length, Pixels, Shrink, Theme};
use tessera_core::api::{Block, Inline, MentionTarget, RichText, TextStyle, UserId};

use crate::links::needs_confirmation;
use crate::state::Message;
use crate::text_safety::{
    ELLIPSIS, MAX_BODY_CHARS, MAX_NAME_CHARS, multi_line_capped, name_or_placeholder,
};
use crate::theme;

/// Spoilers the user revealed, as (item ID, spoiler index).
pub type Revealed = HashSet<(String, usize)>;

/// Most spoilers remembered as revealed.
pub const MAX_REVEALED: usize = 1024;

/// Characters drawn per message, over all blocks.
pub const MAX_MESSAGE_CHARS: usize = 4 * MAX_BODY_CHARS;

/// Lines drawn per message, over all blocks.
pub const MAX_MESSAGE_LINES: usize = 200;

/// Deepest block nesting drawn; deeper content is cut.
pub const MAX_DEPTH: usize = 32;

/// What a click on rich text does.
#[derive(Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A web or mail link.
    Url {
        /// The target, still to be checked by `check_link_url`.
        url: String,
        /// The visible text names another host than the target.
        confirm: bool,
    },
    /// A user pill.
    User(UserId),
    /// A room pill: an ID or an alias.
    Room(String),
    /// A spoiler pill; `item` and `index` say which one.
    Spoiler {
        /// ID of the timeline item.
        item: String,
        /// Position among the item's spoilers.
        index: usize,
    },
}

// Manual impl: URLs, IDs and aliases are user content.
impl fmt::Debug for LinkTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Url { .. } => "LinkTarget::Url(<redacted>)",
            Self::User(_) => "LinkTarget::User(<redacted>)",
            Self::Room(_) => "LinkTarget::Room(<redacted>)",
            Self::Spoiler { .. } => "LinkTarget::Spoiler",
        })
    }
}

/// How a span looks beyond its [`TextStyle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Normal text.
    Plain,
    /// A link.
    Link,
    /// A mention pill; `own` when it names the signed-in user.
    Mention {
        /// Names the signed-in user.
        own: bool,
    },
    /// A hidden spoiler.
    SpoilerPill,
    /// A revealed spoiler's text.
    SpoilerOpen,
}

/// A run of text with one look, before colours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanModel {
    /// Sanitised text.
    pub text: String,
    /// Character styling.
    pub style: TextStyle,
    /// Look.
    pub look: Look,
    /// What a click does.
    pub link: Option<LinkTarget>,
}

/// What [`spans_for`] needs to know about the item.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    /// The signed-in user.
    pub own_user: Option<&'a UserId>,
    /// ID of the timeline item.
    pub item: &'a str,
    /// Spoilers already revealed.
    pub revealed: &'a Revealed,
}

/// Running state of one message: what is left of its budget, and how many
/// spoilers were met.
#[derive(Clone, Copy, Debug)]
pub struct Walk {
    chars_left: usize,
    lines_left: usize,
    spoilers: usize,
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            chars_left: MAX_MESSAGE_CHARS,
            lines_left: MAX_MESSAGE_LINES,
            spoilers: 0,
        }
    }
}

impl Walk {
    fn exhausted(&self) -> bool {
        self.chars_left == 0 || self.lines_left == 0
    }

    /// Takes the budget for `text`, which may span lines.
    fn take(&mut self, text: &str) -> String {
        if self.exhausted() {
            return String::new();
        }
        // Only what can be shown is sanitised, however long the input is.
        let head: String = text
            .chars()
            .take(self.chars_left.saturating_add(1))
            .collect();
        let out = multi_line_capped(&head, self.chars_left, self.lines_left);
        self.spend(&out);
        out
    }

    /// Takes the budget for a name or label: one line, at most
    /// [`MAX_NAME_CHARS`] characters, never empty.
    fn take_label(&mut self, text: &str) -> String {
        if self.exhausted() {
            return String::new();
        }
        let head: String = text.chars().take(MAX_NAME_CHARS + 1).collect();
        let out = name_or_placeholder(&head, MAX_NAME_CHARS.min(self.chars_left));
        self.spend(&out);
        out
    }

    fn spend(&mut self, out: &str) {
        self.chars_left = self.chars_left.saturating_sub(out.chars().count());
        let breaks = out.matches('\n').count();
        self.lines_left = self.lines_left.saturating_sub(breaks);
    }

    /// A block starts on a new line.
    fn start_block(&mut self) {
        self.lines_left = self.lines_left.saturating_sub(1);
    }
}

/// How many spoilers `inlines` hold, nested ones included.
fn count_spoilers(inlines: &[Inline]) -> usize {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Spoiler(inner) => 1 + count_spoilers(inner),
            _ => 0,
        })
        .sum()
}

/// The spans for `inlines`.
pub fn spans_for(inlines: &[Inline], ctx: &Ctx<'_>, walk: &mut Walk) -> Vec<SpanModel> {
    let mut out = Vec::new();
    push_spans(inlines, ctx, walk, &mut out);
    out
}

fn push_spans(inlines: &[Inline], ctx: &Ctx<'_>, walk: &mut Walk, out: &mut Vec<SpanModel>) {
    for inline in inlines {
        match inline {
            Inline::Text { text, style } => {
                let text = walk.take(text);
                if !text.is_empty() {
                    out.push(SpanModel {
                        text,
                        style: *style,
                        look: Look::Plain,
                        link: None,
                    });
                }
            }
            Inline::Link { text, url, style } => {
                // Judge the text the user will see, not the raw one.
                let shown = walk.take(text);
                if !shown.is_empty() {
                    // The budget may cut the label (`bank.example` + more):
                    // judge the whole text as well as what is shown.
                    let confirm = needs_confirmation(&shown, url) || needs_confirmation(text, url);
                    let text = shown;
                    out.push(SpanModel {
                        text,
                        style: *style,
                        look: Look::Link,
                        link: Some(LinkTarget::Url {
                            url: url.clone(),
                            confirm,
                        }),
                    });
                }
            }
            Inline::Mention { target, label } => {
                let label = walk.take_label(label);
                if label.is_empty() {
                    continue;
                }
                let (own, link) = match target {
                    MentionTarget::User(user) => {
                        (ctx.own_user == Some(user), LinkTarget::User(user.clone()))
                    }
                    MentionTarget::Room(room) => (false, LinkTarget::Room(room.clone())),
                };
                out.push(SpanModel {
                    text: label,
                    style: TextStyle::default(),
                    look: Look::Mention { own },
                    link: Some(link),
                });
            }
            Inline::LineBreak => {
                let text = walk.take("\n");
                if !text.is_empty() {
                    out.push(SpanModel {
                        text,
                        style: TextStyle::default(),
                        look: Look::Plain,
                        link: None,
                    });
                }
            }
            Inline::Spoiler(inner) => {
                let index = walk.spoilers;
                walk.spoilers += 1;
                let toggle = LinkTarget::Spoiler {
                    item: ctx.item.to_owned(),
                    index,
                };
                if ctx.revealed.contains(&(ctx.item.to_owned(), index)) {
                    let from = out.len();
                    push_spans(inner, ctx, walk, out);
                    for span in &mut out[from..] {
                        if span.look == Look::Plain {
                            span.look = Look::SpoilerOpen;
                            span.link = Some(toggle.clone());
                        }
                    }
                } else {
                    // Nested spoilers keep their numbers while hidden.
                    walk.spoilers += count_spoilers(inner);
                    out.push(SpanModel {
                        text: "Spoiler".to_owned(),
                        style: TextStyle::default(),
                        look: Look::SpoilerPill,
                        link: Some(toggle),
                    });
                }
            }
        }
    }
}

/// The font for a style: bold and italic combine, code is monospace.
fn font_for(style: TextStyle) -> Option<Font> {
    if !(style.bold || style.italic || style.code) {
        return None;
    }
    let base = if style.code {
        Font::MONOSPACE
    } else {
        Font::DEFAULT
    };
    Some(Font {
        weight: if style.bold {
            Weight::Bold
        } else {
            base.weight
        },
        style: if style.italic {
            FontStyle::Italic
        } else {
            base.style
        },
        ..base
    })
}

fn pill(background: Color, radius: f32) -> (Background, Border) {
    (
        Background::Color(background),
        Border {
            radius: radius.into(),
            ..Border::default()
        },
    )
}

/// A span with colours for `theme`.
///
/// Colours come from the palette. The muted colour of a header's time is
/// never used here.
pub fn to_span(model: &SpanModel, theme: &Theme) -> Span<'static, LinkTarget> {
    let semantic = theme::semantic(theme);
    let weak = theme.extended_palette().background.weak.color;
    let mut span: Span<'static, LinkTarget> = Span::new(model.text.clone())
        .font_maybe(font_for(model.style))
        .strikethrough(model.style.strike)
        .underline(model.style.underline)
        .link_maybe(model.link.clone());
    if model.style.code {
        let (background, border) = pill(weak, 3.0);
        span = span.background(background).border(border);
    }
    match model.look {
        Look::Plain => {}
        Look::Link => {
            span = span.color(theme.palette().primary).underline(true);
        }
        Look::Mention { own } => {
            let (fill, color) = if own {
                (semantic.highlight, semantic.highlight_text)
            } else {
                (semantic.selection, theme.palette().primary)
            };
            let (background, border) = pill(fill, 8.0);
            span = span
                .color(color)
                .background(background)
                .border(border)
                .padding([0.0, 4.0]);
        }
        Look::SpoilerPill => {
            let (background, border) = pill(semantic.badge, 8.0);
            span = span
                .color(semantic.badge_text)
                .background(background)
                .border(border)
                .padding([0.0, 4.0]);
        }
        Look::SpoilerOpen => {
            let (background, border) = pill(weak, 3.0);
            span = span.background(background).border(border);
        }
    }
    span
}

/// The marker of list item `index`: a bullet, or the number counted from
/// `start` (default 1).
pub fn list_marker(ordered: bool, start: Option<u64>, index: usize) -> String {
    if ordered {
        let number = start
            .unwrap_or(1)
            .saturating_add(u64::try_from(index).unwrap_or(u64::MAX));
        format!("{number}.")
    } else {
        "\u{2022}".to_owned()
    }
}

/// Text size of a heading, by level (1 to 6); clamped, and above the size of
/// body text and sender names (16), so a heading is not a sender line.
pub fn heading_size(level: u8) -> f32 {
    match level {
        0 | 1 => 26.0,
        2 => 22.0,
        3 => 20.0,
        4 => 18.5,
        5 => 17.5,
        _ => 17.0,
    }
}

/// The widgets for `content`, within the budget of one message.
pub fn blocks_view<'a>(content: &RichText, ctx: &Ctx<'_>, theme: &Theme) -> Element<'a, Message> {
    blocks_column(&content.blocks, ctx, theme, &mut Walk::default(), 0)
}

fn paragraph<'a>(
    models: &[SpanModel],
    theme: &Theme,
    size: Option<f32>,
) -> Option<Element<'a, Message>> {
    if models.is_empty() {
        return None;
    }
    let spans: Vec<Span<'static, LinkTarget>> =
        models.iter().map(|model| to_span(model, theme)).collect();
    let mut rich = rich_text(spans)
        .on_link_click(Message::LinkClicked)
        .wrapping(Wrapping::WordOrGlyph)
        .width(Fill);
    if let Some(size) = size {
        rich = rich.size(Pixels(size));
    }
    Some(rich.into())
}

fn cut_marker<'a>() -> Element<'a, Message> {
    text(ELLIPSIS.to_string()).into()
}

fn blocks_column<'a>(
    items: &[Block],
    ctx: &Ctx<'_>,
    theme: &Theme,
    walk: &mut Walk,
    depth: usize,
) -> Element<'a, Message> {
    let mut col = Column::new().spacing(4).width(Fill);
    for block in items {
        if walk.exhausted() || depth >= MAX_DEPTH {
            col = col.push(cut_marker());
            break;
        }
        walk.start_block();
        match block {
            Block::Paragraph(inlines) => {
                let models = spans_for(inlines, ctx, walk);
                if let Some(p) = paragraph(&models, theme, None) {
                    col = col.push(p);
                }
            }
            Block::Heading { level, inlines } => {
                let mut models = spans_for(inlines, ctx, walk);
                for model in &mut models {
                    model.style.bold = true;
                }
                if let Some(p) = paragraph(&models, theme, Some(heading_size(*level))) {
                    col = col.push(p);
                }
            }
            Block::Code { text: code } => {
                let code = walk.take(code);
                col = col.push(code_block(code));
            }
            Block::Quote(inner) => {
                col = col.push(
                    row![
                        rule::vertical(3),
                        blocks_column(inner, ctx, theme, walk, depth + 1)
                    ]
                    .spacing(8)
                    .height(Shrink),
                );
            }
            Block::List {
                ordered,
                start,
                items,
            } => {
                let mut list = Column::new().spacing(2).width(Fill);
                for (i, item) in items.iter().enumerate() {
                    if walk.exhausted() {
                        list = list.push(cut_marker());
                        break;
                    }
                    list = list.push(
                        row![
                            text(list_marker(*ordered, *start, i)).width(Length::Fixed(28.0)),
                            blocks_column(item, ctx, theme, walk, depth + 1),
                        ]
                        .spacing(4),
                    );
                }
                col = col.push(list);
            }
            Block::Rule => {
                col = col.push(rule::horizontal(1));
            }
        }
    }
    col.into()
}

fn code_block<'a>(code: String) -> Element<'a, Message> {
    container(
        text(code)
            .font(Font::MONOSPACE)
            .wrapping(Wrapping::WordOrGlyph)
            .width(Fill),
    )
    .width(Fill)
    .padding(6)
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(Background::Color(palette.background.weak.color)),
            border: Border {
                color: palette.background.strong.color,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        }
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Inline {
        Inline::Text {
            text: text.into(),
            style: TextStyle::default(),
        }
    }

    fn ctx<'a>(revealed: &'a Revealed, own: Option<&'a UserId>) -> Ctx<'a> {
        Ctx {
            own_user: own,
            item: "item1",
            revealed,
        }
    }

    fn spans(inlines: &[Inline]) -> Vec<SpanModel> {
        spans_for(inlines, &ctx(&Revealed::new(), None), &mut Walk::default())
    }

    #[test]
    fn styles_combine_on_a_link() {
        let style = TextStyle {
            bold: true,
            italic: true,
            ..TextStyle::default()
        };
        let out = spans(&[Inline::Link {
            text: "site".into(),
            url: "https://example.org/".into(),
            style,
        }]);
        assert_eq!(out.len(), 1);
        assert!(out[0].style.bold && out[0].style.italic);
        assert_eq!(out[0].look, Look::Link);
        assert_eq!(
            out[0].link,
            Some(LinkTarget::Url {
                url: "https://example.org/".into(),
                confirm: false
            })
        );
    }

    #[test]
    fn a_link_whose_text_names_another_host_asks_first() {
        let out = spans(&[Inline::Link {
            text: "bank.example".into(),
            url: "https://evil.example/".into(),
            style: TextStyle::default(),
        }]);
        assert!(matches!(
            out[0].link,
            Some(LinkTarget::Url { confirm: true, .. })
        ));
    }

    #[test]
    fn a_label_cut_by_the_budget_is_judged_whole() {
        let mut walk = Walk {
            chars_left: 4,
            ..Walk::default()
        };
        let out = spans_for(
            &[Inline::Link {
                text: "evil.example".into(),
                url: "https://bank.example/".into(),
                style: TextStyle::default(),
            }],
            &ctx(&Revealed::new(), None),
            &mut walk,
        );
        assert!(
            !out[0].text.contains('.'),
            "cut before the dot: {:?}",
            out[0].text
        );
        assert!(matches!(
            out[0].link,
            Some(LinkTarget::Url { confirm: true, .. })
        ));
    }

    #[test]
    fn a_mention_is_a_pill_with_a_user_target() {
        let alice = UserId::from("@alice:x");
        let inline = Inline::Mention {
            target: MentionTarget::User(alice.clone()),
            label: "Alice".into(),
        };
        let revealed = Revealed::new();
        let other = UserId::from("@bob:x");
        let out = spans_for(
            std::slice::from_ref(&inline),
            &ctx(&revealed, Some(&other)),
            &mut Walk::default(),
        );
        assert_eq!(out[0].text, "Alice");
        assert_eq!(out[0].look, Look::Mention { own: false });
        assert_eq!(out[0].link, Some(LinkTarget::User(alice.clone())));
        let out = spans_for(
            &[inline],
            &ctx(&revealed, Some(&alice)),
            &mut Walk::default(),
        );
        assert_eq!(out[0].look, Look::Mention { own: true });
    }

    #[test]
    fn a_room_pill_targets_the_room() {
        let out = spans(&[Inline::Mention {
            target: MentionTarget::Room("!r:x".into()),
            label: "\u{202E}Lounge\n".into(),
        }]);
        assert_eq!(out[0].link, Some(LinkTarget::Room("!r:x".into())));
        assert!(
            !out[0].text.contains(['\u{202E}', '\n']),
            "{:?}",
            out[0].text
        );
    }

    #[test]
    fn bidi_overrides_in_link_text_are_replaced() {
        let out = spans(&[Inline::Link {
            text: "good\u{202E}moc.live".into(),
            url: "https://example.org/".into(),
            style: TextStyle::default(),
        }]);
        assert!(!out[0].text.contains('\u{202E}'));
        assert!(out[0].text.contains('\u{FFFD}'));
    }

    #[test]
    fn plain_text_keeps_its_lines_and_loses_controls() {
        let out = spans(&[plain("a\r\nb\u{202E}c"), Inline::LineBreak, plain("d")]);
        let all: String = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(all, "a\nb\u{FFFD}c\nd");
    }

    #[test]
    fn a_spoiler_hides_its_text_until_revealed() {
        let inlines = [Inline::Spoiler(vec![plain("secret")]), plain(" after")];
        let hidden = spans(&inlines);
        assert_eq!(hidden[0].text, "Spoiler");
        assert_eq!(hidden[0].look, Look::SpoilerPill);
        assert_eq!(
            hidden[0].link,
            Some(LinkTarget::Spoiler {
                item: "item1".into(),
                index: 0
            })
        );
        assert!(hidden.iter().all(|s| !s.text.contains("secret")));
        let mut revealed = Revealed::new();
        revealed.insert(("item1".into(), 0));
        let open = spans_for(&inlines, &ctx(&revealed, None), &mut Walk::default());
        assert_eq!(open[0].text, "secret");
        assert_eq!(open[0].look, Look::SpoilerOpen);
        assert!(matches!(
            open[0].link,
            Some(LinkTarget::Spoiler { index: 0, .. })
        ));
        assert_eq!(open[1].text, " after");
    }

    #[test]
    fn spoiler_indexes_do_not_depend_on_what_is_revealed() {
        let inlines = [
            Inline::Spoiler(vec![Inline::Spoiler(vec![plain("inner")])]),
            Inline::Spoiler(vec![plain("second")]),
        ];
        let index_of_last = |revealed: &Revealed| {
            let out = spans_for(&inlines, &ctx(revealed, None), &mut Walk::default());
            match out.last().and_then(|s| s.link.clone()) {
                Some(LinkTarget::Spoiler { index, .. }) => index,
                other => panic!("{other:?}"),
            }
        };
        let mut revealed = Revealed::new();
        assert_eq!(index_of_last(&revealed), 2);
        revealed.insert(("item1".into(), 0));
        assert_eq!(index_of_last(&revealed), 2);
        revealed.insert(("item1".into(), 1));
        assert_eq!(index_of_last(&revealed), 2);
    }

    #[test]
    fn the_budget_caps_characters_and_lines() {
        let big = plain(&"x".repeat(MAX_MESSAGE_CHARS * 3));
        let out = spans(&[big.clone(), big]);
        let total: usize = out.iter().map(|s| s.text.chars().count()).sum();
        assert_eq!(total, MAX_MESSAGE_CHARS);
        assert!(out[0].text.ends_with(ELLIPSIS));
        let lines = plain(&"y\n".repeat(5000));
        let out = spans(&[lines.clone(), lines]);
        let breaks: usize = out.iter().map(|s| s.text.matches('\n').count()).sum();
        assert!(breaks <= MAX_MESSAGE_LINES, "{breaks}");
    }

    #[test]
    fn a_pill_label_is_one_line_and_capped() {
        let out = spans(&[Inline::Mention {
            target: MentionTarget::User(UserId::from("@a:x")),
            label: "n".repeat(5000),
        }]);
        assert_eq!(out[0].text.chars().count(), MAX_NAME_CHARS);
        let blank = spans(&[Inline::Mention {
            target: MentionTarget::User(UserId::from("@a:x")),
            label: "  ".into(),
        }]);
        assert_eq!(blank[0].text, crate::text_safety::NAME_PLACEHOLDER);
    }

    fn every_look() -> Vec<SpanModel> {
        let alice = UserId::from("@alice:x");
        let inlines = [
            plain("plain"),
            Inline::Text {
                text: "code".into(),
                style: TextStyle {
                    code: true,
                    bold: true,
                    ..TextStyle::default()
                },
            },
            Inline::Link {
                text: "l".into(),
                url: "https://e.org/".into(),
                style: TextStyle::default(),
            },
            Inline::Mention {
                target: MentionTarget::User(alice.clone()),
                label: "Alice".into(),
            },
            Inline::Spoiler(vec![plain("s")]),
        ];
        let revealed = Revealed::new();
        let mut out = spans_for(
            &inlines,
            &ctx(&revealed, Some(&alice)),
            &mut Walk::default(),
        );
        let mut open = Revealed::new();
        open.insert(("item1".into(), 0));
        out.extend(spans_for(&inlines, &ctx(&open, None), &mut Walk::default()));
        out
    }

    #[test]
    fn no_span_can_look_like_the_muted_time_of_a_header() {
        for theme in [
            theme::theme_for(iced::theme::Mode::Light),
            theme::theme_for(iced::theme::Mode::Dark),
        ] {
            let muted = theme::semantic(&theme).muted;
            for model in every_look() {
                let span = to_span(&model, &theme);
                assert_ne!(span.color, Some(muted), "{model:?}");
                assert_eq!(span.size, None, "{model:?}");
            }
        }
    }

    #[test]
    fn links_and_pills_are_coloured_and_plain_text_is_not() {
        let theme = theme::theme_for(iced::theme::Mode::Light);
        let models = every_look();
        let by = |look: Look| {
            models
                .iter()
                .find(|m| m.look == look)
                .expect("look present")
        };
        assert_eq!(to_span(by(Look::Plain), &theme).color, None);
        let link = to_span(by(Look::Link), &theme);
        assert!(link.underline && link.color.is_some() && link.link.is_some());
        assert!(
            to_span(by(Look::Mention { own: true }), &theme)
                .highlight
                .is_some()
        );
        let own = to_span(by(Look::Mention { own: true }), &theme);
        let other = SpanModel {
            look: Look::Mention { own: false },
            ..by(Look::Mention { own: true }).clone()
        };
        assert_ne!(own.highlight, to_span(&other, &theme).highlight);
        assert!(to_span(by(Look::SpoilerPill), &theme).highlight.is_some());
    }

    #[test]
    fn code_is_monospace_and_styles_map_to_fonts() {
        let theme = theme::theme_for(iced::theme::Mode::Light);
        let models = every_look();
        let code = models.iter().find(|m| m.style.code).expect("code span");
        let span = to_span(code, &theme);
        let font = span.font.expect("code has a font");
        assert_eq!(font.family, Font::MONOSPACE.family);
        assert_eq!(font.weight, Weight::Bold);
        assert!(span.highlight.is_some());
        let italic = SpanModel {
            text: "i".into(),
            style: TextStyle {
                italic: true,
                strike: true,
                underline: true,
                ..TextStyle::default()
            },
            look: Look::Plain,
            link: None,
        };
        let span = to_span(&italic, &theme);
        assert_eq!(span.font.expect("font").style, FontStyle::Italic);
        assert!(span.strikethrough && span.underline);
    }

    #[test]
    fn link_targets_never_print_their_content() {
        let targets = [
            LinkTarget::Url {
                url: "https://sentinel.example/".into(),
                confirm: true,
            },
            LinkTarget::User(UserId::from("@sentinel:x")),
            LinkTarget::Room("#sentinel:x".into()),
            LinkTarget::Spoiler {
                item: "sentinel".into(),
                index: 3,
            },
        ];
        for target in targets {
            assert!(!format!("{target:?}").contains("sentinel"));
        }
    }

    #[test]
    fn list_markers() {
        assert_eq!(list_marker(false, None, 4), "\u{2022}");
        assert_eq!(list_marker(true, None, 0), "1.");
        assert_eq!(list_marker(true, Some(5), 2), "7.");
        assert_eq!(
            list_marker(true, Some(u64::MAX), 3),
            format!("{}.", u64::MAX)
        );
    }

    fn quote_of(depth: usize) -> RichText {
        let mut block = Block::Paragraph(vec![plain("deep")]);
        for i in 0..depth {
            block = if i % 2 == 0 {
                Block::Quote(vec![block])
            } else {
                Block::List {
                    ordered: i % 4 == 1,
                    start: None,
                    items: vec![vec![block]],
                }
            };
        }
        RichText {
            blocks: vec![block],
        }
    }

    #[test]
    fn every_block_kind_builds_and_deep_nesting_is_cut() {
        let theme = theme::theme_for(iced::theme::Mode::Dark);
        let revealed = Revealed::new();
        let c = ctx(&revealed, None);
        let all = RichText {
            blocks: vec![
                Block::Heading {
                    level: 1,
                    inlines: vec![plain("h")],
                },
                Block::Paragraph(vec![plain("p")]),
                Block::Code {
                    text: "fn main() {}\n".repeat(10),
                },
                Block::Rule,
                Block::Paragraph(vec![]),
            ],
        };
        drop(blocks_view(&all, &c, &theme));
        // Far beyond the core's own cap of 32; the view must stop at MAX_DEPTH.
        drop(blocks_view(&quote_of(200), &c, &theme));
        drop(blocks_view(&RichText::default(), &c, &theme));
    }

    #[test]
    fn heading_sizes_stay_clear_of_the_body_and_header_size() {
        for level in 1..=6 {
            assert!(heading_size(level) > 16.0, "{level}");
        }
        assert!(heading_size(1) > heading_size(2));
        assert!(heading_size(9) > 16.0);
        assert!(heading_size(0) > 16.0);
    }
}
