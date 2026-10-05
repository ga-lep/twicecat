//! Building the content of an outgoing message: markdown to HTML, mentions
//! and the plain-text body.
//!
//! The message text is user content: nothing here logs it.

use std::ops::Range;

use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::events::Mentions;
use matrix_sdk::ruma::events::room::message::{
    FormattedBody, MessageType, RoomMessageEventContentWithoutRelation,
};
use matrix_sdk::ruma::html::SanitizerConfig;

use crate::api::{MentionRef, MessageKind, OutgoingMessage, TextFormat};

/// Most start tags (`<` followed by a letter) a formatted body may hold. The
/// sanitizer here and the receive mapper (`rich_text::from_html`) parse the
/// body into a tree whose recursive `Drop` overflowed a 2 MiB debug stack at
/// about 6,000 nested elements. A tree is never deeper than its element count,
/// so counting tags in the generated HTML (not in the source: markdown nests
/// with `>` and list markers alone) keeps both far from that. Above it the
/// message is sent as plain text.
const MAX_TAGS: usize = 1500;

/// Elements removed together with their content. The sanitizer's spec list
/// would drop the others' tags and keep their text.
const REMOVED_ELEMENTS: [&str; 3] = ["img", "script", "style"];

/// Builds the event content for `message`:
/// - each mention label is linked at its first free occurrence in the text;
/// - the text goes through markdown, and the HTML is sanitized and bounded;
/// - the `body` is the text as typed, and `m.mentions` lists the users whose
///   label was found and survives in the formatted body.
pub(crate) fn outgoing_content(
    message: &OutgoingMessage,
) -> RoomMessageEventContentWithoutRelation {
    let text = message.text.expose();
    match message.format {
        TextFormat::Markdown => {}
        // As Element's `/plain` and `/html`: no markdown and no pills, but the
        // picked mentions still go to `m.mentions` (element-web
        // `utils/messages.ts` `attachMentions`).
        TextFormat::Plain => {
            let content = match message.kind {
                MessageKind::Text => RoomMessageEventContentWithoutRelation::text_plain(text),
                MessageKind::Notice => RoomMessageEventContentWithoutRelation::notice_plain(text),
                MessageKind::Emote => RoomMessageEventContentWithoutRelation::emote_plain(text),
            };
            return with_picked_mentions(content, &message.mentions);
        }
        TextFormat::Html | TextFormat::Spoiler | TextFormat::Rainbow => {
            // Element: `/html` sends the source twice, `/spoiler` wraps it
            // unescaped (`makeHtmlMessage(message, <span data-mx-spoiler>…)`),
            // `/rainbow` colours each character (`textToHtmlRainbow`).
            let html = match message.format {
                TextFormat::Spoiler => format!("<span data-mx-spoiler>{text}</span>"),
                TextFormat::Rainbow => rainbow_html(text),
                _ => text.to_owned(),
            };
            let mut content = match message.kind {
                MessageKind::Text => RoomMessageEventContentWithoutRelation::text_html(text, html),
                MessageKind::Notice => {
                    RoomMessageEventContentWithoutRelation::notice_html(text, html)
                }
                MessageKind::Emote => {
                    RoomMessageEventContentWithoutRelation::emote_html(text, html)
                }
            };
            match &mut content.msgtype {
                MessageType::Text(t) => sanitize_formatted(&mut t.formatted),
                MessageType::Notice(n) => sanitize_formatted(&mut n.formatted),
                MessageType::Emote(e) => sanitize_formatted(&mut e.formatted),
                _ => {}
            }
            return with_picked_mentions(content, &message.mentions);
        }
    }
    let (source, mentioned) = markdown_with_mentions(text, &message.mentions);
    let mut content = match message.kind {
        MessageKind::Text => RoomMessageEventContentWithoutRelation::text_markdown(source),
        MessageKind::Notice => RoomMessageEventContentWithoutRelation::notice_markdown(source),
        MessageKind::Emote => RoomMessageEventContentWithoutRelation::emote_markdown(source),
    };
    let formatted = match &mut content.msgtype {
        MessageType::Text(t) => Some((&mut t.body, &mut t.formatted)),
        MessageType::Notice(n) => Some((&mut n.body, &mut n.formatted)),
        MessageType::Emote(e) => Some((&mut e.body, &mut e.formatted)),
        _ => None,
    };
    let mut html = None;
    if let Some((body, formatted)) = formatted {
        text.clone_into(body);
        let made_by_markdown = formatted.is_some();
        sanitize_formatted(formatted);
        if !made_by_markdown && text.contains('\n') {
            let breaks = line_breaks_html(text);
            if !too_many_tags(&breaks) {
                *formatted = Some(FormattedBody::html(breaks));
            }
        }
        html = formatted.as_ref().map(|f| f.body.as_str());
    }
    // Only a user whose link is in the body that is sent is notified: a label
    // inside code, or in a body sent as plain text, shows no pill.
    let mentioned: Vec<OwnedUserId> = mentioned
        .into_iter()
        .filter(|user| html.is_some_and(|h| h.contains(&mention_href(user))))
        .collect();
    if mentioned.is_empty() {
        content
    } else {
        content.add_mentions(Mentions::with_user_ids(mentioned))
    }
}

/// Makes the markdown-made formatted body safe: always, because markdown
/// itself makes links (`javascript:`, `data:`) and images, not only raw HTML.
///
/// `matrix-sdk`'s markdown (pulldown-cmark) passes HTML events through
/// unchanged. The Matrix-spec sanitizer allow-lists the tags and attributes
/// (`href` schemes: http, https, ftp, mailto, magnet, matrix; `img` `src`:
/// mxc), and `img`, `script` and `style` go with their content. Code spans
/// are unaffected: markdown has already escaped their `<`. A body with more
/// than [`MAX_TAGS`] start tags is dropped before it is parsed.
fn sanitize_formatted(formatted: &mut Option<FormattedBody>) {
    let Some(f) = formatted else {
        return;
    };
    if too_many_tags(&f.body) {
        *formatted = None;
        return;
    }
    let html = crate::rich_text::ParsedHtml::parse(&f.body);
    html.sanitize_with(&SanitizerConfig::strict().remove_elements(REMOVED_ELEMENTS));
    f.body = html.to_string();
}

/// Whether `html` has more than [`MAX_TAGS`] start-tag candidates (`<`
/// followed by an ASCII letter). Over-counts, which is the safe side.
fn too_many_tags(html: &str) -> bool {
    html.as_bytes()
        .windows(2)
        .filter(|w| w[0] == b'<' && w[1].is_ascii_alphabetic())
        .nth(MAX_TAGS)
        .is_some()
}

/// Element's `textToHtmlRainbow` (element-web v1.12.30 `utils/colour.ts`):
/// each character in a `data-mx-color` span, the hue turning once over the
/// text at CIELAB lightness 75; spaces stay bare. Unlike Element, characters
/// are HTML-escaped. The step uses the UTF-16 length, as JavaScript's
/// `str.length` does.
pub(crate) fn rainbow_html(text: &str) -> String {
    let units = text.encode_utf16().count();
    if units == 0 {
        return String::new();
    }
    #[allow(clippy::cast_precision_loss)] // a message is far below 2^52 units
    let frequency = 2.0 * std::f64::consts::PI / units as f64;
    let mut html = String::with_capacity(text.len() * 40);
    for (i, chunk) in rainbow_chunks(text).into_iter().enumerate() {
        if chunk == " " {
            html.push(' ');
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let hue = i as f64 * frequency;
        let (red, green, blue) = lab_to_rgb(75.0, 127.0 * hue.cos(), 127.0 * hue.sin());
        html.push_str(&format!(
            r##"<span data-mx-color="#{red:02x}{green:02x}{blue:02x}">"##
        ));
        for c in chunk.chars() {
            match c {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '"' => html.push_str("&quot;"),
                _ => html.push(c),
            }
        }
        html.push_str("</span>");
    }
    html
}

/// The text cut into what Element's lodash `split(str, "")` keeps whole, as
/// near as possible without a grapheme table: combining marks, variation
/// selectors, emoji modifiers and tags stay with the character before them,
/// and a zero-width joiner glues the next character on.
fn rainbow_chunks(text: &str) -> Vec<&str> {
    let joins_previous = |c: char| {
        matches!(c,
            '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FE20}'..='\u{FE2F}'
            | '\u{200D}'
            | '\u{1F3FB}'..='\u{1F3FF}'
            | '\u{E0020}'..='\u{E007F}'
            | '\u{E0100}'..='\u{E01EF}')
    };
    let mut chunks: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut glue_next = false;
    for (at, c) in text.char_indices() {
        if at > 0 && !joins_previous(c) && !glue_next {
            chunks.push(&text[start..at]);
            start = at;
        }
        glue_next = c == '\u{200D}';
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

/// CIELAB (D65) to sRGB bytes, as Element's `labToRGB`.
fn lab_to_rgb(l: f64, a: f64, b: f64) -> (u8, u8, u8) {
    fn adjust_xyz(v: f64) -> f64 {
        if v > 0.2069 {
            v.powi(3)
        } else {
            0.1284 * v - 0.01771
        }
    }
    fn adjust_rgb(v: f64) -> u8 {
        let corrected = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        // Clamped to 0..=1 first, so the cast cannot truncate.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = (corrected.clamp(0.0, 1.0) * 255.0).round() as u8;
        byte
    }
    let y = (l + 16.0) / 116.0;
    let x = adjust_xyz(y + a / 500.0) * 0.9505;
    let z = adjust_xyz(y - b / 200.0) * 1.089;
    let y = adjust_xyz(y);
    let red = 3.240_969_94 * x - 1.537_383_18 * y - 0.498_610_76 * z;
    let green = -0.969_243_64 * x + 1.875_967_5 * y + 0.041_555_06 * z;
    let blue = 0.055_630_08 * x - 0.203_976_96 * y + 1.056_971_51 * z;
    (adjust_rgb(red), adjust_rgb(green), adjust_rgb(blue))
}

/// Adds `m.mentions` for every picked mention with a valid user ID (none:
/// the content unchanged).
fn with_picked_mentions(
    content: RoomMessageEventContentWithoutRelation,
    mentions: &[MentionRef],
) -> RoomMessageEventContentWithoutRelation {
    let users: Vec<OwnedUserId> = mentions
        .iter()
        .filter_map(|m| OwnedUserId::try_from(m.user_id.as_str()).ok())
        .collect();
    if users.is_empty() {
        content
    } else {
        content.add_mentions(Mentions::with_user_ids(users))
    }
}

/// The `href` attribute (as serialized) of a mention link.
fn mention_href(user: &OwnedUserId) -> String {
    let mut href = String::from(r#"href="https://matrix.to/#/"#);
    percent_encode(user.as_str(), &mut href);
    href.push('"');
    href
}

/// HTML for a text that has no markdown but several lines: the escaped lines
/// joined by `<br />`, as markdown renders a line break. (The markdown
/// wrapper leaves such a text unformatted, so clients that read only the
/// formatted body would otherwise lose the breaks.)
fn line_breaks_html(text: &str) -> String {
    let mut html = String::with_capacity(text.len() + 16);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            html.push_str("<br />\n");
        }
        for c in line.chars() {
            match c {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '"' => html.push_str("&quot;"),
                _ => html.push(c),
            }
        }
    }
    html
}

/// The markdown source with every found mention label replaced by a link to
/// the user, and the mentioned users (once each, in text order).
fn markdown_with_mentions(text: &str, mentions: &[MentionRef]) -> (String, Vec<OwnedUserId>) {
    // Each label takes its first occurrence that no earlier mention covers.
    let mut found: Vec<(Range<usize>, &MentionRef, OwnedUserId)> = Vec::new();
    for mention in mentions {
        let Ok(user_id) = OwnedUserId::try_from(mention.user_id.as_str()) else {
            continue;
        };
        if mention.label.is_empty() {
            continue;
        }
        let free = text
            .match_indices(mention.label.as_str())
            .find(|(at, label)| {
                let range = *at..at + label.len();
                found
                    .iter()
                    .all(|(r, ..)| range.end <= r.start || r.end <= range.start)
            });
        if let Some((at, label)) = free {
            found.push((at..at + label.len(), mention, user_id));
        }
    }
    found.sort_by_key(|(range, ..)| range.start);

    let mut source = String::with_capacity(text.len());
    let mut users: Vec<OwnedUserId> = Vec::new();
    let mut cursor = 0;
    for (range, mention, user_id) in found {
        source.push_str(&text[cursor..range.start]);
        source.push('[');
        escape_markdown(&mention.label, &mut source);
        source.push_str("](https://matrix.to/#/");
        percent_encode(user_id.as_str(), &mut source);
        source.push(')');
        cursor = range.end;
        if !users.contains(&user_id) {
            users.push(user_id);
        }
    }
    source.push_str(&text[cursor..]);
    (source, users)
}

/// Backslash-escapes every ASCII punctuation character (always valid in
/// CommonMark), so the label is shown as typed.
fn escape_markdown(label: &str, out: &mut String) {
    for c in label.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Percent-encodes everything but the RFC 3986 unreserved characters.
fn percent_encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::events::room::message::MessageType;

    use super::*;
    use crate::api::{MentionRef, MessageKind, MessageText, RoomId, TextFormat, UserId};

    fn message(text: &str, mentions: Vec<MentionRef>, kind: MessageKind) -> OutgoingMessage {
        OutgoingMessage {
            room_id: RoomId::from("!r:localhost"),
            text: MessageText::from(text),
            mentions,
            reply_to: None,
            kind,
            format: TextFormat::Markdown,
        }
    }

    fn mention(label: &str, user: &str) -> MentionRef {
        MentionRef {
            label: label.to_owned(),
            user_id: UserId::from(user.to_owned()),
        }
    }

    fn formatted_as(text: &str, format: TextFormat) -> (String, Option<String>) {
        let mut m = message(text, Vec::new(), MessageKind::Text);
        m.format = format;
        text_parts(&outgoing_content(&m))
    }

    /// element-web v1.12.30 `slash-commands/__snapshots__/rainbow.test.ts.snap`.
    const ELEMENT_RAINBOW: &str = r##"<span data-mx-color="#ff00be">t</span><span data-mx-color="#ff0080">h</span><span data-mx-color="#ff0041">i</span><span data-mx-color="#ff5f00">s</span> <span data-mx-color="#faa900">i</span><span data-mx-color="#c3bf00">s</span> <span data-mx-color="#00d800">a</span> <span data-mx-color="#00e371">t</span><span data-mx-color="#00e6b6">e</span><span data-mx-color="#00e7f8">s</span><span data-mx-color="#00e7ff">t</span> <span data-mx-color="#00deff">m</span><span data-mx-color="#00d2ff">e</span><span data-mx-color="#00c0ff">s</span><span data-mx-color="#44a4ff">s</span><span data-mx-color="#e87dff">a</span><span data-mx-color="#ff42ff">g</span><span data-mx-color="#ff00fe">e</span>"##;

    #[test]
    fn rainbow_html_matches_elements_snapshot() {
        assert_eq!(rainbow_html("this is a test message"), ELEMENT_RAINBOW);
    }

    #[test]
    fn rainbow_format_sends_the_text_and_coloured_html() {
        let (body, html) = formatted_as("this is a test message", TextFormat::Rainbow);
        assert_eq!(body, "this is a test message");
        assert_eq!(
            html.as_deref(),
            Some(ELEMENT_RAINBOW),
            "survives sanitising"
        );
        let (_, escaped) = formatted_as("a<b", TextFormat::Rainbow);
        let escaped = escaped.expect("html");
        assert!(
            escaped.contains("&lt;") && !escaped.contains("<b"),
            "{escaped}"
        );
    }

    #[test]
    fn rainbow_keeps_combined_characters_together() {
        // An accent, a variation selector and a ZWJ sequence stay in one span.
        let html = rainbow_html("e\u{301}\u{2764}\u{FE0F}\u{1F469}\u{200D}\u{1F4BB}");
        assert_eq!(html.matches("<span").count(), 3, "{html}");
    }

    #[test]
    fn spoiler_format_wraps_the_text_like_element() {
        let (body, html) = formatted_as("the butler <script>x</script>did it", TextFormat::Spoiler);
        assert_eq!(body, "the butler <script>x</script>did it");
        let html = html.expect("an html body");
        assert!(html.starts_with("<span data-mx-spoiler"), "{html}");
        assert!(
            html.contains("the butler") && html.contains("did it"),
            "{html}"
        );
        assert!(!html.contains("script"), "{html}");
    }

    #[test]
    fn plain_and_html_keep_the_mentions_like_element() {
        for format in [TextFormat::Plain, TextFormat::Html] {
            let mut m = message(
                "hi Bob",
                vec![mention("Bob", "@bob:localhost")],
                MessageKind::Text,
            );
            m.format = format;
            let mentions = outgoing_content(&m).mentions.expect("m.mentions");
            assert_eq!(mentions.user_ids.len(), 1, "{format:?}");
            assert!(
                mentions
                    .user_ids
                    .iter()
                    .any(|u| u.as_str() == "@bob:localhost")
            );
        }
    }

    #[test]
    fn plain_format_sends_the_text_without_markdown() {
        let (body, html) = formatted_as("**not bold** Bob", TextFormat::Plain);
        assert_eq!(body, "**not bold** Bob");
        assert_eq!(html, None);
    }

    #[test]
    fn html_format_sends_the_source_as_body_and_sanitised_html() {
        let source =
            r#"<a href="https://matrix.org">click here</a> *stars* <script>alert(1)</script>"#;
        let (body, html) = formatted_as(source, TextFormat::Html);
        assert_eq!(body, source, "Element sends the source as the body too");
        let html = html.expect("an html body");
        assert!(
            html.contains(r#"<a href="https://matrix.org">click here</a>"#),
            "{html}"
        );
        assert!(
            !html.contains("script") && !html.contains("alert"),
            "{html}"
        );
        assert!(
            html.contains("*stars*") && !html.contains("<em>"),
            "no markdown: {html}"
        );
    }

    /// Body and formatted body of a text message.
    fn text_parts(content: &RoomMessageEventContentWithoutRelation) -> (String, Option<String>) {
        match &content.msgtype {
            MessageType::Text(t) => (t.body.clone(), t.formatted.as_ref().map(|f| f.body.clone())),
            other => panic!("not a text message: {other:?}"),
        }
    }

    fn user_ids(content: &RoomMessageEventContentWithoutRelation) -> Vec<String> {
        content
            .mentions
            .as_ref()
            .map(|m| m.user_ids.iter().map(ToString::to_string).collect())
            .unwrap_or_default()
    }

    #[test]
    fn plain_text_has_no_formatted_body() {
        let c = outgoing_content(&message("hello", vec![], MessageKind::Text));
        assert_eq!(text_parts(&c), ("hello".to_owned(), None));
        assert!(c.mentions.is_none());
    }

    #[test]
    fn markdown_becomes_html() {
        let c = outgoing_content(&message("**bold** and _it_", vec![], MessageKind::Text));
        let (body, html) = text_parts(&c);
        assert_eq!(body, "**bold** and _it_");
        assert_eq!(
            html.as_deref(),
            Some("<strong>bold</strong> and <em>it</em>")
        );
    }

    #[test]
    fn fenced_code_becomes_pre_code() {
        let c = outgoing_content(&message("```\nlet x = 1;\n```", vec![], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert!(html.contains("<pre><code>"), "{html}");
        assert!(html.contains("let x = 1;"), "{html}");
    }

    #[test]
    fn newline_becomes_br() {
        let c = outgoing_content(&message("a\nb", vec![], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert!(html.contains("a<br />"), "{html}");
        assert!(html.ends_with('b'), "{html}");
    }

    #[test]
    fn line_breaks_escape_angle_brackets() {
        let c = outgoing_content(&message("a < b & c\nd", vec![], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert_eq!(html, "a &lt; b &amp; c<br />\nd");
    }

    #[test]
    fn mention_becomes_a_matrix_to_link_with_plain_body() {
        let text = "hi Alice [x], welcome";
        let m = mention("Alice [x]", "@alice:localhost");
        let c = outgoing_content(&message(text, vec![m], MessageKind::Text));
        let (body, html) = text_parts(&c);
        assert_eq!(body, text);
        let html = html.expect("formatted");
        assert!(
            html.contains(r#"<a href="https://matrix.to/#/%40alice%3Alocalhost">Alice [x]</a>"#),
            "{html}"
        );
        assert_eq!(user_ids(&c), ["@alice:localhost"]);
    }

    #[test]
    fn a_label_that_is_gone_is_not_mentioned() {
        let m = mention("Bob", "@bob:localhost");
        let c = outgoing_content(&message("hello", vec![m], MessageKind::Text));
        assert!(user_ids(&c).is_empty());
        assert_eq!(text_parts(&c), ("hello".to_owned(), None));
    }

    #[test]
    fn only_the_first_occurrence_of_a_label_is_linked() {
        let m = mention("Al", "@alice:localhost");
        let c = outgoing_content(&message("Al and Al", vec![m], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
    }

    #[test]
    fn two_mentions_with_one_label_inside_the_other_do_not_overlap() {
        let a = mention("Al Smith", "@alsmith:localhost");
        let b = mention("Al", "@al:localhost");
        let c = outgoing_content(&message("Al Smith", vec![a, b], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
        assert_eq!(user_ids(&c), ["@alsmith:localhost"]);
    }

    #[test]
    fn markdown_in_a_label_is_escaped() {
        let m = mention("*bob*", "@bob:localhost");
        let c = outgoing_content(&message("hey *bob*", vec![m], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert!(html.contains(">*bob*</a>"), "{html}");
        assert!(!html.contains("<em>"), "{html}");
    }

    #[test]
    fn emote_kind_gives_an_emote() {
        let c = outgoing_content(&message("/me waves", vec![], MessageKind::Emote));
        assert!(matches!(c.msgtype, MessageType::Emote(_)));
    }

    #[test]
    fn notice_kind_gives_a_notice() {
        let c = outgoing_content(&message("**x**", vec![], MessageKind::Notice));
        assert!(matches!(c.msgtype, MessageType::Notice(_)));
    }

    #[test]
    fn raw_html_does_not_survive_in_the_formatted_body() {
        for source in [
            "<img src=x onerror=alert(1)>",
            "<script>alert(1)</script>",
            "**b** <img src=x onerror=alert(1)> <script>alert(2)</script>",
            "<p onclick=alert(1)>x</p>\n\n<iframe src=\"https://e.example\"></iframe>",
            "<a href=\"javascript:alert(1)\">x</a> *y*",
        ] {
            let c = outgoing_content(&message(source, vec![], MessageKind::Text));
            let (body, html) = text_parts(&c);
            // The plain body is the text as typed.
            assert_eq!(body, source);
            let html = html.unwrap_or_default().to_lowercase();
            for bad in [
                "<img",
                "<script",
                "onerror",
                "onclick",
                "<iframe",
                "javascript:",
            ] {
                assert!(!html.contains(bad), "{bad} survived in {html}");
            }
        }
    }

    #[test]
    fn code_spans_keep_angle_brackets_as_text() {
        let c = outgoing_content(&message("`<b>x</b>`", vec![], MessageKind::Text));
        let html = text_parts(&c).1.expect("formatted");
        assert_eq!(html, "<code>&lt;b&gt;x&lt;/b&gt;</code>");
    }

    #[test]
    fn a_flood_of_tags_is_sent_as_plain_text() {
        let source = "<b>".repeat(5000);
        let c = outgoing_content(&message(&source, vec![], MessageKind::Text));
        assert_eq!(text_parts(&c), (source, None));
    }

    /// Runs `f` on a 2 MiB thread, the smallest stack we run on.
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(f)
            .expect("spawn")
            .join()
            .expect("no crash on a 2 MiB stack")
    }

    #[test]
    fn markdown_made_unsafe_links_and_images_are_sanitized() {
        for source in [
            "[x](javascript:alert(1))",
            "[x](data:text/html,hi)",
            "![a](https://e.example/x.png)",
            "[x](JavaScript:alert(1))",
            "[x](vbscript:y)",
        ] {
            let c = outgoing_content(&message(source, vec![], MessageKind::Text));
            let html = text_parts(&c).1.unwrap_or_default().to_lowercase();
            for bad in ["javascript:", "data:", "vbscript:", "<img", "e.example"] {
                assert!(!html.contains(bad), "{bad} survived in {html} ({source})");
            }
        }
        let c = outgoing_content(&message(
            "[x](https://e.example/)",
            vec![],
            MessageKind::Text,
        ));
        let html = text_parts(&c).1.expect("formatted");
        assert!(html.contains(r#"href="https://e.example/""#), "{html}");
    }

    #[test]
    fn raw_anchor_with_href_after_another_attribute_is_sanitized() {
        let source = r#"<a title="t" href="javascript:alert(1)">x</a>"#;
        let c = outgoing_content(&message(source, vec![], MessageKind::Text));
        let html = text_parts(&c).1.unwrap_or_default().to_lowercase();
        assert!(!html.contains("javascript:"), "{html}");
    }

    #[test]
    fn nesting_made_by_markdown_alone_is_sent_as_plain_text() {
        for source in [
            format!("<b>x</b>\n\n{} y", ">".repeat(50_000)),
            format!("{} y", ">".repeat(50_000)),
            format!("<b>x</b>\n\n{}", ">".repeat(50_000)),
            format!("{}y", "- ".repeat(50_000)),
            "![](u)".repeat(5_000),
        ] {
            let kept = source.clone();
            let (body, html) = on_small_stack(move || {
                let c = outgoing_content(&message(&source, vec![], MessageKind::Text));
                text_parts(&c)
            });
            assert_eq!(body, kept);
            assert!(
                html.is_none(),
                "{:?} formatted body of {} bytes",
                &kept[..12],
                html.map_or(0, |h| h.len())
            );
        }
    }

    #[test]
    fn a_body_at_the_tag_cap_is_kept_and_maps_on_a_small_stack() {
        // Raw `<div>` nests as deep as the source says (markdown caps `>` at 100).
        let source = format!("{}y", "<div>".repeat(MAX_TAGS - 10));
        let html = on_small_stack(move || {
            let c = outgoing_content(&message(&source, vec![], MessageKind::Text));
            let html = text_parts(&c).1.expect("formatted");
            // Our own echo must stay safe for the receive mapper.
            let _ = crate::rich_text::from_html(&html, "y");
            html
        });
        // The sanitizer also caps the depth (100, from the spec).
        assert!(html.contains("<div>"), "{html}");
    }

    #[test]
    fn implicit_table_elements_at_the_tag_cap_are_sent_on_a_small_stack() {
        // `<table><td>` also opens a tbody and a tr: the tree is about four
        // times as deep as the tag count.
        let source = format!("{}y", "<table><td>".repeat(MAX_TAGS / 2 - 5));
        let html = on_small_stack(move || {
            let c = outgoing_content(&message(&source, vec![], MessageKind::Text));
            let html = text_parts(&c).1.expect("formatted");
            let _ = crate::rich_text::from_html(&html, "y");
            html
        });
        assert!(html.contains("<table>"), "{html}");
    }

    #[test]
    fn many_lines_are_sent_without_a_formatted_body_above_the_cap() {
        let source = "a\n".repeat(MAX_TAGS + 10);
        let c = outgoing_content(&message(&source, vec![], MessageKind::Text));
        assert!(text_parts(&c).1.is_none());
    }

    #[test]
    fn a_mention_inside_code_is_not_notified() {
        let m = mention("Alice", "@alice:localhost");
        for text in ["`hi Alice`", "```\nhi Alice\n```", "    hi Alice"] {
            let c = outgoing_content(&message(text, vec![m.clone()], MessageKind::Text));
            let html = text_parts(&c).1.expect("formatted");
            assert!(!html.contains("href="), "{html}");
            assert!(user_ids(&c).is_empty(), "{text}: {:?}", user_ids(&c));
        }
    }

    #[test]
    fn a_mention_outside_code_is_kept_next_to_one_inside() {
        let m = mention("Alice", "@alice:localhost");
        let c = outgoing_content(&message("Alice and `Alice`", vec![m], MessageKind::Text));
        assert_eq!(user_ids(&c), ["@alice:localhost"]);
    }
}
