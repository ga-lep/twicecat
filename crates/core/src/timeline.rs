//! Mapping of the SDK's room timeline to the public timeline types.
//!
//! Senders and bodies are untrusted server data: nothing here logs them.

use matrix_sdk::ruma::events::StateEventContentChange;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::{OwnedRoomId, UserId as SdkUserId};
use matrix_sdk_ui::timeline::{
    AnyOtherStateEventContentChange, EmbeddedEvent, EventSendState, EventTimelineItem,
    InReplyToDetails, MembershipChange, MsgLikeKind, Profile, TimelineDetails,
    TimelineItem as SdkItem, TimelineItemContent, VirtualTimelineItem,
};

use crate::api::{
    Block, CoreError, Inline, ItemActions, MessageDetails, MessageKind, PlaceholderKind,
    ReactionGroup, ReplyPreview, ReplyState, RichText, RoomId, SendFailure, SendStatus,
    StateChange, TextStyle, TimelineItem, TimelineItemKind, UserId,
};

/// Longest quoted line of a reply, in characters.
const REPLY_BODY_CHARS: usize = 200;
/// Most reacting users kept per reaction group.
const REACTION_SENDERS: usize = 5;
/// Most readers listed on one item.
const MAX_READERS: usize = 20;

/// Maps the send state of a local echo.
pub(crate) fn map_send_state(state: &EventSendState) -> SendStatus {
    match state {
        EventSendState::NotSentYet { .. } => SendStatus::Sending,
        EventSendState::SendingFailed { .. } => SendStatus::Failed,
        EventSendState::Sent { .. } => SendStatus::Sent,
    }
}

/// Parses a room ID; a malformed ID cannot name a known room.
pub(crate) fn parse_room_id(id: &RoomId) -> Result<OwnedRoomId, CoreError> {
    matrix_sdk::ruma::RoomId::parse(id.as_str()).map_err(|_| CoreError::UnknownRoom(id.clone()))
}

/// Maps one SDK timeline item. `own` is the logged-in user.
pub(crate) fn map_item(item: &SdkItem, own: Option<&SdkUserId>) -> TimelineItem {
    let id = item.unique_id().0.clone();
    if let Some(virtual_item) = item.as_virtual() {
        let kind = match virtual_item {
            VirtualTimelineItem::DateDivider(ts) => TimelineItemKind::DateDivider {
                timestamp_ms: u64::from(ts.get()),
            },
            VirtualTimelineItem::ReadMarker => TimelineItemKind::ReadMarker,
            VirtualTimelineItem::TimelineStart => TimelineItemKind::TimelineStart,
        };
        return TimelineItem::new(id, kind);
    }
    let Some(event) = item.as_event() else {
        return TimelineItem::new(id, TimelineItemKind::Other);
    };
    let mut mapped = TimelineItem::new(id, map_event(event, own));
    mapped.read_by = receipts_without_own(event.read_receipts().keys().map(|u| &**u), own);
    mapped
}

fn user(id: &SdkUserId) -> UserId {
    UserId::from(id.to_string())
}

fn profile_name(profile: &TimelineDetails<Profile>) -> Option<String> {
    match profile {
        TimelineDetails::Ready(p) => p.display_name.clone(),
        _ => None,
    }
}

fn map_event(event: &EventTimelineItem, own: Option<&SdkUserId>) -> TimelineItemKind {
    let sender = user(event.sender());
    let sender_name = profile_name(event.sender_profile());
    let timestamp_ms = u64::from(event.timestamp().get());
    let placeholder = |what| TimelineItemKind::Placeholder {
        sender: sender.clone(),
        sender_name: sender_name.clone(),
        is_own: event.is_own(),
        timestamp_ms,
        what,
    };
    let state = |change| TimelineItemKind::State {
        sender: sender.clone(),
        sender_name: sender_name.clone(),
        timestamp_ms,
        change,
    };
    match event.content() {
        TimelineItemContent::MsgLike(content) => match &content.kind {
            MsgLikeKind::Message(message) => {
                let kind = match message.msgtype() {
                    MessageType::Text(_) => MessageKind::Text,
                    MessageType::Notice(_) => MessageKind::Notice,
                    MessageType::Emote(_) => MessageKind::Emote,
                    MessageType::Image(_) => return placeholder(PlaceholderKind::Image),
                    MessageType::Video(_) => return placeholder(PlaceholderKind::Video),
                    MessageType::Audio(_) => return placeholder(PlaceholderKind::Audio),
                    MessageType::File(_) => return placeholder(PlaceholderKind::File),
                    MessageType::Location(_) => return placeholder(PlaceholderKind::Location),
                    _ => return placeholder(PlaceholderKind::Unsupported),
                };
                let details = MessageDetails {
                    sender_name: sender_name.clone(),
                    sender_name_ambiguous: matches!(
                        event.sender_profile(),
                        TimelineDetails::Ready(p) if p.display_name_ambiguous
                    ),
                    kind,
                    content: plain_rich_text(message.body()),
                    edited: message.is_edited(),
                    highlighted: event.is_highlighted(),
                    reply_to: content.in_reply_to.as_ref().map(map_reply),
                    reactions: reaction_groups(
                        content
                            .reactions
                            .iter()
                            .map(|(key, senders)| (key.as_str(), senders.keys().map(|u| &**u))),
                        own,
                    ),
                    actions: item_actions(
                        event.can_be_replied_to(),
                        event.is_editable(),
                        event.is_own(),
                        event.is_remote_event(),
                        event.is_local_echo(),
                    ),
                    send_failure: match event.send_state() {
                        Some(EventSendState::SendingFailed { is_recoverable, .. }) => {
                            Some(SendFailure {
                                recoverable: *is_recoverable,
                            })
                        }
                        _ => None,
                    },
                };
                // A remote event has no send state: the server accepted it.
                let sent = event.send_state().map_or(SendStatus::Sent, map_send_state);
                TimelineItemKind::Message {
                    sender: sender.clone(),
                    body: message.body().to_owned(),
                    is_own: event.is_own(),
                    timestamp_ms,
                    sent,
                    details: Box::new(details),
                }
            }
            MsgLikeKind::Sticker(_) => placeholder(PlaceholderKind::Sticker),
            MsgLikeKind::Poll(_) => placeholder(PlaceholderKind::Poll),
            MsgLikeKind::LiveLocation(_) => placeholder(PlaceholderKind::Location),
            MsgLikeKind::Redacted => placeholder(PlaceholderKind::Redacted),
            MsgLikeKind::UnableToDecrypt(_) => placeholder(PlaceholderKind::UnableToDecrypt),
            MsgLikeKind::Other(_) => placeholder(PlaceholderKind::Unsupported),
        },
        TimelineItemContent::MembershipChange(change) => state(match change.change() {
            Some(c) => map_membership(c, change.user_id(), change.display_name()),
            None => other_state("m.room.member"),
        }),
        TimelineItemContent::ProfileChange(change) => {
            state(if let Some(name) = change.displayname_change() {
                StateChange::DisplayName {
                    old: name.old.clone(),
                    new: name.new.clone(),
                }
            } else if change.avatar_url_change().is_some() {
                StateChange::Avatar
            } else {
                other_state("m.room.member")
            })
        }
        TimelineItemContent::OtherState(other) => state(map_other_state(other.content())),
        TimelineItemContent::FailedToParseMessageLike { .. }
        | TimelineItemContent::FailedToParseState { .. } => state(other_state("unknown")),
        TimelineItemContent::CallInvite | TimelineItemContent::RtcNotification { .. } => {
            placeholder(PlaceholderKind::Unsupported)
        }
    }
}

fn other_state(event_type: &str) -> StateChange {
    StateChange::Other {
        event_type: event_type.to_owned(),
    }
}

fn map_other_state(content: &AnyOtherStateEventContentChange) -> StateChange {
    use AnyOtherStateEventContentChange as C;
    match content {
        C::RoomCreate(_) => StateChange::RoomCreated,
        C::RoomName(StateEventContentChange::Original { content, .. }) => StateChange::RoomName {
            name: Some(content.name.clone()).filter(|n| !n.is_empty()),
        },
        C::RoomName(StateEventContentChange::Redacted(_)) => StateChange::RoomName { name: None },
        C::RoomTopic(_) => StateChange::Topic,
        C::RoomAvatar(_) => StateChange::RoomAvatar,
        C::RoomEncryption(_) => StateChange::Encryption,
        other => StateChange::Other {
            event_type: other.event_type().to_string(),
        },
    }
}

/// Maps a membership change of `target` (named `target_name`, when known).
pub(crate) fn map_membership(
    change: MembershipChange,
    target: &SdkUserId,
    target_name: Option<String>,
) -> StateChange {
    let target = user(target);
    match change {
        MembershipChange::Joined => StateChange::Joined,
        MembershipChange::Left => StateChange::Left,
        MembershipChange::Banned | MembershipChange::KickedAndBanned => StateChange::Banned {
            target,
            target_name,
        },
        MembershipChange::Unbanned => StateChange::Unbanned {
            target,
            target_name,
        },
        MembershipChange::Kicked => StateChange::Kicked {
            target,
            target_name,
        },
        MembershipChange::Invited => StateChange::Invited {
            target,
            target_name,
        },
        MembershipChange::InvitationAccepted => StateChange::InviteAccepted,
        MembershipChange::InvitationRejected => StateChange::InviteRejected,
        MembershipChange::InvitationRevoked => StateChange::InviteRevoked {
            target,
            target_name,
        },
        MembershipChange::Knocked => StateChange::Knocked,
        MembershipChange::KnockAccepted => StateChange::KnockAccepted {
            target,
            target_name,
        },
        MembershipChange::KnockRetracted => StateChange::KnockRetracted,
        MembershipChange::KnockDenied => StateChange::KnockDenied {
            target,
            target_name,
        },
        MembershipChange::None | MembershipChange::Error | MembershipChange::NotImplemented => {
            other_state("m.room.member")
        }
    }
}

/// Maps the reply details of a message. When the quoted event is not loaded
/// its sender is unknown and left empty.
fn map_reply(reply: &InReplyToDetails) -> ReplyPreview {
    match &reply.event {
        TimelineDetails::Ready(embedded) => {
            let EmbeddedEvent {
                content,
                sender,
                sender_profile,
                ..
            } = &**embedded;
            ReplyPreview {
                sender: user(sender),
                sender_name: profile_name(sender_profile),
                body: content
                    .as_message()
                    .map_or_else(String::new, |m| reply_body(m.body())),
                state: ReplyState::Ready,
            }
        }
        other => ReplyPreview {
            sender: UserId::from(String::new()),
            sender_name: None,
            body: String::new(),
            state: match other {
                TimelineDetails::Pending => ReplyState::Loading,
                _ => ReplyState::Unavailable,
            },
        },
    }
}

/// The first line of `body`, cut to 200 characters (never inside one).
pub(crate) fn reply_body(body: &str) -> String {
    body.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(REPLY_BODY_CHARS)
        .collect()
}

/// Groups reactions, keeping the SDK order. `count` is the number of senders,
/// `own` says whether `own` is among them, and at most 5 senders are kept.
pub(crate) fn reaction_groups<'a, I, S>(groups: I, own: Option<&SdkUserId>) -> Vec<ReactionGroup>
where
    I: IntoIterator<Item = (&'a str, S)>,
    S: IntoIterator<Item = &'a SdkUserId>,
{
    groups
        .into_iter()
        .map(|(key, senders)| {
            let mut count = 0u32;
            let mut is_own = false;
            let mut kept = Vec::new();
            for sender in senders {
                count = count.saturating_add(1);
                is_own |= own == Some(sender);
                if kept.len() < REACTION_SENDERS {
                    kept.push(user(sender));
                }
            }
            ReactionGroup {
                key: key.to_owned(),
                count,
                own: is_own,
                senders: kept,
            }
        })
        .collect()
}

/// The users who read up to an item, without the logged-in user, at most 20.
pub(crate) fn receipts_without_own<'a>(
    readers: impl IntoIterator<Item = &'a SdkUserId>,
    own: Option<&SdkUserId>,
) -> Vec<UserId> {
    readers
        .into_iter()
        .filter(|u| own != Some(*u))
        .take(MAX_READERS)
        .map(user)
        .collect()
}

/// The plain text as rich text, one paragraph per line.
pub(crate) fn plain_rich_text(body: &str) -> RichText {
    if body.is_empty() {
        return RichText::default();
    }
    RichText {
        blocks: body
            .split('\n')
            .map(|line| {
                Block::Paragraph(if line.is_empty() {
                    Vec::new()
                } else {
                    vec![Inline::Text {
                        text: line.to_owned(),
                        style: TextStyle::default(),
                    }]
                })
            })
            .collect(),
    }
}

/// What the user may do with a message. Redacting is offered on own and on
/// remote events (the server decides about other people's), reacting on
/// remote events and local echoes.
pub(crate) fn item_actions(
    can_reply: bool,
    can_edit: bool,
    is_own: bool,
    is_remote: bool,
    is_local: bool,
) -> ItemActions {
    ItemActions {
        can_reply,
        can_edit,
        can_redact: is_own || is_remote,
        can_react: is_remote || is_local,
        can_copy: true,
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::event_id;

    use super::*;

    #[test]
    fn send_state_mapping_table() {
        let not_yet = EventSendState::NotSentYet { progress: None };
        let failed = EventSendState::SendingFailed {
            error: std::sync::Arc::new(matrix_sdk::Error::AuthenticationRequired),
            is_recoverable: true,
        };
        let sent = EventSendState::Sent {
            event_id: event_id!("$e:localhost").to_owned(),
        };
        assert_eq!(map_send_state(&not_yet), SendStatus::Sending);
        assert_eq!(map_send_state(&failed), SendStatus::Failed);
        assert_eq!(map_send_state(&sent), SendStatus::Sent);
    }

    fn uid(s: &str) -> matrix_sdk::ruma::OwnedUserId {
        matrix_sdk::ruma::UserId::parse(s).unwrap()
    }

    #[test]
    fn membership_mapping_table() {
        let target = uid("@t:localhost");
        let name = Some("Tee".to_owned());
        let named = || (UserId::from(target.to_string()), name.clone());
        let other = || StateChange::Other {
            event_type: "m.room.member".into(),
        };
        let with = |f: fn(UserId, Option<String>) -> StateChange| {
            let (u, n) = named();
            f(u, n)
        };
        let cases: Vec<(MembershipChange, StateChange)> = vec![
            (MembershipChange::None, other()),
            (MembershipChange::Error, other()),
            (MembershipChange::Joined, StateChange::Joined),
            (MembershipChange::Left, StateChange::Left),
            (
                MembershipChange::Banned,
                with(|target, target_name| StateChange::Banned {
                    target,
                    target_name,
                }),
            ),
            (
                MembershipChange::Unbanned,
                with(|target, target_name| StateChange::Unbanned {
                    target,
                    target_name,
                }),
            ),
            (
                MembershipChange::Kicked,
                with(|target, target_name| StateChange::Kicked {
                    target,
                    target_name,
                }),
            ),
            (
                MembershipChange::Invited,
                with(|target, target_name| StateChange::Invited {
                    target,
                    target_name,
                }),
            ),
            (
                MembershipChange::KickedAndBanned,
                with(|target, target_name| StateChange::Banned {
                    target,
                    target_name,
                }),
            ),
            (
                MembershipChange::InvitationAccepted,
                StateChange::InviteAccepted,
            ),
            (
                MembershipChange::InvitationRejected,
                StateChange::InviteRejected,
            ),
            (
                MembershipChange::InvitationRevoked,
                with(|target, target_name| StateChange::InviteRevoked {
                    target,
                    target_name,
                }),
            ),
            (MembershipChange::Knocked, StateChange::Knocked),
            (
                MembershipChange::KnockAccepted,
                with(|target, target_name| StateChange::KnockAccepted {
                    target,
                    target_name,
                }),
            ),
            (
                MembershipChange::KnockRetracted,
                StateChange::KnockRetracted,
            ),
            (
                MembershipChange::KnockDenied,
                with(|target, target_name| StateChange::KnockDenied {
                    target,
                    target_name,
                }),
            ),
            (MembershipChange::NotImplemented, other()),
        ];
        assert_eq!(cases.len(), 17);
        for (change, expected) in cases {
            assert_eq!(
                map_membership(change, &target, name.clone()),
                expected,
                "{change:?}"
            );
        }
    }

    type Group<'a> = (&'a str, Vec<&'a matrix_sdk::ruma::UserId>);

    #[test]
    fn reaction_groups_cap_senders_and_flag_own() {
        let users: Vec<_> = (0..7).map(|i| uid(&format!("@u{i}:localhost"))).collect();
        let refs: Vec<&matrix_sdk::ruma::UserId> = users.iter().map(|u| &**u).collect();
        let groups: Vec<Group<'_>> = vec![
            ("a", refs.clone()),
            ("b", vec![refs[6]]),
            ("c", vec![refs[1]]),
        ];
        let own = uid("@u6:localhost");
        let out = reaction_groups(groups, Some(&own));
        let keys: Vec<_> = out.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["a", "b", "c"], "SDK order kept");
        assert_eq!(out[0].count, 7);
        assert_eq!(out[0].senders.len(), 5);
        // The own user is the 7th sender: counted, so `own`, but not kept.
        assert!(out[0].own);
        assert!(out[1].own);
        assert!(!out[2].own);
        assert_eq!(out[2].senders, [UserId::from("@u1:localhost")]);
        assert!(reaction_groups(Vec::<Group<'_>>::new(), None).is_empty());
        let none = reaction_groups(vec![("a", vec![refs[0]])], None);
        assert!(!none[0].own);
    }

    #[test]
    fn reply_body_first_line_capped_by_chars() {
        assert_eq!(reply_body(""), "");
        assert_eq!(reply_body("one\ntwo"), "one");
        assert_eq!(reply_body("\nsecond"), "");
        let long = "é".repeat(250);
        let out = reply_body(&long);
        assert_eq!(out.chars().count(), 200);
        assert!(out.chars().all(|c| c == 'é'));
        let exact = "x".repeat(200);
        assert_eq!(reply_body(&exact), exact);
        let emoji = "😀".repeat(201);
        assert_eq!(reply_body(&emoji).chars().count(), 200);
    }

    #[test]
    fn receipts_exclude_own_and_cap_at_twenty() {
        let own = uid("@me:localhost");
        let users: Vec<_> = (0..30).map(|i| uid(&format!("@u{i}:localhost"))).collect();
        let mut all: Vec<&matrix_sdk::ruma::UserId> = users.iter().map(|u| &**u).collect();
        all.insert(3, &own);
        let out = receipts_without_own(all, Some(&own));
        assert_eq!(out.len(), 20);
        assert!(!out.iter().any(|u| u.as_str() == own.as_str()));
        assert_eq!(out[3].as_str(), "@u3:localhost");
        let few = receipts_without_own([&*own], Some(&own));
        assert!(few.is_empty());
        let no_own = receipts_without_own([&*own], None);
        assert_eq!(no_own.len(), 1);
    }

    #[test]
    fn plain_rich_text_splits_paragraphs() {
        let rt = plain_rich_text("a\nb");
        assert_eq!(rt.blocks.len(), 2);
        assert!(matches!(
            &rt.blocks[0],
            Block::Paragraph(i) if matches!(i.as_slice(), [Inline::Text { text, .. }] if text == "a")
        ));
        assert!(plain_rich_text("").blocks.is_empty());
    }

    #[test]
    fn actions_table() {
        // (replyable, editable, own, remote, local)
        let a = item_actions(true, true, true, true, false);
        assert!(a.can_reply && a.can_edit && a.can_redact && a.can_react && a.can_copy);
        let remote_other = item_actions(true, false, false, true, false);
        assert!(!remote_other.can_edit && remote_other.can_redact && remote_other.can_react);
        let local = item_actions(false, true, true, false, true);
        assert!(!local.can_reply && local.can_redact && local.can_react);
        let neither = item_actions(false, false, false, false, false);
        assert!(!neither.can_redact && !neither.can_react);
        assert!(neither.can_copy);
    }

    #[test]
    fn room_id_parsing() {
        let ok = parse_room_id(&RoomId::from("!abc:localhost")).unwrap();
        assert_eq!(ok.as_str(), "!abc:localhost");
        let bad = RoomId::from("not a room id");
        assert_eq!(
            parse_room_id(&bad).unwrap_err(),
            CoreError::UnknownRoom(bad.clone())
        );
    }
}
