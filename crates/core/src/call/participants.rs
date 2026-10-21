//! Who is in the call: the active memberships in the room state, reported
//! as `CallParticipants` with what the media layer knows of each (speaking,
//! muted, cleaned), and handed to the SFU manager (who publishes where), the
//! key distribution (whom to send our key) and the video tracks (whose are
//! shown).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use matrix_sdk::ruma::events::StateEventType;
use serde_json::Value;
use tokio::sync::{Notify, mpsc, watch};

use super::cue::CueHandle;
use super::e2ee::Recipient;
use super::media::SharedFlags;
use super::membership::{self, CALL_MEMBER_EVENT, Membership};
use super::sfus::Expected;
use super::video::Videos;
use super::{UserVolumes, now_ms, user_volume};
use crate::api::{
    CoreEvent, DeviceId, ListDiff, Participant, ParticipantId, RoomId, SoundSource, UserId,
};
use crate::call_sounds::Cue;

/// How often participants are re-read even without a state change, so that
/// expired memberships disappear.
const PARTICIPANT_RECHECK: Duration = Duration::from_secs(5);

/// Every membership in the room's state, active or not.
async fn read_memberships(room: &Room) -> Vec<Membership> {
    let Ok(events) = room
        .get_state_events(StateEventType::from(CALL_MEMBER_EVENT))
        .await
    else {
        return Vec::new();
    };
    events
        .into_iter()
        .filter_map(|raw| match raw {
            RawAnySyncOrStrippedState::Sync(raw) => raw.deserialize_as::<Value>().ok(),
            RawAnySyncOrStrippedState::Stripped(_) => None,
        })
        .filter_map(|event| {
            let sender = event.get("sender")?.as_str()?;
            let state_key = event.get("state_key")?.as_str()?;
            let ts = event.get("origin_server_ts")?.as_u64()?;
            membership::parse(sender, state_key, ts, event.get("content")?)
        })
        .collect()
}

/// Active memberships, oldest first.
pub(crate) async fn active_memberships(room: &Room) -> Vec<Membership> {
    let now = now_ms();
    let mut active: Vec<Membership> = read_memberships(room)
        .await
        .into_iter()
        .filter(|m| m.is_active(now))
        .collect();
    active.sort_by(|a, b| (a.joined_ms, &a.member_id).cmp(&(b.joined_ms, &b.member_id)));
    active
}

/// Active memberships (oldest first) as API participants.
async fn participants(
    room: &Room,
    active: &[Membership],
    volumes: &UserVolumes,
    flags: &SharedFlags,
) -> Vec<Participant> {
    let own_user = room.own_user_id().as_str().to_owned();
    let own_device = room
        .client()
        .device_id()
        .map(|d| d.as_str().to_owned())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(active.len());
    for m in active {
        let display_name = match matrix_sdk::ruma::UserId::parse(&m.user_id) {
            Ok(uid) => room
                .get_member_no_sync(&uid)
                .await
                .ok()
                .flatten()
                .and_then(|member| member.display_name().map(str::to_owned)),
            Err(_) => None,
        };
        let user_id = UserId::from(m.user_id.as_str());
        let volume_percent = user_volume(volumes, &user_id, SoundSource::Voice);
        let share_volume_percent = user_volume(volumes, &user_id, SoundSource::ScreenShare);
        let (speaking, muted, cleaning) = flags
            .flags
            .read()
            .map(|f| {
                (
                    f.is_speaking(&m.backend_identity),
                    f.muted.contains(&m.backend_identity),
                    f.cleaning
                        .get(&m.backend_identity)
                        .copied()
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        out.push(Participant {
            id: ParticipantId::from(m.member_id.as_str()),
            is_local: m.user_id == own_user && m.device_id == own_device,
            user_id,
            device_id: DeviceId::from(m.device_id.as_str()),
            display_name,
            muted,
            speaking,
            volume_percent,
            share_volume_percent,
            cleaning,
        });
    }
    out
}

/// Our side, for the participant watcher.
pub(super) struct Ours {
    pub(super) identity: String,
    pub(super) service: String,
    /// Who to play on which SFU, for `manage_sfus`.
    pub(super) expected: watch::Sender<Expected>,
    /// The other members' devices, for our key (`distribute_keys`).
    pub(super) recipients: watch::Sender<Vec<Recipient>>,
    /// Told when our membership, once seen, is gone (`keep_membership`).
    pub(super) gone: Arc<Notify>,
    /// Plays the cue for another person's arrival.
    pub(super) cues: CueHandle,
}

/// How long a member that left counts as still being there: its return in
/// that time (a reconnect, a re-announced or refreshed membership) is not an
/// arrival.
const REJOIN_QUIET: Duration = Duration::from_secs(60);

/// Arrivals within this time of a cue are folded into it.
const CUE_WINDOW: Duration = Duration::from_secs(3);

/// The most devices, departed devices and remembered users the detector
/// keeps. Membership lists come from other users, so the memory they can cost
/// is bounded: at most `MAX_TRACKED` devices of a list are looked at (after
/// the filtering and the per-user bound below), and when too many have left or
/// are remembered, the ones seen longest ago are forgotten.
const MAX_TRACKED: usize = 256;

/// The most devices of one user the detector looks at, so that one user (who
/// can publish any number of memberships) cannot use up the whole budget.
const MAX_PER_USER: usize = 8;

/// Tells when another person's arrival is worth a cue. Pure: the caller
/// passes the clock.
///
/// - The first update is the baseline: whoever is there makes no sound.
/// - Nothing sounds before our own membership has been seen (`own_seen`);
///   arrivals before then are taken into the baseline.
/// - Our own user's devices never sound.
/// - A device that left less than [`REJOIN_QUIET`] ago and comes back does
///   not sound; a device that stays but is refreshed (a new `joined_ms`) is
///   not an arrival.
/// - A user who was there, or already caused a cue, less than
///   [`REJOIN_QUIET`] ago and shows up on a new device does not sound: at
///   most one cue per user per minute, whatever device IDs they use.
/// - Arrivals within [`CUE_WINDOW`] of the last cue are folded into it.
///
/// Devices are told apart by `backend_identity` (`sender:device`), not by
/// `membershipID`. The user part is fixed by the server, but the device part
/// is chosen by the sender, who can rotate it at will; that is why the
/// per-user rule exists.
///
/// What is looked at: empty user IDs and our own user are dropped first, then
/// at most [`MAX_PER_USER`] devices per user, then at most [`MAX_TRACKED`]
/// devices in all, always the first in `(user, device)` order (independent of
/// the list's order and of any timestamp the sender sets). In a call of more
/// than `MAX_TRACKED` such devices, the users sorting last are not seen; one
/// of them may cue when it comes into view after more than [`REJOIN_QUIET`].
#[derive(Debug, Default)]
pub(super) struct Arrivals {
    /// The first update has been seen.
    baselined: bool,
    /// The devices present at the last update.
    present: BTreeSet<String>,
    /// Devices that left, and when.
    left: BTreeMap<String, Instant>,
    /// Users, and when they were last present or last caused a cue.
    users: BTreeMap<String, Instant>,
    /// When the last cue was due.
    last_cue: Option<Instant>,
}

impl Arrivals {
    /// Takes the current active memberships; true when a cue is due.
    pub(super) fn update(
        &mut self,
        active: &[Membership],
        own_user: &str,
        own_seen: bool,
        now: Instant,
    ) -> bool {
        // Who is there now: filter first (nobody, our own user), then bound
        // per user, then in total; a sorted order, so the choice is the same
        // whatever the order of the list.
        let candidates: BTreeSet<(&str, &str)> = active
            .iter()
            .filter(|m| !m.user_id.is_empty() && m.user_id != own_user)
            .map(|m| (m.user_id.as_str(), m.backend_identity.as_str()))
            .collect();
        let mut per_user = 0usize;
        let mut previous_user = None;
        let mut current: BTreeMap<&str, &str> = BTreeMap::new();
        for (user, identity) in candidates {
            if previous_user != Some(user) {
                previous_user = Some(user);
                per_user = 0;
            }
            per_user += 1;
            if per_user > MAX_PER_USER {
                continue;
            }
            current.insert(identity, user);
            if current.len() >= MAX_TRACKED {
                break;
            }
        }
        let was_baselined = std::mem::replace(&mut self.baselined, true);

        self.left
            .retain(|_, left_at| now.saturating_duration_since(*left_at) < REJOIN_QUIET);
        self.users
            .retain(|_, seen_at| now.saturating_duration_since(*seen_at) < REJOIN_QUIET);
        for gone in self
            .present
            .iter()
            .filter(|id| !current.contains_key(id.as_str()))
        {
            self.left.insert(gone.clone(), now);
        }
        while self.left.len() > MAX_TRACKED {
            // Forget whoever left longest ago.
            let Some(oldest) = self
                .left
                .iter()
                .min_by_key(|(id, left_at)| (**left_at, (*id).clone()))
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.left.remove(&oldest);
        }

        let mut arrived = false;
        for (id, user) in &current {
            if self.present.contains(*id) {
                continue;
            }
            let rejoined = self.left.remove(*id).is_some();
            let user_recent = self.users.contains_key(*user);
            if !rejoined && !user_recent {
                arrived = true;
                // Not again for this user within the quiet time, even on
                // another device ID.
                self.users.insert((*user).to_owned(), now);
            }
        }
        // Whoever is there now was seen now.
        for user in current.values() {
            self.users.insert((*user).to_owned(), now);
        }
        while self.users.len() > MAX_TRACKED {
            // Forget whoever was seen longest ago.
            let Some(oldest) = self
                .users
                .iter()
                .min_by_key(|(user, seen_at)| (**seen_at, (*user).clone()))
                .map(|(user, _)| user.clone())
            else {
                break;
            };
            self.users.remove(&oldest);
        }
        self.present = current.keys().map(|id| (*id).to_owned()).collect();

        let due = arrived
            && was_baselined
            && own_seen
            && self
                .last_cue
                .is_none_or(|at| now.saturating_duration_since(at) >= CUE_WINDOW);
        if due {
            self.last_cue = Some(now);
        }
        due
    }
}

/// Emits `CallParticipants` (a full reset) whenever the active set changes,
/// and tells the SFU manager who publishes where.
pub(super) async fn watch_participants(
    room: Room,
    room_id: RoomId,
    events: mpsc::Sender<CoreEvent>,
    volumes: UserVolumes,
    flags: SharedFlags,
    videos: Videos,
    ours: Ours,
) {
    let mut info = room.subscribe_info();
    let mut ticker = tokio::time::interval(PARTICIPANT_RECHECK);
    let mut last: Option<Vec<Participant>> = None;
    // Our membership reaches the store some time after we send it: only its
    // disappearance after that counts.
    let mut seen_own = false;
    let mut arrivals = Arrivals::default();
    loop {
        let active = active_memberships(&room).await;
        if active.iter().any(|m| m.backend_identity == ours.identity) {
            seen_own = true;
        } else if seen_own {
            seen_own = false;
            tracing::warn!("our call membership is gone; joining again");
            ours.gone.notify_one();
        }
        if arrivals.update(
            &active,
            room.own_user_id().as_str(),
            seen_own,
            Instant::now(),
        ) {
            ours.cues.play(Cue::OtherJoined);
        }
        videos.set_members(&active);
        let expected = membership::expected_by_sfu(&active, &ours.identity, &ours.service);
        ours.expected.send_if_modified(|e| {
            let changed = *e != expected;
            *e = expected;
            changed
        });
        let recipients: Vec<Recipient> = active
            .iter()
            .filter(|m| m.backend_identity != ours.identity)
            .map(|m| Recipient {
                user_id: m.user_id.clone(),
                device_id: m.device_id.clone(),
                since: m.joined_ms,
            })
            .collect();
        ours.recipients.send_if_modified(|r| {
            let changed = *r != recipients;
            *r = recipients;
            changed
        });
        let now = participants(&room, &active, &volumes, &flags).await;
        if last.as_ref() != Some(&now) {
            let event = CoreEvent::CallParticipants {
                room_id: room_id.clone(),
                diffs: vec![ListDiff::Reset {
                    values: now.clone(),
                }],
            };
            if events.send(event).await.is_err() {
                return;
            }
            last = Some(now);
        }
        tokio::select! {
            changed = info.next() => if changed.is_none() { return },
            () = flags.changed.notified() => {}
            _ = ticker.tick() => {}
        }
    }
}

#[cfg(test)]
mod arrivals {
    use super::*;

    const ME: &str = "@alice:example.org";

    fn device(user: &str, device: &str) -> Membership {
        Membership {
            user_id: user.into(),
            device_id: device.into(),
            member_id: format!("{user}:{device}"),
            backend_identity: format!("{user}:{device}"),
            livekit_service_url: None,
            focus: None,
            joined_ms: 1,
            expires_ms: 1_000,
        }
    }

    fn carol() -> Membership {
        device("@carol:example.org", "C1")
    }

    fn dave() -> Membership {
        device("@dave:example.org", "D1")
    }

    fn eve() -> Membership {
        device("@eve:example.org", "E1")
    }

    fn own() -> Membership {
        device(ME, "A1")
    }

    /// An `Arrivals` past its baseline, with our membership seen and `list`
    /// present.
    fn settled(list: &[Membership], t0: Instant) -> Arrivals {
        let mut arrivals = Arrivals::default();
        assert!(!arrivals.update(list, ME, true, t0));
        arrivals
    }

    fn secs(t0: Instant, s: u64) -> Instant {
        t0 + Duration::from_secs(s)
    }

    #[test]
    fn the_first_update_never_cues_whoever_is_there() {
        let t0 = Instant::now();
        let mut arrivals = Arrivals::default();
        assert!(!arrivals.update(&[own(), carol(), dave()], ME, true, t0));
        // And nothing changed afterwards.
        assert!(!arrivals.update(&[own(), carol(), dave()], ME, true, secs(t0, 5)));
    }

    #[test]
    fn nothing_cues_before_our_own_membership_is_seen() {
        let t0 = Instant::now();
        let mut arrivals = Arrivals::default();
        assert!(!arrivals.update(&[carol()], ME, false, t0));
        // An arrival before our membership shows up is silent...
        assert!(!arrivals.update(&[carol(), dave()], ME, false, secs(t0, 10)));
        // ...and part of the baseline once it does.
        assert!(!arrivals.update(&[own(), carol(), dave()], ME, true, secs(t0, 15)));
        assert!(arrivals.update(&[own(), carol(), dave(), eve()], ME, true, secs(t0, 20)));
    }

    #[test]
    fn a_new_member_of_another_user_cues_once() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own()], t0);
        assert!(arrivals.update(&[own(), carol()], ME, true, secs(t0, 10)));
        assert!(!arrivals.update(&[own(), carol()], ME, true, secs(t0, 15)));
        assert!(!arrivals.update(&[own(), carol()], ME, true, secs(t0, 20)));
    }

    #[test]
    fn a_new_device_of_our_own_user_does_not_cue() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own()], t0);
        let phone = device(ME, "PHONE");
        assert!(!arrivals.update(&[own(), phone], ME, true, secs(t0, 10)));
    }

    #[test]
    fn our_own_user_on_a_new_device_does_not_cue_even_when_it_was_not_in_the_baseline() {
        // Our own user is not in the per-user quiet map here (the baseline has
        // only Carol), so the per-user rule cannot hide a missing own-user
        // filter: only the filter keeps these two devices silent.
        let t0 = Instant::now();
        let mut arrivals = settled(&[carol()], t0);
        let phone = device(ME, "PHONE");
        let tablet = device(ME, "TABLET");
        assert!(!arrivals.update(&[carol(), phone.clone()], ME, true, secs(t0, 10)));
        assert!(!arrivals.update(&[carol(), phone, tablet], ME, true, secs(t0, 20)));
    }

    #[test]
    fn a_member_that_comes_back_within_a_minute_does_not_cue_but_after_one_does() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own(), carol()], t0);
        assert!(!arrivals.update(&[own()], ME, true, secs(t0, 10)), "left");
        assert!(
            !arrivals.update(&[own(), carol()], ME, true, secs(t0, 40)),
            "back after 30 s"
        );

        // Gone again, and away for longer than a minute.
        assert!(!arrivals.update(&[own()], ME, true, secs(t0, 50)));
        assert!(
            arrivals.update(&[own(), carol()], ME, true, secs(t0, 50 + 61)),
            "back after 61 s"
        );
    }

    #[test]
    fn an_unchanged_list_or_a_refreshed_member_does_not_cue() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own(), carol()], t0);
        assert!(!arrivals.update(&[own(), carol()], ME, true, secs(t0, 10)));
        let mut refreshed = carol();
        refreshed.joined_ms += 5_000;
        refreshed.expires_ms += 5_000;
        assert!(!arrivals.update(&[own(), refreshed], ME, true, secs(t0, 20)));
    }

    #[test]
    fn arrivals_within_three_seconds_give_one_cue_and_a_later_one_another() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own()], t0);
        let ms = Duration::from_millis;
        assert!(arrivals.update(&[own(), carol()], ME, true, t0 + ms(10_000)));
        assert!(!arrivals.update(&[own(), carol(), dave()], ME, true, t0 + ms(11_000)));
        assert!(!arrivals.update(&[own(), carol(), dave(), eve()], ME, true, t0 + ms(12_999)));
        let frank = device("@frank:example.org", "F1");
        assert!(
            arrivals.update(
                &[own(), carol(), dave(), eve(), frank],
                ME,
                true,
                t0 + ms(13_000)
            ),
            "3 s after the cue"
        );
    }

    #[test]
    fn several_arrivals_in_one_update_give_one_cue() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own()], t0);
        assert!(arrivals.update(&[own(), carol(), dave(), eve()], ME, true, secs(t0, 10)));
    }

    #[test]
    fn odd_membership_data_does_not_panic_or_cue_wrongly() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own()], t0);
        // Duplicate IDs count once; an empty user is no one.
        let nobody = device("", "");
        assert!(!arrivals.update(&[own(), nobody.clone(), nobody], ME, true, secs(t0, 10)));
        assert!(arrivals.update(&[own(), carol(), carol()], ME, true, secs(t0, 20)));
        assert!(!arrivals.update(&[], "", true, secs(t0, 30)));
        assert!(!arrivals.update(&[], ME, true, secs(t0, 40)));
    }

    #[test]
    fn a_large_hostile_list_is_bounded_and_does_not_flap() {
        let t0 = Instant::now();
        let hostile = |n: usize| -> Vec<Membership> {
            (0..n)
                .map(|i| device(&format!("@bot{i}:example.org"), &format!("D{i}")))
                .collect()
        };
        let mut arrivals = settled(&[own()], t0);
        let big = hostile(50_000);
        // A flood of arrivals is one cue...
        assert!(arrivals.update(&big, ME, true, secs(t0, 10)));
        // ...and the same list again is not new, however long it is.
        assert!(!arrivals.update(&big, ME, true, secs(t0, 20)));
        assert!(!arrivals.update(&big, ME, true, secs(t0, 30)));
        assert!(arrivals.present.len() <= MAX_TRACKED);

        // Everyone leaving and coming back over and over keeps the memory
        // of who left bounded.
        for round in 0..20u64 {
            let now = secs(t0, 40 + round * 2);
            let list = hostile(50_000 + usize::try_from(round).unwrap() * 1_000);
            let _ = arrivals.update(&list[40_000..], ME, true, now);
            let _ = arrivals.update(&[], ME, true, now);
            assert!(arrivals.left.len() <= MAX_TRACKED);
            assert!(arrivals.present.len() <= MAX_TRACKED);
        }
    }

    #[test]
    fn rotating_device_ids_of_one_user_cue_once_a_minute() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own()], t0);
        let rotated = |n: u64| device("@mallory:example.org", &format!("R{n}"));
        assert!(arrivals.update(&[own(), rotated(0)], ME, true, secs(t0, 10)));
        // A new device ID every 3 s (the burst window) for a minute: silent.
        for n in 1..=18u64 {
            assert!(
                !arrivals.update(&[own(), rotated(n)], ME, true, secs(t0, 10 + 3 * n)),
                "rotation {n}"
            );
        }
        // Gone for more than a minute, then a new device: a cue again.
        assert!(!arrivals.update(&[own()], ME, true, secs(t0, 70)));
        assert!(arrivals.update(&[own(), rotated(99)], ME, true, secs(t0, 70 + 61)));
    }

    #[test]
    fn a_user_back_on_a_new_device_within_a_minute_does_not_cue() {
        let t0 = Instant::now();
        let mut arrivals = settled(&[own(), carol()], t0);
        assert!(!arrivals.update(&[own()], ME, true, secs(t0, 10)));
        let new_device = device("@carol:example.org", "C2");
        assert!(!arrivals.update(&[own(), new_device.clone()], ME, true, secs(t0, 40)));
        assert!(!arrivals.update(&[own()], ME, true, secs(t0, 50)));
        assert!(arrivals.update(&[own(), new_device], ME, true, secs(t0, 50 + 61)));
    }

    fn hostile_user(n: usize) -> Vec<Membership> {
        (0..n)
            .map(|i| {
                let mut m = device("@aaa:example.org", &format!("H{i}"));
                m.joined_ms = 0;
                m.expires_ms = u64::MAX;
                m
            })
            .collect()
    }

    #[test]
    fn a_hostile_user_with_many_memberships_does_not_stop_others_cueing() {
        let t0 = Instant::now();
        let mut list = vec![own()];
        list.extend(hostile_user(300));
        let mut arrivals = settled(&list, t0);
        list.push(carol());
        assert!(arrivals.update(&list, ME, true, secs(t0, 10)));
        assert!(arrivals.present.len() <= MAX_PER_USER + 1);
    }

    #[test]
    fn our_own_devices_do_not_use_up_the_slots() {
        let t0 = Instant::now();
        // 300 devices of our own user, first in the list.
        let mut list: Vec<Membership> = (0..300)
            .map(|i| {
                let mut m = device(ME, &format!("A{i:03}"));
                m.joined_ms = 0;
                m
            })
            .collect();
        let mut arrivals = settled(&list, t0);
        list.push(carol());
        assert!(arrivals.update(&list, ME, true, secs(t0, 10)));
    }

    #[test]
    fn many_users_each_get_a_few_devices_and_the_total_is_bounded() {
        let t0 = Instant::now();
        let mut list = vec![own()];
        for u in 0..40 {
            for d in 0..20 {
                list.push(device(&format!("@u{u:02}:example.org"), &format!("D{d}")));
            }
        }
        let mut arrivals = settled(&list, t0);
        assert!(arrivals.present.len() <= MAX_TRACKED);
        assert!(arrivals.users.len() <= MAX_TRACKED);
        // The same list, shuffled, is the same view: no spurious cue.
        list.reverse();
        assert!(!arrivals.update(&list, ME, true, secs(t0, 10)));
        assert!(!arrivals.update(&list, ME, true, secs(t0, 200)));
    }

    #[test]
    fn a_device_window_shift_does_not_cue_a_long_present_user() {
        let t0 = Instant::now();
        // One user with 12 devices, device IDs sorting before and after
        // each other as the user refreshes: only 8 are looked at.
        let mut list = vec![own()];
        list.extend((0..12).map(|i| device("@carol:example.org", &format!("D{i:02}"))));
        let mut arrivals = settled(&list, t0);
        // The first devices leave: others come into the window.
        let rest: Vec<Membership> = list.iter().skip(5).cloned().collect();
        assert!(!arrivals.update(&rest, ME, true, secs(t0, 10)));
        assert!(!arrivals.update(&list, ME, true, secs(t0, 20)));
    }
}
