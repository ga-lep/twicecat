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
use matrix_sdk::ruma::html::{Html, SanitizerConfig};

use crate::api::{MentionRef, MessageKind, OutgoingMessage};

/// Most `<` characters a message may hold and still be sent as HTML. Raw
/// HTML needs a `<`, and the sanitizer parses it into a tree, so a flood of
/// tags is sent as plain text instead.
const MAX_ANGLE_BRACKETS: usize = 1000;

/// Elements removed together with their content. The sanitizer's spec list
/// would drop the others' tags and keep their text.
const REMOVED_ELEMENTS: [&str; 3] = ["img", "script", "style"];

/// Builds the event content for `message`:
/// - each mention label is linked at its first free occurrence in the text;
/// - the text goes through markdown, and raw HTML in it is sanitized away;
/// - the `body` is the text as typed, and `m.mentions` lists the users whose
///   label was found.
pub(crate) fn outgoing_content(
    message: &OutgoingMessage,
) -> RoomMessageEventContentWithoutRelation {
    let text = message.text.expose();
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
    if let Some((body, formatted)) = formatted {
        text.clone_into(body);
        sanitize_formatted(formatted, text);
        if formatted.is_none() && text.contains('\n') {
            *formatted = Some(FormattedBody::html(line_breaks_html(text)));
        }
    }
    if mentioned.is_empty() {
        content
    } else {
        content.add_mentions(Mentions::with_user_ids(mentioned))
    }
}

/// Drops raw HTML from a markdown-made formatted body.
///
/// `matrix-sdk`'s markdown (pulldown-cmark) passes HTML events through
/// unchanged. Without a `<` in the source there is none, so the body is kept
/// as is; with one, the Matrix-spec sanitizer allow-lists the tags and
/// attributes and `img`, `script` and `style` go with their content. Code
/// spans are unaffected: markdown has already escaped their `<`.
fn sanitize_formatted(formatted: &mut Option<FormattedBody>, source: &str) {
    let angle_brackets = source.matches('<').count();
    if angle_brackets == 0 {
        return;
    }
    if angle_brackets > MAX_ANGLE_BRACKETS {
        *formatted = None;
        return;
    }
    if let Some(f) = formatted {
        let html = Html::parse(&f.body);
        html.sanitize_with(&SanitizerConfig::strict().remove_elements(REMOVED_ELEMENTS));
        f.body = html.to_string();
    }
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
    use crate::api::{MentionRef, MessageKind, MessageText, RoomId, UserId};

    fn message(text: &str, mentions: Vec<MentionRef>, kind: MessageKind) -> OutgoingMessage {
        OutgoingMessage {
            room_id: RoomId::from("!r:localhost"),
            text: MessageText::from(text),
            mentions,
            reply_to: None,
            kind,
        }
    }

    fn mention(label: &str, user: &str) -> MentionRef {
        MentionRef {
            label: label.to_owned(),
            user_id: UserId::from(user.to_owned()),
        }
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
}
