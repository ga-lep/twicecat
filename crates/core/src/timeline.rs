//! Mapping of the SDK's room timeline to the public timeline types.
//!
//! Senders and bodies are untrusted server data: nothing here logs them.

use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk_ui::timeline::{
    EventSendState, EventTimelineItem, MsgLikeKind, TimelineItem as SdkItem,
};

use crate::api::{CoreError, RoomId, SendStatus, TimelineItem, TimelineItemKind, UserId};

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

/// Maps one SDK timeline item. Only text messages carry content.
pub(crate) fn map_item(item: &SdkItem) -> TimelineItem {
    let kind = item.as_event().map_or(TimelineItemKind::Other, map_event);
    TimelineItem {
        id: item.unique_id().0.clone(),
        kind,
    }
}

fn map_event(event: &EventTimelineItem) -> TimelineItemKind {
    let Some(content) = event.content().as_msglike() else {
        return TimelineItemKind::Other;
    };
    let MsgLikeKind::Message(message) = &content.kind else {
        return TimelineItemKind::Other;
    };
    if !matches!(message.msgtype(), MessageType::Text(_)) {
        return TimelineItemKind::Other;
    }
    // A remote event has no send state: the server accepted it.
    let sent = event.send_state().map_or(SendStatus::Sent, map_send_state);
    TimelineItemKind::Message {
        sender: UserId::from(event.sender().to_string()),
        body: message.body().to_owned(),
        is_own: event.is_own(),
        timestamp_ms: u64::from(event.timestamp().get()),
        sent,
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
