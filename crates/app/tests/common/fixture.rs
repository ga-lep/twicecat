//! Fixture apps for the headless UI gallery.
//!
//! Each [`Scenario`] builds an `App` through `App::new` and `App::update`
//! only, with `CoreEvent`s from `twicecat_core::api` and the same messages a
//! click would send. Commands go to [`NullSink`], which drops them: no core
//! runs, no network is used and no audio device is opened.
//!
//! Every name is made up (`example.org`); there are no tokens, passwords or
//! real keys. The "recovery key" is an obvious dummy.

use std::time::Duration;

use iced::Task;
use twicecat_app::audio_settings::AudioMessage;
use twicecat_app::calls::CallMessage;
use twicecat_app::rooms::RoomsMessage;
use twicecat_app::security::{Overlay, ReauthPrompt, RecoverySetup};
use twicecat_app::sessions::SessionsMessage;
use twicecat_app::settings::SettingsMessage;
use twicecat_app::switcher::SwitcherMessage;
use twicecat_app::threads::ThreadsMessage;
use twicecat_app::upload::{Outcome, Prepared, Refusal, Source, UploadPaths};
use twicecat_app::verification::Phase;
use twicecat_app::{App, Message, Screen, Startup};
use twicecat_app::{CommandSink, EventsSlot};
use twicecat_core::api::{
    AudioDevice, AudioDeviceId, AudioDevices, AudioSettings, AudioStats, Block, CallIntent,
    CallState, CancelReason, CleaningStatus, Command, CoreError, CoreEvent, CrossSigning, DeviceId,
    DeviceSummary, DeviceTrust, DeviceVerification, EnhancerBackend, EventId, GpuBackend,
    GpuEnhancement, Inline, InputLevel, InputMode, ItemActions, ListDiff, MediaContent, MediaKind,
    MembershipView, MentionTarget, MessageDetails, MessageKind, MessagePreview, PaginationState,
    Participant, ParticipantId, PlaceholderKind, PowerLevelView, QrImage, ReactionGroup,
    ReauthAction, RecoveryKey, RecoveryProgress, RecoveryStatus, ReplyPreview, ReplyState,
    RichText, Role, RoomDetailsView, RoomId, RoomMemberView, RoomNotificationModeView,
    RoomPermissions, RoomSummary, SasEmoji, SecurityStatus, SendFailure, SendStatus, Shield,
    ShieldCode, SpaceView, StateChange, SyncState, TextStyle, ThreadSummaryView, TimelineItem,
    TimelineItemKind, TypingUser, UploadProgress, UserId, UserTrust, UtdReason, VerificationId,
    VerificationPeer, VerificationPhase,
};

/// Drops every command: the gallery never talks to a core.
pub struct NullSink;

impl CommandSink for NullSink {
    fn send(&self, _command: Command) -> Option<Task<Message>> {
        None
    }

    fn deadline(&self, _after: Duration) -> Task<Message> {
        Task::none()
    }

    fn timer(&self, _after: Duration, _message: Message) -> Task<Message> {
        Task::none()
    }

    fn command_sender(&self) -> Option<tokio::sync::mpsc::Sender<Command>> {
        None
    }

    fn runtime(&self) -> Option<tokio::runtime::Handle> {
        None
    }
}

/// One screen of the gallery.
pub struct Scenario {
    /// File-name safe, unique.
    pub name: &'static str,
    /// Shows the room list, so the gallery also draws it with the rail.
    pub room_list: bool,
    /// Builds the app in this screen's state.
    pub build: fn() -> App,
    /// Whether the app really is in the state the name promises.
    pub reached: fn(&App) -> bool,
}

fn core(event: CoreEvent) -> Message {
    Message::Core(Box::new(event))
}

fn feed(app: &mut App, event: CoreEvent) {
    let _ = app.update(core(event));
}

fn press(app: &mut App, message: Message) {
    let _ = app.update(message);
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).expect("after 1970")
}

fn room(name: &str) -> RoomId {
    RoomId::from(format!("!{name}:example.org"))
}

fn user(name: &str) -> UserId {
    UserId::from(format!("@{name}:example.org"))
}

// ---- Account and room list ---------------------------------------------

fn fresh() -> App {
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let (app, _boot) = App::new(Startup {
        sink: Box::new(NullSink),
        events: EventsSlot::new(rx),
    });
    app
}

fn logged_out() -> App {
    let mut app = fresh();
    feed(&mut app, CoreEvent::NoSession);
    app
}

fn logged_in_loading() -> App {
    let mut app = fresh();
    feed(
        &mut app,
        CoreEvent::LoggedIn {
            user_id: user("me"),
            device_id: DeviceId::from("GALLERYDEV"),
            restored: false,
        },
    );
    feed(&mut app, CoreEvent::SyncState(SyncState::Running));
    app
}

fn summary(name: &str, title: &str) -> RoomSummary {
    let mut room = RoomSummary::new(room(name), title.to_owned());
    room.is_encrypted = true;
    room
}

fn preview(sender: &str, name: &str, body: &str, minutes_ago: u64) -> MessagePreview {
    MessagePreview {
        sender: user(sender),
        sender_name: Some(name.to_owned()),
        body: body.to_owned(),
        timestamp_ms: now_ms() - minutes_ago * 60_000,
        is_own: false,
    }
}

fn rooms() -> Vec<RoomSummary> {
    let mut lounge = summary("lounge", "Lounge");
    lounge.unread_messages = 3;
    lounge.unread_notifications = 3;
    lounge.last_message = Some(preview("bea", "Bea Example", "See you at the stand-up", 4));

    let mut design = summary("design", "Design review");
    design.unread_messages = 12;
    design.unread_notifications = 12;
    design.unread_mentions = 2;
    design.highlights = 2;
    design.last_message = Some(preview("cy", "Cy Example", "@me can you look at this?", 9));

    let mut alice = summary("alice", "Alice Example");
    alice.is_dm = true;
    alice.dm_peer = Some(user("alice"));
    alice.last_message = Some(preview(
        "alice",
        "Alice Example",
        "Call me when you are free",
        30,
    ));

    let mut alerts = summary("alerts", "Build alerts");
    alerts.unread_messages = 40;
    alerts.notification_mode = Some(RoomNotificationModeView::Mute);

    let mut hangout = summary("hangout", "Voice hangout");
    hangout.is_call_room = true;

    let mut invite = summary("book-club", "Book club");
    invite.is_invite = true;
    invite.is_encrypted = false;

    let mut standup = summary("standup", "Stand-up");
    standup.marked_unread = true;

    let mut open = summary("open", "Open house");
    open.is_encrypted = false;
    open.last_message = Some(preview("dee", "Dee Example", "Welcome everyone", 600));

    let mut bob = summary("bob", "Bob Example");
    bob.is_dm = true;
    bob.dm_peer = Some(user("bob"));

    let mut out = vec![
        lounge, design, alice, alerts, hangout, invite, standup, open, bob,
    ];
    for (i, title) in ["Garden club", "Kitchen", "Photos", "Reading list", "Travel"]
        .iter()
        .enumerate()
    {
        let mut filler = summary(&format!("filler{i}"), title);
        filler.last_message = Some(preview("eve", "Eve Example", "Thanks!", 1_000 + i as u64));
        out.push(filler);
    }
    out
}

fn main_no_room() -> App {
    let mut app = logged_in_loading();
    let list = rooms();
    let total = u32::try_from(list.len()).expect("a few rooms");
    feed(
        &mut app,
        CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: list }]),
    );
    feed(
        &mut app,
        CoreEvent::RoomListLoaded {
            total_rooms: Some(total),
        },
    );
    feed(
        &mut app,
        CoreEvent::Spaces(vec![
            SpaceView {
                id: room("space-home"),
                name: "Home".to_owned(),
                avatar: None,
                level: 0,
                children: 4,
            },
            SpaceView {
                id: room("space-work"),
                name: "Work".to_owned(),
                avatar: None,
                level: 0,
                children: 6,
            },
        ]),
    );
    feed(
        &mut app,
        CoreEvent::UnreadTotals {
            notifications: 55,
            highlights: 2,
        },
    );
    feed(
        &mut app,
        CoreEvent::ActiveCall {
            room_id: room("hangout"),
            participants: 2,
        },
    );
    app
}

// ---- Timeline -----------------------------------------------------------

fn run(text: &str, style: TextStyle) -> Inline {
    Inline::Text {
        text: text.to_owned(),
        style,
    }
}

fn plain(text: &str) -> Inline {
    run(text, TextStyle::default())
}

fn paragraph(text: &str) -> RichText {
    RichText {
        blocks: vec![Block::Paragraph(vec![plain(text)])],
    }
}

fn details(name: &str, content: RichText) -> MessageDetails {
    MessageDetails {
        sender_name: Some(name.to_owned()),
        content,
        actions: ItemActions {
            can_reply: true,
            can_react: true,
            can_copy: true,
            ..ItemActions::default()
        },
        ..MessageDetails::default()
    }
}

struct Say {
    id: &'static str,
    sender: &'static str,
    own: bool,
    minutes_ago: u64,
    sent: SendStatus,
    body: String,
    details: MessageDetails,
}

fn say(
    id: &'static str,
    sender: &'static str,
    name: &'static str,
    minutes_ago: u64,
    body: &str,
) -> Say {
    Say {
        id,
        sender,
        own: sender == "me",
        minutes_ago,
        sent: SendStatus::Sent,
        body: body.to_owned(),
        details: details(name, paragraph(body)),
    }
}

impl Say {
    fn item(self) -> TimelineItem {
        let mut item = TimelineItem::new(
            self.id.to_owned(),
            TimelineItemKind::Message {
                sender: user(self.sender),
                body: self.body,
                is_own: self.own,
                timestamp_ms: now_ms() - self.minutes_ago * 60_000,
                sent: self.sent,
                details: Box::new(self.details),
            },
        );
        if self.sent == SendStatus::Sent {
            item.event_id = Some(EventId::from(format!("${}", self.id)));
        }
        item
    }
}

fn state(id: &str, sender: &str, name: &str, change: StateChange) -> TimelineItem {
    TimelineItem::new(
        id.to_owned(),
        TimelineItemKind::State {
            sender: user(sender),
            sender_name: Some(name.to_owned()),
            timestamp_ms: now_ms() - 3_000_000,
            change,
        },
    )
}

fn placeholder(id: &str, what: PlaceholderKind, utd: Option<UtdReason>) -> TimelineItem {
    TimelineItem::new(
        id.to_owned(),
        TimelineItemKind::Placeholder {
            sender: user("bea"),
            sender_name: Some("Bea Example".to_owned()),
            is_own: false,
            timestamp_ms: now_ms() - 50 * 60_000,
            what,
            utd,
        },
    )
}

fn rich_content() -> RichText {
    let bold = TextStyle {
        bold: true,
        ..TextStyle::default()
    };
    let italic = TextStyle {
        italic: true,
        ..TextStyle::default()
    };
    let code = TextStyle {
        code: true,
        ..TextStyle::default()
    };
    RichText {
        blocks: vec![
            Block::Heading {
                level: 2,
                inlines: vec![plain("Plan for Friday")],
            },
            Block::Paragraph(vec![
                plain("We should "),
                run("really", bold),
                plain(" ship "),
                run("this week", italic),
                plain(", see "),
                Inline::Link {
                    text: "the notes".to_owned(),
                    url: "https://example.org/notes".to_owned(),
                    style: TextStyle::default(),
                },
                plain(" or ask "),
                Inline::Mention {
                    target: MentionTarget::User(user("me")),
                    label: "@me".to_owned(),
                },
                plain(". Run "),
                run("cargo test", code),
                plain(". The answer is "),
                Inline::Spoiler(vec![plain("forty-two")]),
                plain("."),
            ]),
            Block::Quote(vec![Block::Paragraph(vec![plain(
                "Measure twice, cut once.",
            )])]),
            Block::List {
                ordered: false,
                start: None,
                items: vec![
                    vec![Block::Paragraph(vec![plain("Write the plan")])],
                    vec![Block::Paragraph(vec![plain("Review it")])],
                ],
            },
            Block::Code {
                text: "fn main() {\n    println!(\"hello\");\n}".to_owned(),
            },
            Block::Rule,
        ],
    }
}

/// Every kind of item the timeline draws, oldest first.
fn timeline_items() -> Vec<TimelineItem> {
    let mut items = vec![TimelineItem::new(
        "start".to_owned(),
        TimelineItemKind::TimelineStart,
    )];
    items.push(state(
        "s-created",
        "bea",
        "Bea Example",
        StateChange::RoomCreated,
    ));
    items.push(state(
        "s-enc",
        "bea",
        "Bea Example",
        StateChange::Encryption,
    ));
    items.push(state("s-joined", "cy", "Cy Example", StateChange::Joined));
    items.push(TimelineItem::new(
        "d1".to_owned(),
        TimelineItemKind::DateDivider {
            timestamp_ms: now_ms() - 26 * 3_600_000,
        },
    ));
    items.push(say("m1", "bea", "Bea Example", 1_500, "Good morning, everyone.").item());
    items.push(
        say(
            "m2",
            "bea",
            "Bea Example",
            1_499,
            "A second message in a row, so it groups under the first.",
        )
        .item(),
    );
    let mut rich = say("m3", "cy", "Cy Example", 1_400, "Plan for Friday");
    rich.details.content = rich_content();
    items.push(rich.item());
    items.push(placeholder(
        "p-utd",
        PlaceholderKind::UnableToDecrypt,
        Some(UtdReason::SentBeforeWeJoined),
    ));
    items.push(placeholder("p-red", PlaceholderKind::Redacted, None));
    items.push(placeholder("p-poll", PlaceholderKind::Poll, None));
    items.push(TimelineItem::new(
        "other".to_owned(),
        TimelineItemKind::Other,
    ));
    items.push(TimelineItem::new(
        "d2".to_owned(),
        TimelineItemKind::DateDivider {
            timestamp_ms: now_ms(),
        },
    ));
    items.push(
        say(
            "m4",
            "alice",
            "Alice Example",
            90,
            "A long message that has to wrap over several lines when the pane is narrow, \
             with enough words to show how the text flows, how the line height feels, and \
             where the lines break in this font at this size.",
        )
        .item(),
    );
    let mut reply = say("m5", "me", "Me", 80, "Thanks, I will read it now.");
    reply.details.reply_to = Some(ReplyPreview {
        event_id: EventId::from("$m4"),
        sender: user("alice"),
        sender_name: Some("Alice Example".to_owned()),
        body: "A long message that has to wrap".to_owned(),
        state: ReplyState::Ready,
    });
    reply.details.edited = true;
    reply.details.actions.can_edit = true;
    reply.details.actions.can_redact = true;
    items.push(reply.item());
    let mut reacted = say("m6", "bea", "Bea Example", 70, "Lunch is on me today.");
    reacted.details.reactions = vec![
        ReactionGroup {
            key: "\u{1F44D}".to_owned(),
            count: 3,
            own: true,
            senders: vec![user("me"), user("cy"), user("alice")],
        },
        ReactionGroup {
            key: "\u{1F389}".to_owned(),
            count: 1,
            own: false,
            senders: vec![user("cy")],
        },
    ];
    items.push(reacted.item());
    let mut mention = say("m7", "cy", "Cy Example", 65, "@me can you look at this?");
    mention.details.highlighted = true;
    items.push(mention.item());
    let mut emote = say("m8", "alice", "Alice Example", 60, "waves hello");
    emote.details.kind = MessageKind::Emote;
    items.push(emote.item());
    let mut notice = say(
        "m9",
        "alice",
        "Alice Example",
        59,
        "Deploy finished without errors.",
    );
    notice.details.kind = MessageKind::Notice;
    items.push(notice.item());
    let mut shield = say(
        "m10",
        "dee",
        "Dee Example",
        55,
        "Sent from a device that is not verified.",
    );
    shield.details.shield = Some(Shield::Red(ShieldCode::UnsignedDevice));
    items.push(shield.item());
    let mut image = say("img0", "bea", "Bea Example", 50, "holiday.png");
    let mut media = MediaContent::new(MediaKind::Image, "holiday.png".to_owned());
    media.mimetype = Some("image/png".to_owned());
    media.size = Some(180_000);
    media.width = Some(640);
    media.height = Some(480);
    image.details.media = Some(Box::new(media));
    items.push(image.item());
    let mut file = say("file0", "bea", "Bea Example", 49, "report.pdf");
    let mut media = MediaContent::new(MediaKind::File, "report.pdf".to_owned());
    media.mimetype = Some("application/pdf".to_owned());
    media.size = Some(2_400_000);
    file.details.media = Some(Box::new(media));
    items.push(file.item());
    let mut audio = say("audio0", "cy", "Cy Example", 48, "voice-note.ogg");
    let mut media = MediaContent::new(MediaKind::Audio, "voice-note.ogg".to_owned());
    media.duration_ms = Some(23_000);
    media.size = Some(90_000);
    audio.details.media = Some(Box::new(media));
    items.push(audio.item());
    let mut threaded = say(
        "thread-root",
        "alice",
        "Alice Example",
        45,
        "Let us discuss the schedule in a thread.",
    );
    threaded.details.thread = Some(ThreadSummaryView {
        replies: 3,
        latest_sender: Some(user("bea")),
        latest_sender_name: Some("Bea Example".to_owned()),
        latest_preview: Some("Friday works for me".to_owned()),
    });
    items.push(threaded.item());
    // An upload in progress, then a send that failed and a local echo.
    let mut uploading = say("up0", "me", "Me", 6, "clip.mp4");
    uploading.sent = SendStatus::Sending;
    let mut media = MediaContent::new(MediaKind::Image, "scan.png".to_owned());
    media.size = Some(5_000_000);
    media.upload = Some(UploadProgress {
        sent: 2_000_000,
        total: 5_000_000,
    });
    uploading.details.media = Some(Box::new(media));
    items.push(uploading.item());
    let mut failed = say("fail0", "me", "Me", 5, "This one did not go through.");
    failed.sent = SendStatus::Failed;
    failed.details.send_failure = Some(SendFailure { recoverable: true });
    items.push(failed.item());
    let mut sending = say("echo0", "me", "Me", 0, "Sending this right now");
    sending.sent = SendStatus::Sending;
    items.push(sending.item());
    let mut last = TimelineItem::new("marker".to_owned(), TimelineItemKind::ReadMarker);
    last.read_by = vec![user("bea")];
    items.push(last);
    items
}

fn open_lounge(app: &mut App) {
    press(app, Message::SelectRoom(room("lounge")));
    feed(
        app,
        CoreEvent::TimelineDiff {
            room_id: room("lounge"),
            diffs: vec![ListDiff::Reset {
                values: timeline_items(),
            }],
        },
    );
    feed(
        app,
        CoreEvent::Pagination {
            room_id: room("lounge"),
            state: PaginationState::ReachedStart,
        },
    );
    app.timeline.scrolled(0.0, 560.0, 640.0);
}

pub fn main_room() -> App {
    let mut app = main_no_room();
    open_lounge(&mut app);
    app
}

/// A room list built to crowd its rows: long names and previews (`body`),
/// big counts, and a room with every tag there is (voice, invite, muted,
/// ringing, a mention count).
pub fn crowded_room_list(body: &str) -> App {
    let mut app = main_room();
    let mut busy = summary("lounge", &format!("Lounge {body}"));
    busy.unread_messages = 120;
    busy.unread_notifications = 120;
    busy.last_message = Some(preview("bea", "Bea Example", body, 4));
    let mut everything = summary("everything", &format!("Everything at once {body}"));
    everything.is_call_room = true;
    everything.is_invite = true;
    everything.notification_mode = Some(RoomNotificationModeView::Mute);
    everything.unread_messages = 120;
    everything.unread_notifications = 120;
    everything.highlights = 5;
    everything.unread_mentions = 5;
    everything.last_message = Some(preview("cy", "Cy Example", body, 9));
    let mut ringing = summary("hangout", &format!("Voice hangout {body}"));
    ringing.is_call_room = true;
    ringing.unread_messages = 3;
    ringing.unread_notifications = 3;
    ringing.last_message = Some(preview("dee", "Dee Example", body, 30));
    let mut counted = summary("counted", &format!("In a call {body}"));
    counted.is_call_room = true;
    counted.is_invite = true;
    counted.last_message = Some(preview("dee", "Dee Example", body, 31));
    app.rooms = vec![busy, everything, ringing, counted];
    feed(
        &mut app,
        CoreEvent::IncomingCall {
            room_id: room("hangout"),
            caller: user("dee"),
            intent: CallIntent::Audio,
            ring: true,
        },
    );
    feed(
        &mut app,
        CoreEvent::IncomingCall {
            room_id: room("everything"),
            caller: user("cy"),
            intent: CallIntent::Audio,
            ring: true,
        },
    );
    feed(
        &mut app,
        CoreEvent::ActiveCall {
            room_id: room("counted"),
            participants: 12,
        },
    );
    app
}

/// The room with only the items called `ids` (the timeline pane shows the
/// newest items; the gallery cannot scroll, so each part of the timeline gets
/// its own screen).
fn lounge_with(ids: &[&str]) -> App {
    let mut app = main_no_room();
    press(&mut app, Message::SelectRoom(room("lounge")));
    let items = timeline_items()
        .into_iter()
        .filter(|item| ids.contains(&item.id.as_str()))
        .collect();
    feed(
        &mut app,
        CoreEvent::TimelineDiff {
            room_id: room("lounge"),
            diffs: vec![ListDiff::Reset { values: items }],
        },
    );
    app.timeline.scrolled(0.0, 560.0, 640.0);
    app
}

/// Room events, a rich-text message and the placeholders.
fn main_room_states() -> App {
    lounge_with(&[
        "start",
        "s-created",
        "s-enc",
        "s-joined",
        "d1",
        "m1",
        "m2",
        "m3",
        "p-utd",
        "p-red",
        "p-poll",
        "other",
    ])
}

/// Plain, wrapped, reply, edited, reaction, mention, emote, notice and
/// shield messages.
fn main_room_text() -> App {
    lounge_with(&["d2", "m4", "m5", "m6", "m7", "m8", "m9", "m10"])
}

fn main_typing() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::Typing {
            room_id: room("lounge"),
            users: vec![
                TypingUser {
                    user_id: user("bea"),
                    name: Some("Bea Example".to_owned()),
                },
                TypingUser {
                    user_id: user("cy"),
                    name: Some("Cy Example".to_owned()),
                },
            ],
        },
    );
    app
}

fn main_reply() -> App {
    let mut app = main_typing();
    press(&mut app, Message::ReplyRequested("m4".to_owned()));
    app
}

fn main_picker() -> App {
    let mut app = main_room();
    press(&mut app, Message::PickerOpened("m6".to_owned()));
    app
}

fn main_dm() -> App {
    let mut app = main_no_room();
    press(&mut app, Message::SelectRoom(room("alice")));
    feed(
        &mut app,
        CoreEvent::UserTrust {
            user_id: user("alice"),
            trust: UserTrust::Unverified,
        },
    );
    feed(
        &mut app,
        CoreEvent::TimelineDiff {
            room_id: room("alice"),
            diffs: vec![ListDiff::Reset {
                values: timeline_items().into_iter().skip(5).take(6).collect(),
            }],
        },
    );
    app.timeline.scrolled(0.0, 560.0, 640.0);
    app
}

/// The direct message of `main-dm` with `peer` as the other person and
/// `trust` as the core's word on them.
#[allow(dead_code)] // each integration test uses its own part of the fixture
pub fn dm_with_peer(peer: &str, trust: UserTrust) -> App {
    let mut app = main_no_room();
    let id = UserId::from(peer.to_owned());
    for summary in &mut app.rooms {
        if summary.id == room("alice") {
            summary.dm_peer = Some(id.clone());
        }
    }
    press(&mut app, Message::SelectRoom(room("alice")));
    feed(&mut app, CoreEvent::UserTrust { user_id: id, trust });
    app
}

fn main_invite() -> App {
    let mut app = main_no_room();
    press(&mut app, Message::SelectRoom(room("book-club")));
    app
}

fn main_empty_room() -> App {
    let mut app = main_no_room();
    press(&mut app, Message::SelectRoom(room("open")));
    feed(
        &mut app,
        CoreEvent::TimelineDiff {
            room_id: room("open"),
            diffs: vec![ListDiff::Reset { values: Vec::new() }],
        },
    );
    app
}

// ---- Thread -------------------------------------------------------------

pub fn thread_open(app: &mut App) {
    press(
        app,
        Message::Threads(ThreadsMessage::Open(EventId::from("$thread-root"))),
    );
    feed(
        app,
        CoreEvent::ThreadDiff {
            room_id: room("lounge"),
            root: EventId::from("$thread-root"),
            diffs: vec![ListDiff::Reset {
                values: vec![
                    say(
                        "t0",
                        "alice",
                        "Alice Example",
                        45,
                        "Let us discuss the schedule in a thread.",
                    )
                    .item(),
                    say("t1", "bea", "Bea Example", 44, "Monday is bad for me.").item(),
                    say("t2", "me", "Me", 43, "Tuesday then?").item(),
                    say("t3", "bea", "Bea Example", 42, "Friday works for me").item(),
                ],
            }],
        },
    );
    if let Some(panel) = app.threads.panel.as_mut() {
        panel.timeline.scrolled(0.0, 560.0, 320.0);
        panel.draft = "Friday it is".to_owned();
    }
}

pub fn thread_close(app: &mut App) {
    press(app, Message::Threads(ThreadsMessage::Close));
}

fn main_thread() -> App {
    let mut app = main_room();
    thread_open(&mut app);
    app
}

// ---- Calls --------------------------------------------------------------

fn person(
    name: &str,
    local: bool,
    muted: bool,
    speaking: bool,
    cleaning: CleaningStatus,
) -> Participant {
    Participant {
        id: ParticipantId::from(format!("{name}-participant")),
        user_id: user(name),
        device_id: DeviceId::from("GALLERYDEV"),
        display_name: Some(format!("{} Example", capitalised(name))),
        is_local: local,
        muted,
        speaking,
        volume_percent: 100,
        cleaning,
    }
}

fn capitalised(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn call_events(app: &mut App, state: CallState) {
    feed(app, CoreEvent::CallState(state));
}

fn call_participants(app: &mut App) {
    feed(
        app,
        CoreEvent::CallParticipants {
            room_id: room("lounge"),
            diffs: vec![ListDiff::Reset {
                values: vec![
                    person("me", true, false, false, CleaningStatus::Off),
                    person("bea", false, false, true, CleaningStatus::Cleaned),
                    person("cy", false, true, false, CleaningStatus::Off),
                    person("dee", false, false, false, CleaningStatus::Starting),
                ],
            }],
        },
    );
    feed(
        app,
        CoreEvent::CallDeclined {
            room_id: room("lounge"),
            user_id: user("alice"),
        },
    );
}

fn ptt_settings(key: Option<&str>) -> AudioSettings {
    AudioSettings {
        input_mode: InputMode::PushToTalk {
            release_delay_ms: 200,
            toggle: false,
        },
        push_to_talk: twicecat_core::api::PushToTalkBindings {
            key: key.map(str::to_owned),
            ..Default::default()
        },
        ..AudioSettings::default()
    }
}

pub fn call_connected(app: &mut App) {
    call_events(
        app,
        CallState::Connected {
            room_id: room("lounge"),
            encrypted: true,
        },
    );
    call_participants(app);
}

pub fn call_left(app: &mut App) {
    call_events(app, CallState::Idle);
}

pub fn call_panel_open(app: &mut App) {
    if !app.calls.panel_open {
        press(app, Message::Call(CallMessage::TogglePanel));
    }
}

pub fn call_panel_close(app: &mut App) {
    if app.calls.panel_open {
        press(app, Message::Call(CallMessage::TogglePanel));
    }
}

fn main_call_joining() -> App {
    let mut app = main_room();
    call_events(
        &mut app,
        CallState::Joining {
            room_id: room("lounge"),
        },
    );
    app
}

fn main_call_connected() -> App {
    let mut app = main_room();
    call_connected(&mut app);
    call_panel_open(&mut app);
    app
}

fn main_call_reconnecting() -> App {
    let mut app = main_room();
    call_connected(&mut app);
    call_events(
        &mut app,
        CallState::Reconnecting {
            room_id: room("lounge"),
            attempt: 2,
        },
    );
    call_panel_open(&mut app);
    app
}

fn main_call_ptt(talking: bool) -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::AudioSettings(ptt_settings(Some("F13"))),
    );
    call_connected(&mut app);
    call_panel_open(&mut app);
    app.ptt.talking = talking;
    app
}

fn main_call_ptt_talking() -> App {
    main_call_ptt(true)
}

fn main_call_ptt_idle() -> App {
    main_call_ptt(false)
}

fn main_call_notice() -> App {
    let mut app = main_call_connected();
    call_events(
        &mut app,
        CallState::Left {
            room_id: room("lounge"),
            reason: twicecat_core::api::CallEndReason::Failed(
                "The call server is unreachable".to_owned(),
            ),
        },
    );
    app
}

fn ring_dm() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::IncomingCall {
            room_id: room("alice"),
            caller: user("alice"),
            intent: CallIntent::Audio,
            ring: true,
        },
    );
    app
}

fn ring_group() -> App {
    let mut app = main_no_room();
    feed(
        &mut app,
        CoreEvent::IncomingCall {
            room_id: room("design"),
            caller: user("cy"),
            intent: CallIntent::Audio,
            ring: true,
        },
    );
    app
}

// ---- Voice & audio -------------------------------------------------------

fn devices() -> AudioDevices {
    let device = |id: &str, name: &str, is_default: bool| AudioDevice {
        id: AudioDeviceId::from(id),
        name: name.to_owned(),
        is_default,
    };
    AudioDevices {
        inputs: vec![
            device("in-usb", "USB headset microphone", true),
            device("in-built", "Built-in microphone", false),
        ],
        outputs: vec![
            device("out-usb", "USB headset", true),
            device("out-speakers", "Speakers", false),
        ],
    }
}

fn voice_audio_with(gpu: Option<GpuEnhancement>) -> App {
    let mut app = main_call_connected();
    feed(&mut app, CoreEvent::AudioDevices(devices()));
    let mut settings = ptt_settings(Some("F13"));
    settings.input_device = Some(AudioDeviceId::from("in-usb"));
    settings.output_device = Some(AudioDeviceId::from("out-usb"));
    feed(&mut app, CoreEvent::AudioSettings(settings));
    feed(
        &mut app,
        CoreEvent::InputLevel(InputLevel {
            rms_db: -32.0,
            peak_db: -18.0,
            transmitting: true,
        }),
    );
    feed(
        &mut app,
        CoreEvent::AudioStats(AudioStats {
            echo_return_loss_enhancement_db: Some(24.0),
            residual_echo_likelihood: Some(0.05),
            echo_delay_ms: Some(42),
            processing_us: 800,
            late_wakes: 1,
            max_wake_us: 1_900,
            capture_overruns: 0,
            playback_underruns: 0,
            remote_cleaning: Some(EnhancerBackend::Cpu),
        }),
    );
    if let Some(gpu) = gpu {
        feed(&mut app, CoreEvent::GpuEnhancement(gpu));
    }
    press(&mut app, Message::Audio(AudioMessage::Open));
    app
}

fn voice_audio() -> App {
    voice_audio_with(Some(GpuEnhancement {
        available: vec![GpuBackend::TensorRt, GpuBackend::Cuda],
        running: Some(EnhancerBackend::Gpu {
            backend: GpuBackend::TensorRt,
        }),
    }))
}

fn voice_audio_no_gpu() -> App {
    voice_audio_with(Some(GpuEnhancement {
        available: Vec::new(),
        running: None,
    }))
}

fn voice_audio_looking() -> App {
    voice_audio_with(None)
}

fn voice_audio_diagnostics() -> App {
    let mut app = voice_audio();
    press(&mut app, Message::Audio(AudioMessage::Diagnostics));
    app
}

// ---- Settings, sessions and the dialogs ------------------------------------

fn settings() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::OwnProfile(twicecat_core::api::OwnProfileView {
            display_name: Some("Me Example".to_owned()),
            avatar: None,
        }),
    );
    feed(
        &mut app,
        CoreEvent::NotificationSettings(twicecat_core::api::NotificationSettingsView {
            group_default: RoomNotificationModeView::MentionsAndKeywords,
            dm_default: RoomNotificationModeView::AllMessages,
            keywords: vec!["gallery".to_owned(), "release".to_owned()],
        }),
    );
    press(&mut app, Message::Settings(SettingsMessage::Open));
    app
}

fn sessions() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::Devices(vec![
            DeviceSummary {
                device_id: DeviceId::from("GALLERYDEV"),
                display_name: Some("Twicecat on this computer".to_owned()),
                last_seen_ms: Some(now_ms()),
                current: true,
                trust: DeviceTrust::Verified,
            },
            DeviceSummary {
                device_id: DeviceId::from("PHONE1"),
                display_name: Some("Phone".to_owned()),
                last_seen_ms: Some(now_ms() - 2 * 86_400_000),
                current: false,
                trust: DeviceTrust::Unverified,
            },
            DeviceSummary {
                device_id: DeviceId::from("OLDLAPTOP"),
                display_name: None,
                last_seen_ms: None,
                current: false,
                trust: DeviceTrust::NoKeys,
            },
        ]),
    );
    press(&mut app, Message::Sessions(SessionsMessage::Open));
    app
}

fn room_settings() -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenSettings));
    feed(
        &mut app,
        CoreEvent::RoomDetails(RoomDetailsView {
            room_id: room("lounge"),
            name: "Lounge".to_owned(),
            topic: Some("Where we talk about everything".to_owned()),
            avatar: None,
            is_encrypted: false,
            alias: Some("#lounge:example.org".to_owned()),
            permissions: RoomPermissions {
                can_set_name: true,
                can_set_topic: true,
                can_set_avatar: true,
                can_enable_encryption: true,
                can_invite: true,
            },
        }),
    );
    app
}

fn members() -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenMembers));
    let member = |name: &str, role: Role, power: PowerLevelView, membership| RoomMemberView {
        user_id: user(name),
        display_name: Some(format!("{} Example", capitalised(name))),
        avatar: None,
        power,
        role,
        membership,
    };
    feed(
        &mut app,
        CoreEvent::RoomMembers {
            room_id: room("lounge"),
            members: vec![
                member(
                    "bea",
                    Role::Creator,
                    PowerLevelView::Creator,
                    MembershipView::Joined,
                ),
                member(
                    "cy",
                    Role::Admin,
                    PowerLevelView::Level(100),
                    MembershipView::Joined,
                ),
                member(
                    "alice",
                    Role::Moderator,
                    PowerLevelView::Level(50),
                    MembershipView::Joined,
                ),
                member(
                    "me",
                    Role::User,
                    PowerLevelView::Level(0),
                    MembershipView::Joined,
                ),
                member(
                    "dee",
                    Role::User,
                    PowerLevelView::Level(0),
                    MembershipView::Invited,
                ),
            ],
            truncated: false,
        },
    );
    app
}

fn create_room() -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenCreate));
    press(
        &mut app,
        Message::Rooms(RoomsMessage::CreateName("Weekend plans".into())),
    );
    press(
        &mut app,
        Message::Rooms(RoomsMessage::CreateTopic("Hikes and picnics".into())),
    );
    app
}

fn join_room() -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenJoin));
    press(
        &mut app,
        Message::Rooms(RoomsMessage::JoinInput("#open:example.org".into())),
    );
    app
}

fn start_dm() -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenDm));
    press(
        &mut app,
        Message::Rooms(RoomsMessage::DmInput("@alice:example.org".into())),
    );
    app
}

fn room_leave_confirm() -> App {
    let mut app = room_settings();
    press(&mut app, Message::Rooms(RoomsMessage::LeaveAsk));
    app
}

fn upload() -> App {
    let mut app = main_room();
    press(
        &mut app,
        Message::FilesChosen(UploadPaths(vec![
            "/gallery/photo.png".into(),
            "/gallery/notes.txt".into(),
            "/gallery/huge.bin".into(),
            "/gallery/missing.doc".into(),
        ])),
    );
    let ids: Vec<u64> = app
        .uploads
        .dialog
        .as_ref()
        .map(|dialog| dialog.entries.iter().map(|entry| entry.id).collect())
        .unwrap_or_default();
    let ready = |name: &str, size: u64, mime: &'static str, kind: MediaKind| {
        Outcome::Ready(Box::new(Prepared {
            name: name.to_owned(),
            size,
            source: Source::Path(format!("/gallery/{name}").into()),
            mime,
            kind,
            width: None,
            height: None,
            thumbnail: None,
            preview: None,
        }))
    };
    let outcomes = [
        Some(ready("photo.png", 180_000, "image/png", MediaKind::Image)),
        Some(ready("notes.txt", 2_048, "text/plain", MediaKind::File)),
        Some(Outcome::Refused(Refusal::Unreadable)),
        // The fourth line stays "reading the file".
        None,
    ];
    for (id, outcome) in ids.into_iter().zip(outcomes) {
        if let Some(outcome) = outcome {
            press(
                &mut app,
                Message::UploadPrepared {
                    entry: id,
                    outcome: Box::new(outcome),
                },
            );
        }
    }
    app
}

fn viewer() -> App {
    let mut app = main_room();
    press(&mut app, Message::MediaClicked("img0".to_owned()));
    let mut pixels = Vec::with_capacity(320 * 240 * 4);
    for y in 0..240_u32 {
        for x in 0..320_u32 {
            pixels.extend_from_slice(&[
                u8::try_from(x * 255 / 319).expect("fits"),
                u8::try_from(y * 255 / 239).expect("fits"),
                160,
                255,
            ]);
        }
    }
    let image = image::RgbaImage::from_raw(320, 240, pixels).expect("size matches");
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("encode a PNG");
    let (handle, width, height) =
        twicecat_app::media::decode_media(png.get_ref(), twicecat_app::media::decode_limits())
            .expect("the gallery picture decodes");
    press(
        &mut app,
        Message::MediaDecoded {
            room_id: room("lounge"),
            item_id: "img0".to_owned(),
            size: twicecat_core::api::MediaSize::Full,
            decoded: Some(twicecat_app::media::Decoded {
                handle,
                width,
                height,
            }),
        },
    );
    app
}

fn switcher() -> App {
    let mut app = main_room();
    press(
        &mut app,
        Message::Shortcut {
            shortcut: twicecat_app::keys::Shortcut::Switcher,
            captured: false,
        },
    );
    press(
        &mut app,
        Message::Switcher(SwitcherMessage::Input("e".to_owned())),
    );
    app
}

// ---- Login, waiting, loading ---------------------------------------------

fn login_error() -> App {
    let mut app = logged_out();
    press(&mut app, Message::UsernameChanged("me".to_owned()));
    press(&mut app, Message::LoginSubmitted);
    feed(&mut app, CoreEvent::LoginFailed(CoreError::NotAllowed));
    app
}

fn waiting() -> App {
    let mut app = logged_out();
    press(
        &mut app,
        Message::ServerChanged("https://matrix.example.org".to_owned()),
    );
    press(&mut app, Message::OAuthRequested);
    feed(
        &mut app,
        CoreEvent::OAuthUrl(twicecat_core::api::AuthUrl::from(
            "https://auth.example.org/authorize?state=gallery",
        )),
    );
    press(&mut app, Message::BrowserOpened { ok: true });
    app
}

// ---- Verification ----------------------------------------------------------

fn verification(phase: VerificationPhase) -> App {
    let mut app = main_room();
    let id = VerificationId::from("verification-1");
    feed(
        &mut app,
        CoreEvent::VerificationRequested {
            id: id.clone(),
            peer: VerificationPeer {
                user_id: user("alice"),
                device_id: Some(DeviceId::from("PHONE1")),
                device_name: Some("Alice's phone".to_owned()),
                own_account: false,
            },
        },
    );
    feed(&mut app, CoreEvent::Verification { id, phase });
    app
}

fn emojis() -> Box<[SasEmoji; 7]> {
    let one = |symbol: &str, description: &str| SasEmoji {
        symbol: symbol.to_owned(),
        description: description.to_owned(),
    };
    Box::new([
        one("\u{1F436}", "Dog"),
        one("\u{1F431}", "Cat"),
        one("\u{1F981}", "Lion"),
        one("\u{1F434}", "Horse"),
        one("\u{1F984}", "Unicorn"),
        one("\u{1F437}", "Pig"),
        one("\u{1F418}", "Elephant"),
    ])
}

fn qr() -> QrImage {
    let size: u16 = 25;
    let dark = (0..size * size)
        .map(|i| {
            let (x, y) = (i % size, i / size);
            let finder = |a: u16, b: u16| x >= a && x < a + 7 && y >= b && y < b + 7;
            finder(0, 0) || finder(size - 7, 0) || finder(0, size - 7) || (x * 7 + y * 3) % 5 < 2
        })
        .collect();
    QrImage { size, dark }
}

fn verification_requested() -> App {
    verification(VerificationPhase::Requested { incoming: true })
}
fn verification_ready() -> App {
    verification(VerificationPhase::Ready)
}
fn verification_waiting() -> App {
    verification(VerificationPhase::Waiting)
}
fn verification_qr() -> App {
    verification(VerificationPhase::ShowQr(qr()))
}
fn verification_scanned() -> App {
    verification(VerificationPhase::QrScanned)
}
fn verification_emojis() -> App {
    verification(VerificationPhase::Emojis(emojis()))
}
fn verification_decimals() -> App {
    verification(VerificationPhase::Decimals([1234, 5678, 9012]))
}
fn verification_confirmed() -> App {
    verification(VerificationPhase::Confirmed)
}
fn verification_done() -> App {
    verification(VerificationPhase::Done)
}
fn verification_cancelled() -> App {
    verification(VerificationPhase::Cancelled {
        reason: CancelReason::Mismatch,
    })
}

// ---- Recovery and banners --------------------------------------------------

const DUMMY_KEY: &str = "GALL ERYO NLYX NOTA REAL KEYX 0000 1111 2222 3333 4444 5555";

fn status(device: DeviceVerification, recovery: RecoveryStatus) -> SecurityStatus {
    SecurityStatus {
        recovery,
        device,
        cross_signing: CrossSigning::Complete,
        has_devices_to_verify_against: true,
        identity_exists: true,
    }
}

pub fn banner_on(app: &mut App) {
    app.security.dismissed_recovery_banner = false;
    feed(
        app,
        CoreEvent::Security(status(
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        )),
    );
}

pub fn banner_off(app: &mut App) {
    feed(
        app,
        CoreEvent::Security(status(
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        )),
    );
}

fn banner_device() -> App {
    let mut app = main_room();
    banner_on(&mut app);
    app
}

fn banner_recovery_off() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::Security(status(
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        )),
    );
    app
}

fn banner_recovery_incomplete() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::Security(status(
            DeviceVerification::Verified,
            RecoveryStatus::Incomplete,
        )),
    );
    app
}

fn banner_notice() -> App {
    let mut app = main_room();
    app.security.notice = Some("Recovery is set up. Keep your key somewhere safe.".to_owned());
    app
}

fn recovery_working() -> App {
    let mut app = banner_recovery_off();
    app.security.progress = Some(RecoveryProgress::BackingUp {
        done: 30,
        total: 120,
    });
    app.security.busy = Some(twicecat_app::security::Busy::SettingUp);
    app
}

fn recovery_key() -> App {
    let mut app = banner_recovery_off();
    app.security.setup = Some(RecoverySetup::new(RecoveryKey::from(DUMMY_KEY), false));
    app
}

fn recovery_restore() -> App {
    let mut app = banner_recovery_incomplete();
    app.security.overlay = Some(Overlay::Restore);
    app
}

fn recovery_confirm_identity() -> App {
    let mut app = banner_device();
    app.security.overlay = Some(Overlay::ConfirmIdentity);
    app.security.fresh_login = true;
    app
}

fn recovery_reset_confirm() -> App {
    let mut app = banner_device();
    app.security.overlay = Some(Overlay::ResetConfirm);
    app
}

fn recovery_reauth() -> App {
    let mut app = banner_device();
    app.security.reauth = Some(ReauthPrompt::new(ReauthAction::ResetIdentity));
    app
}

// ---- The list ---------------------------------------------------------------

fn s(
    name: &'static str,
    room_list: bool,
    build: fn() -> App,
    reached: fn(&App) -> bool,
) -> Scenario {
    Scenario {
        name,
        room_list,
        build,
        reached,
    }
}

fn in_main(app: &App) -> bool {
    app.screen == Screen::Main
}

fn call_phase(app: &App) -> Option<twicecat_app::calls::CallPhase> {
    app.calls.call.as_ref().map(|call| call.phase)
}

fn phase_is_connected(app: &App) -> bool {
    matches!(
        call_phase(app),
        Some(twicecat_app::calls::CallPhase::Connected { .. })
    )
}

fn verification_phase_is(app: &App, wanted: fn(&Phase) -> bool) -> bool {
    app.verification
        .as_ref()
        .is_some_and(|model| wanted(&model.phase))
}

/// Every screen of the gallery, in the order of the plan.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        s("login", false, logged_out, |a| a.screen == Screen::Login),
        s("login-error", false, login_error, |a| {
            a.screen == Screen::Login && a.notice.is_some()
        }),
        s("waiting", false, waiting, |a| {
            a.screen == Screen::WaitingForBrowser
        }),
        s("loading", false, logged_in_loading, |a| {
            a.screen == Screen::Loading
        }),
        s("main-no-room", true, main_no_room, |a| {
            in_main(a) && a.selected.is_none() && a.rooms.len() > 10
        }),
        s("main-empty-room", true, main_empty_room, |a| {
            in_main(a) && a.selected.is_some() && a.timeline.items.is_empty()
        }),
        s("main-room", true, main_room, |a| {
            in_main(a) && a.selected.is_some() && a.timeline.items.len() > 25
        }),
        s("main-room-states", true, main_room_states, |a| {
            in_main(a) && a.timeline.items.len() == 12
        }),
        s("main-room-text", true, main_room_text, |a| {
            in_main(a) && a.timeline.items.len() == 8
        }),
        s("main-typing", true, main_typing, |a| !a.typing.is_empty()),
        s("main-reply", true, main_reply, |a| {
            matches!(a.composer.mode, twicecat_app::composer::Mode::Reply(_))
        }),
        s("main-picker", true, main_picker, |a| {
            a.actions.picker.is_some()
        }),
        s("main-dm", true, main_dm, |a| a.dm_peer_trust().is_some()),
        s("main-invite", true, main_invite, |a| {
            a.rooms
                .iter()
                .any(|r| r.is_invite && Some(&r.id) == a.selected.as_ref())
        }),
        s("main-thread", true, main_thread, |a| {
            a.threads.panel.is_some()
        }),
        s("main-call-joining", true, main_call_joining, |a| {
            call_phase(a) == Some(twicecat_app::calls::CallPhase::Joining)
        }),
        s("main-call-connected", true, main_call_connected, |a| {
            phase_is_connected(a) && a.calls.panel_open && a.calls.rows().len() >= 4
        }),
        s(
            "main-call-reconnecting",
            true,
            main_call_reconnecting,
            |a| {
                matches!(
                    call_phase(a),
                    Some(twicecat_app::calls::CallPhase::Reconnecting { .. })
                )
            },
        ),
        s("main-call-ptt-talking", true, main_call_ptt_talking, |a| {
            a.ptt.talking && phase_is_connected(a)
        }),
        s("main-call-ptt-idle", true, main_call_ptt_idle, |a| {
            !a.ptt.talking && phase_is_connected(a)
        }),
        s("main-call-notice", true, main_call_notice, |a| {
            a.calls.notice.is_some()
        }),
        s("ring-dm", true, ring_dm, |a| !a.calls.rings.is_empty()),
        s("ring-group", true, ring_group, |a| {
            !a.calls.rings.is_empty()
        }),
        s("voice-audio", false, voice_audio, |a| {
            a.audio_ui.open
                && a.calls
                    .gpu
                    .as_ref()
                    .is_some_and(|g| !g.available.is_empty())
        }),
        s("voice-audio-no-gpu", false, voice_audio_no_gpu, |a| {
            a.audio_ui.open && a.calls.gpu.as_ref().is_some_and(|g| g.available.is_empty())
        }),
        s("voice-audio-looking", false, voice_audio_looking, |a| {
            a.audio_ui.open && a.calls.gpu.is_none()
        }),
        s(
            "voice-audio-diagnostics",
            false,
            voice_audio_diagnostics,
            |a| a.audio_ui.open && a.audio_ui.diagnostics,
        ),
        s("settings", false, settings, |a| a.settings_ui.open),
        s("sessions", false, sessions, |a| a.sessions.open),
        s("room-settings", false, room_settings, |a| {
            a.rooms_ui.settings.is_some()
        }),
        s("room-leave-confirm", false, room_leave_confirm, |a| {
            a.rooms_ui.settings.is_some()
        }),
        s("members", false, members, |a| {
            a.rooms_ui.members_panel.is_some()
        }),
        s("create-room", false, create_room, |a| {
            a.rooms_ui.dialog.is_some()
        }),
        s("join-room", false, join_room, |a| {
            a.rooms_ui.dialog.is_some()
        }),
        s("start-dm", false, start_dm, |a| a.rooms_ui.dialog.is_some()),
        s("upload", false, upload, |a| {
            a.uploads
                .dialog
                .as_ref()
                .is_some_and(|d| d.sendable() == 2 && d.loading())
        }),
        s("viewer", false, viewer, |a| {
            a.media
                .viewer
                .as_ref()
                .is_some_and(|v| v.decoded().is_some())
        }),
        s("switcher", false, switcher, |a| a.switcher.open),
        s(
            "verification-requested",
            false,
            verification_requested,
            |a| verification_phase_is(a, |p| matches!(p, Phase::Requested { .. })),
        ),
        s("verification-ready", false, verification_ready, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::Ready))
        }),
        s("verification-waiting", false, verification_waiting, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::Waiting))
        }),
        s("verification-qr", false, verification_qr, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::ShowQr))
        }),
        s("verification-scanned", false, verification_scanned, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::QrScanned))
        }),
        s("verification-emojis", false, verification_emojis, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::Emojis(_)))
        }),
        s("verification-decimals", false, verification_decimals, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::Decimals(_)))
        }),
        s(
            "verification-confirmed",
            false,
            verification_confirmed,
            |a| verification_phase_is(a, |p| matches!(p, Phase::Confirmed)),
        ),
        s("verification-done", false, verification_done, |a| {
            verification_phase_is(a, |p| matches!(p, Phase::Done))
        }),
        s(
            "verification-cancelled",
            false,
            verification_cancelled,
            |a| verification_phase_is(a, |p| matches!(p, Phase::Cancelled(_))),
        ),
        s("banner-device", true, banner_device, |a| {
            a.security.banner() == Some(twicecat_app::security::Banner::DeviceUnverified)
        }),
        s("banner-recovery-off", true, banner_recovery_off, |a| {
            a.security.banner() == Some(twicecat_app::security::Banner::RecoveryDisabled)
        }),
        s(
            "banner-recovery-incomplete",
            true,
            banner_recovery_incomplete,
            |a| a.security.banner() == Some(twicecat_app::security::Banner::RecoveryIncomplete),
        ),
        s("banner-notice", true, banner_notice, |a| {
            a.security.notice.is_some()
        }),
        s("recovery-working", false, recovery_working, |a| {
            a.security.progress.is_some()
        }),
        s("recovery-key", false, recovery_key, |a| {
            a.security.setup.is_some()
        }),
        s("recovery-restore", false, recovery_restore, |a| {
            a.security.overlay == Some(Overlay::Restore)
        }),
        s(
            "recovery-confirm-identity",
            false,
            recovery_confirm_identity,
            |a| a.security.overlay == Some(Overlay::ConfirmIdentity),
        ),
        s(
            "recovery-reset-confirm",
            false,
            recovery_reset_confirm,
            |a| a.security.overlay == Some(Overlay::ResetConfirm),
        ),
        s("recovery-reauth", false, recovery_reauth, |a| {
            a.security.reauth.is_some()
        }),
    ]
}
