//! Our membership in the room state (Element Call's compatibility mode,
//! see `membership`): the delayed empty membership that leaves for us if we
//! crash (MSC4140), restarted while we are alive; the membership itself,
//! refreshed before it expires; and joining again from scratch when it is
//! gone.

use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::UpdateAction;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::unstable_v2 as update_delayed;
use matrix_sdk::ruma::api::client::delayed_events::{DelayParameters, delayed_state_event};
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};

use super::membership::{
    self, CALL_MEMBER_EVENT, DELAYED_LEAVE, DELAYED_LEAVE_RESTART, MEMBERSHIP_EXPIRES,
};
use super::now_ms;
use crate::api::{CallIntent, CoreError};

/// Refresh our membership this long before it would expire.
const REFRESH_MARGIN: Duration = Duration::from_secs(10 * 60);

/// Schedules the empty membership that fires if we stop restarting it.
/// `Ok(None)` when the server has no delayed events (MSC4140): the call
/// still works, but a crash leaves a stale membership until it expires.
pub(super) async fn schedule_delayed_leave(
    client: &Client,
    room: &Room,
    state_key: &str,
) -> Result<Option<String>, CoreError> {
    let empty: Raw<_> = Raw::new(&json!({}))
        .map_err(|_| CoreError::Internal("empty membership".into()))?
        .cast_unchecked();
    let request = delayed_state_event::unstable::Request::new_raw(
        room.room_id().to_owned(),
        state_key.to_owned(),
        StateEventType::from(CALL_MEMBER_EVENT),
        DelayParameters::Timeout {
            timeout: DELAYED_LEAVE,
        },
        empty,
    );
    match client.send(request).await {
        Ok(reply) => Ok(Some(reply.delay_id)),
        Err(e)
            if matches!(
                e.client_api_error_kind(),
                Some(ErrorKind::Unrecognized | ErrorKind::NotFound)
            ) =>
        {
            tracing::warn!("homeserver has no delayed events; crash-safe leave disabled");
            Ok(None)
        }
        Err(_) => Err(CoreError::Call("could not schedule the call leave".into())),
    }
}

/// Sends our membership; returns its event ID.
pub(super) async fn send_membership(
    room: &Room,
    state_key: &str,
    content: Value,
) -> Result<String, CoreError> {
    room.send_state_event_raw(CALL_MEMBER_EVENT, state_key, content)
        .await
        .map(|r| r.event_id.to_string())
        .map_err(|e| match e.client_api_error_kind() {
            // The room's power levels reserve call memberships (Element
            // creates rooms with `org.matrix.msc3401.call.member` at 0).
            Some(ErrorKind::Forbidden) => {
                CoreError::Call("you are not allowed to join calls in this room".into())
            }
            _ => CoreError::Call("could not publish the call membership".into()),
        })
}

/// Leaves: fires the delayed leave at once, or (without delayed events, or
/// if that failed) sends the empty membership.
pub(super) async fn remove(room: &Room, state_key: &str, delay_id: Option<String>) {
    let fired = match delay_id {
        Some(id) => room
            .client()
            .send(update_delayed::Request::new(id, UpdateAction::Send))
            .await
            .is_ok(),
        None => false,
    };
    if !fired && send_membership(room, state_key, json!({})).await.is_err() {
        tracing::warn!("could not remove the call membership; it expires by itself");
    }
}

/// What [`keep_membership`] needs.
pub(super) struct OwnMembership {
    pub(super) client: Client,
    pub(super) room: Room,
    pub(super) state_key: String,
    pub(super) user_id: String,
    pub(super) device_id: String,
    pub(super) service_url: String,
    /// A voice or a video call, kept on every refresh.
    pub(super) intent: CallIntent,
    pub(super) delay_id: Arc<Mutex<Option<String>>>,
    /// Woken by `watch_participants` when our membership is gone from the
    /// room state.
    pub(super) gone: Arc<Notify>,
}

impl OwnMembership {
    /// Restarts the delayed leave so it never fires while we are alive. If
    /// the server lost it (restart, cancelled) or already sent it, schedules
    /// a new one.
    async fn restart_delayed_leave(&self) {
        let mut guard = self.delay_id.lock().await;
        let Some(id) = guard.clone() else {
            // The homeserver has no delayed events.
            return;
        };
        match self
            .client
            .send(update_delayed::Request::new(id, UpdateAction::Restart))
            .await
        {
            Ok(_) => {}
            Err(e) if matches!(e.client_api_error_kind(), Some(ErrorKind::NotFound)) => {
                if let Ok(Some(new)) =
                    schedule_delayed_leave(&self.client, &self.room, &self.state_key).await
                {
                    *guard = Some(new);
                }
            }
            // Network trouble: try again next tick; the server sends the
            // leave if we stay away longer than the delay.
            Err(_) => {}
        }
    }

    /// Sends our membership, valid for `validity` from `created_ts` (`None`:
    /// from now, a fresh membership).
    async fn publish(&self, created_ts: Option<u64>, validity: Duration) -> Result<(), CoreError> {
        let content = membership::join_content(
            &self.user_id,
            &self.device_id,
            &self.service_url,
            self.room.room_id().as_str(),
            created_ts,
            validity,
            self.intent,
        );
        send_membership(&self.room, &self.state_key, content)
            .await
            .map(|_| ())
    }

    /// Joins again from scratch, as at the start (js-sdk
    /// `MembershipManager`, "missing own membership"): a new delayed leave,
    /// the old one cancelled first (left alone, it would fire and remove the
    /// new membership), then a fresh membership.
    async fn rejoin(&self) -> Result<(), CoreError> {
        let mut guard = self.delay_id.lock().await;
        if let Some(old) = guard.clone() {
            match self
                .client
                .send(update_delayed::Request::new(old, UpdateAction::Cancel))
                .await
            {
                Ok(_) => {}
                Err(e) if matches!(e.client_api_error_kind(), Some(ErrorKind::NotFound)) => {}
                Err(_) => return Err(CoreError::Call("could not cancel the call leave".into())),
            }
            *guard = None;
        }
        *guard = schedule_delayed_leave(&self.client, &self.room, &self.state_key).await?;
        drop(guard);
        self.publish(None, MEMBERSHIP_EXPIRES).await
    }
}

/// Keeps our membership in the room state: restarts the delayed leave,
/// re-publishes the membership with a longer validity before it expires,
/// and joins again from scratch when it is gone (the server sent our leave
/// during a stall longer than the delay, or it expired).
pub(super) async fn keep_membership(own: OwnMembership, joined_ms: u64) {
    let refresh_after =
        |validity: Duration| tokio::time::Instant::now() + validity.saturating_sub(REFRESH_MARGIN);
    let mut ticker = tokio::time::interval(DELAYED_LEAVE_RESTART);
    ticker.tick().await;
    let mut created_ts = joined_ms;
    let mut validity = MEMBERSHIP_EXPIRES;
    let mut refresh_at = refresh_after(validity);
    let mut rejoin = false;
    loop {
        tokio::select! {
            _ = ticker.tick() => own.restart_delayed_leave().await,
            () = tokio::time::sleep_until(refresh_at) => {
                validity += MEMBERSHIP_EXPIRES;
                // Counted from the first join, so the next refresh comes
                // a whole validity later.
                refresh_at = refresh_after(MEMBERSHIP_EXPIRES);
                if own.publish(Some(created_ts), validity).await.is_err() {
                    tracing::warn!("could not refresh the call membership");
                }
            }
            () = own.gone.notified() => rejoin = true,
        }
        if rejoin {
            let started = now_ms();
            match own.rejoin().await {
                Ok(()) => {
                    rejoin = false;
                    created_ts = started;
                    validity = MEMBERSHIP_EXPIRES;
                    refresh_at = refresh_after(validity);
                }
                // Tried again on the next tick.
                Err(_) => tracing::warn!("could not join the call again; retrying"),
            }
        }
    }
}
