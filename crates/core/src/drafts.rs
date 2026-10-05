//! Composer drafts in the session store (matrix-sdk `Room::save_composer_draft`,
//! which keeps them in the encrypted state store of the session, removed with
//! it at logout).
//!
//! The SDK draft holds plain text and an optional HTML copy. The text is the
//! markdown source as typed; the HTML copy links each picked mention to its
//! user, so the mentions survive a restart. Nothing here is logged: the SDK
//! type's derived `Debug` prints the text.

use matrix_sdk::ruma::OwnedEventId;
use matrix_sdk::{ComposerDraft, ComposerDraftType};

use crate::api::{Block, Draft, EventId, Inline, MentionRef, MentionTarget, MessageText};
use crate::rich_text;

/// The SDK draft for `draft`. `None` for blank text or a reply target that is
/// no event ID.
pub(crate) fn to_sdk(draft: &Draft) -> Option<ComposerDraft> {
    let text = draft.text.expose();
    if text.trim().is_empty() {
        return None;
    }
    let draft_type = match &draft.reply_to {
        Some(event) => ComposerDraftType::Reply {
            event_id: OwnedEventId::try_from(event.as_str()).ok()?,
        },
        None => ComposerDraftType::NewMessage,
    };
    let html_text = (!draft.mentions.is_empty()).then(|| html_with_mentions(text, &draft.mentions));
    Some(ComposerDraft {
        plain_text: text.to_owned(),
        html_text,
        draft_type,
        attachments: Vec::new(),
    })
}

/// The draft stored by [`to_sdk`]. An edit draft (Tessera never stores one)
/// and blank text give `None`, so an edit cannot come back as a new message.
pub(crate) fn from_sdk(draft: ComposerDraft) -> Option<Draft> {
    let reply_to = match draft.draft_type {
        ComposerDraftType::NewMessage => None,
        ComposerDraftType::Reply { event_id } => Some(EventId::from(event_id.to_string())),
        ComposerDraftType::Edit { .. } => return None,
    };
    if draft.plain_text.trim().is_empty() {
        return None;
    }
    let mut mentions = Vec::new();
    if let Some(html) = &draft.html_text {
        let parsed = rich_text::from_html(html, &draft.plain_text);
        collect_user_mentions(&parsed.blocks, &mut mentions);
    }
    mentions.retain(|m: &MentionRef| draft.plain_text.contains(&m.label));
    Some(Draft {
        text: MessageText::from(draft.plain_text),
        mentions,
        reply_to,
    })
}

/// `text`, HTML-escaped, with each mention label linked to its user at its
/// first occurrence not taken by an earlier mention.
fn html_with_mentions(text: &str, mentions: &[MentionRef]) -> String {
    let mut found: Vec<(usize, usize, &MentionRef)> = Vec::new();
    for mention in mentions {
        if mention.label.is_empty() {
            continue;
        }
        let free = text
            .match_indices(mention.label.as_str())
            .find(|(at, label)| {
                let end = at + label.len();
                found.iter().all(|(s, e, _)| end <= *s || *at >= *e)
            });
        if let Some((at, label)) = free {
            found.push((at, at + label.len(), mention));
        }
    }
    found.sort_by_key(|(start, _, _)| *start);
    let mut html = String::with_capacity(text.len() + 64 * found.len());
    let mut from = 0;
    for (start, end, mention) in found {
        escape_html(&text[from..start], &mut html);
        html.push_str(r#"<a href="https://matrix.to/#/"#);
        escape_html(mention.user_id.as_str(), &mut html);
        html.push_str(r#"">"#);
        escape_html(&text[start..end], &mut html);
        html.push_str("</a>");
        from = end;
    }
    escape_html(&text[from..], &mut html);
    html
}

fn escape_html(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

fn collect_user_mentions(blocks: &[Block], out: &mut Vec<MentionRef>) {
    fn inlines(list: &[Inline], out: &mut Vec<MentionRef>) {
        for inline in list {
            match inline {
                Inline::Mention {
                    target: MentionTarget::User(user_id),
                    label,
                } => out.push(MentionRef {
                    label: label.clone(),
                    user_id: user_id.clone(),
                }),
                Inline::Spoiler(inner) => inlines(inner, out),
                _ => {}
            }
        }
    }
    for block in blocks {
        match block {
            Block::Paragraph(list) | Block::Heading { inlines: list, .. } => inlines(list, out),
            Block::Quote(inner) => collect_user_mentions(inner, out),
            Block::List { items, .. } => {
                for item in items {
                    collect_user_mentions(item, out);
                }
            }
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::UserId;

    fn mention(label: &str, user: &str) -> MentionRef {
        MentionRef {
            label: label.to_owned(),
            user_id: UserId::from(user),
        }
    }

    fn draft(text: &str, mentions: Vec<MentionRef>, reply: Option<&str>) -> Draft {
        Draft {
            text: MessageText::from(text),
            mentions,
            reply_to: reply.map(EventId::from),
        }
    }

    #[test]
    fn a_draft_with_mentions_and_a_reply_round_trips() {
        let original = draft(
            "@Alice: <b>hi</b> & @Bob\nsecond line",
            vec![
                mention("@Alice", "@alice:localhost"),
                mention("@Bob", "@bob:localhost"),
            ],
            Some("$event:localhost"),
        );
        let sdk = to_sdk(&original).expect("stored");
        assert_eq!(sdk.plain_text, original.text.expose());
        let html = sdk.html_text.clone().expect("html with the mentions");
        assert!(!html.contains("<b>"), "the text is escaped: {html}");
        assert_eq!(from_sdk(sdk), Some(original));
    }

    #[test]
    fn a_plain_draft_has_no_html_and_round_trips() {
        let original = draft("just text", Vec::new(), None);
        let sdk = to_sdk(&original).expect("stored");
        assert_eq!(sdk.html_text, None);
        assert_eq!(sdk.draft_type, ComposerDraftType::NewMessage);
        assert_eq!(from_sdk(sdk), Some(original));
    }

    #[test]
    fn blank_drafts_and_edit_drafts_are_not_restored() {
        assert!(to_sdk(&draft(" \n ", Vec::new(), None)).is_none());
        let edit = ComposerDraft {
            plain_text: "an edit".into(),
            html_text: None,
            draft_type: ComposerDraftType::Edit {
                event_id: OwnedEventId::try_from("$e:x").unwrap(),
            },
            attachments: Vec::new(),
        };
        assert_eq!(from_sdk(edit), None);
    }

    #[test]
    fn a_mention_whose_label_left_the_text_is_dropped() {
        let sdk = ComposerDraft {
            plain_text: "hello".into(),
            html_text: Some(r#"<a href="https://matrix.to/#/@bob:x">@Bob</a> hello"#.into()),
            draft_type: ComposerDraftType::NewMessage,
            attachments: Vec::new(),
        };
        assert_eq!(from_sdk(sdk).expect("restored").mentions, Vec::new());
    }

    #[test]
    fn a_bad_reply_target_stores_nothing() {
        assert!(to_sdk(&draft("hi", Vec::new(), Some("not an event id"))).is_none());
    }
}
