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
use twicecat_app::audio_settings::{AudioMessage, AudioPage};
use twicecat_app::calls::CallMessage;
use twicecat_app::rooms::RoomsMessage;
use twicecat_app::security::{Overlay, ReauthPrompt, RecoverySetup};
use twicecat_app::sessions::SessionsMessage;
use twicecat_app::settings::{SettingsMessage, SettingsPage};
use twicecat_app::switcher::SwitcherMessage;
use twicecat_app::threads::ThreadsMessage;
use twicecat_app::upload::{Outcome, Prepared, Refusal, Source, UploadPaths};
use twicecat_app::verification::Phase;
use twicecat_app::video::VideoMessage;
use twicecat_app::{App, Message, Screen, Startup};
use twicecat_app::{CommandSink, EventsSlot};
use twicecat_core::api::{
    AudioDevice, AudioDeviceId, AudioDevices, AudioSettings, AudioStats, Block, CallIntent,
    CallState, CancelReason, CleaningStatus, CodecLimits, Command, CoreError, CoreEvent,
    CrossSigning, DegradationChoice, DeviceId, DeviceSummary, DeviceTrust, DeviceVerification,
    EncoderBackend, EnhancerBackend, EventId, GpuBackend, GpuEnhancement, Inline, InputLevel,
    InputMode, ItemActions, LayerTuning, ListDiff, LocalVideo, MediaContent, MediaKind,
    MembershipView, MentionTarget, MessageDetails, MessageKind, MessagePreview, NoiseSuppression,
    PaginationState, Participant, ParticipantId, PlaceholderKind, PowerLevelView, QrImage,
    QualityLimitation, ReactionGroup, ReauthAction, RecoveryKey, RecoveryProgress, RecoveryStatus,
    ReplyPreview, ReplyState, RichText, Role, RoomDetailsView, RoomId, RoomMemberView,
    RoomNotificationModeView, RoomPermissions, RoomSummary, SasEmoji, ScreenSource, ScreenSourceId,
    ScreenSourceKind, ScreenSources, ScreenThumbnail, SecurityStatus, SendFailure, SendStatus,
    Shield, ShieldCode, SpaceView, StateChange, SyncState, TextStyle, ThreadSummaryView,
    TimelineItem, TimelineItemKind, TypingUser, UpdateCheckChoice, UploadProgress, UserId,
    UserTrust, UtdReason, VerificationId, VerificationPeer, VerificationPhase, VideoCodec,
    VideoDevice, VideoDeviceId, VideoDevices, VideoDirection, VideoEncoderInfo, VideoEncoders,
    VideoFeed, VideoFrame, VideoKind, VideoMatrix, VideoMode, VideoPixelFormat, VideoPlane,
    VideoRange, VideoSettings, VideoStats, VideoTrackId, VideoTrackStats, VideoTrackView,
    VideoTuning,
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
    let (mut app, _boot) = App::new(Startup {
        sink: Box::new(NullSink),
        events: EventsSlot::new(rx),
    });
    // The update question was answered, so the screens show without its
    // bar; `update_question` and `update_available` show the bars.
    app.ui_settings.update_check = UpdateCheckChoice::Off;
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

/// A reply being written, with the emoji suggestions open under `:sm`.
fn main_reply_suggestions() -> App {
    use iced::widget::text_editor::{Action, Edit};
    let mut app = main_reply();
    for c in ":sm".chars() {
        press(
            &mut app,
            Message::Composer(twicecat_app::composer::EditorAction(Action::Edit(
                Edit::Insert(c),
            ))),
        );
    }
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
        share_volume_percent: 100,
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

// ---- Video in calls -----------------------------------------------------

/// A small NV12 gradient: the gallery cannot draw it (the headless renderer
/// has no GPU), but a track with a frame is a track with a picture.
fn video_feed() -> VideoFeed {
    let (tx, feed) = VideoFeed::channel();
    let (w, h) = (64usize, 36usize);
    let luma: Vec<u8> = (0..w * h).map(|i| (16 + (i % w) * 3) as u8).collect();
    tx.send(VideoFrame {
        format: VideoPixelFormat::Nv12,
        width: w as u32,
        height: h as u32,
        planes: vec![
            VideoPlane {
                data: luma.into(),
                stride: w,
            },
            VideoPlane {
                data: vec![128; w * h / 2].into(),
                stride: w,
            },
        ],
        matrix: VideoMatrix::Bt709,
        range: VideoRange::Limited,
        timestamp_us: 0,
    });
    // The sender goes with the feed's last clone: the track stays live.
    std::mem::forget(tx);
    feed
}

fn video_track(who: &str, kind: VideoKind, local: bool, muted: bool) -> VideoTrackView {
    VideoTrackView {
        id: VideoTrackId::from(format!("{who}-{kind:?}")),
        participant: ParticipantId::from(format!("{who}-participant")),
        user_id: user(who),
        device_id: DeviceId::from("GALLERYDEV"),
        is_local: local,
        kind,
        width: 1280,
        height: 720,
        fps: Some(30),
        codec: Some(VideoCodec::H264),
        muted,
    }
}

/// The lounge call with these tracks, each with a picture.
fn call_with_video(tracks: Vec<VideoTrackView>) -> App {
    let mut app = main_room();
    call_connected(&mut app);
    feed(
        &mut app,
        CoreEvent::VideoTracks {
            room_id: room("lounge"),
            tracks: tracks.clone(),
        },
    );
    for track in tracks {
        feed(
            &mut app,
            CoreEvent::VideoFeed {
                track: track.id,
                feed: video_feed(),
            },
        );
    }
    app
}

fn main_call_video() -> App {
    call_with_video(vec![
        video_track("me", VideoKind::Camera, true, false),
        video_track("bea", VideoKind::Camera, false, false),
        video_track("cy", VideoKind::Camera, false, true),
    ])
}

fn main_call_video_share() -> App {
    call_with_video(vec![
        video_track("me", VideoKind::Camera, true, false),
        video_track("bea", VideoKind::Camera, false, false),
        video_track("dee", VideoKind::Screen, false, false),
    ])
}

fn main_call_video_folded() -> App {
    let mut app = main_call_video();
    press(&mut app, Message::Video(VideoMessage::ToggleStage));
    app
}

fn main_call_sharing() -> App {
    let mut app = main_call_video();
    feed(
        &mut app,
        CoreEvent::LocalVideo(LocalVideo {
            camera: Some(VideoTrackId::from("me-Camera")),
            screen: Some(VideoTrackId::from("me-Screen")),
            screen_audio: true,
        }),
    );
    app
}

fn video_fullscreen() -> App {
    let mut app = main_call_video_share();
    press(
        &mut app,
        Message::Video(VideoMessage::Fullscreen(VideoTrackId::from("dee-Screen"))),
    );
    app
}

fn share_picker() -> App {
    let mut app = main_call_video();
    press(&mut app, Message::Video(VideoMessage::ToggleShare));
    // About the size the picker shows them at: a tiny picture scaled up a lot
    // is drawn out of place by the software renderer.
    let thumb = |shade: u8| {
        let (w, h) = (160usize, 90usize);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let edge = i % w < 3 || i % w + 3 >= w || i / w < 3 || i / w + 3 >= h;
                if edge {
                    [20, 20, 20, 255]
                } else {
                    [shade, (i % w) as u8, 255 - shade, 255]
                }
            })
            .collect();
        ScreenThumbnail {
            width: w as u32,
            height: h as u32,
            rgba: rgba.into(),
        }
    };
    let source = |id: &str, kind, title: &str, w, h, shade| ScreenSource {
        id: ScreenSourceId::from(id),
        kind,
        title: title.to_owned(),
        width: w,
        height: h,
        thumbnail: Some(thumb(shade)),
    };
    feed(
        &mut app,
        CoreEvent::ScreenSources(ScreenSources {
            system_picker: false,
            sources: vec![
                source("m1", ScreenSourceKind::Monitor, "Screen 1", 2560, 1440, 60),
                source("m2", ScreenSourceKind::Monitor, "Screen 2", 1920, 1080, 120),
                source("w1", ScreenSourceKind::Window, "Notes", 1200, 800, 180),
                source(
                    "w2",
                    ScreenSourceKind::Window,
                    "A very long window title that must not run out of its card",
                    900,
                    700,
                    220,
                ),
            ],
        }),
    );
    press(
        &mut app,
        Message::Video(VideoMessage::PickSource(ScreenSourceId::from("m2"))),
    );
    app
}

fn share_picker_system() -> App {
    let mut app = main_call_video();
    press(&mut app, Message::Video(VideoMessage::ToggleShare));
    feed(
        &mut app,
        CoreEvent::ScreenSources(ScreenSources {
            system_picker: true,
            sources: Vec::new(),
        }),
    );
    app
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

fn main_call_unencrypted() -> App {
    let mut app = main_room();
    call_events(
        &mut app,
        CallState::Connected {
            room_id: room("lounge"),
            encrypted: false,
        },
    );
    call_participants(&mut app);
    call_panel_open(&mut app);
    app
}

fn main_call_ptt_muted() -> App {
    let mut app = main_call_ptt(true);
    press(&mut app, Message::Call(CallMessage::ToggleMute));
    app
}

/// The call panel with two people's controls folded out (volume, cleaning).
fn main_call_rows_open() -> App {
    let mut app = main_call_connected();
    press(&mut app, Message::CallRowMore(user("bea")));
    press(&mut app, Message::CallRowMore(user("cy")));
    app
}

/// A caller whose user ID is one long word: no hyphen, space or dot to break
/// a line at, so only a glyph break keeps its end inside the card.
pub const LONG_CALLER: &str = "@aVeryLongCallerNameThatGoesOnAndOnWithoutAnyBreakAtAllNotEvenOneSingleOneHere:farAwayExampleOrg";

fn ring_long_id() -> App {
    let mut app = main_no_room();
    feed(
        &mut app,
        CoreEvent::IncomingCall {
            room_id: room("design"),
            caller: UserId::from(LONG_CALLER),
            intent: CallIntent::Audio,
            ring: true,
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

/// The Voice & audio card on `page`, in a call (the call model has the
/// numbers) or not, with the probe's answer `gpu`.
fn voice_page_with(
    page: AudioPage,
    in_call: bool,
    gpu: Option<GpuEnhancement>,
    tweak: impl FnOnce(&mut AudioSettings, &mut AudioDevices),
) -> App {
    let mut app = if in_call {
        main_call_connected()
    } else {
        main_room()
    };
    let mut devices = devices();
    let mut settings = ptt_settings(Some("F13"));
    settings.input_device = Some(AudioDeviceId::from("in-usb"));
    settings.output_device = Some(AudioDeviceId::from("out-usb"));
    tweak(&mut settings, &mut devices);
    feed(&mut app, CoreEvent::AudioDevices(devices));
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
    advanced(&mut app, true);
    press(&mut app, Message::Audio(AudioMessage::OpenAt(page)));
    app
}

/// The Settings switch **Show advanced settings** on or off.
fn advanced(app: &mut App, on: bool) {
    let mut settings = app.ui_settings;
    settings.advanced_settings = on;
    app.set_ui_settings(settings);
}

fn gpu_found() -> Option<GpuEnhancement> {
    Some(GpuEnhancement {
        available: vec![GpuBackend::TensorRt, GpuBackend::Cuda],
        running: Some(EnhancerBackend::Gpu {
            backend: GpuBackend::TensorRt,
        }),
    })
}

fn voice_page(page: AudioPage) -> App {
    voice_page_with(page, true, gpu_found(), |_, _| {})
}

fn voice_microphone(gpu: Option<GpuEnhancement>) -> App {
    voice_page_with(AudioPage::Microphone, true, gpu, |settings, _| {
        settings.processing.noise_suppression = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 60.0,
            gpu: true,
        };
    })
}

fn voice_gpu_none() -> App {
    voice_microphone(Some(GpuEnhancement {
        available: Vec::new(),
        running: None,
    }))
}

fn voice_gpu_fallback() -> App {
    voice_microphone(Some(GpuEnhancement {
        available: vec![GpuBackend::TensorRt, GpuBackend::Cuda],
        running: Some(EnhancerBackend::CpuAfterGpuTooSlow),
    }))
}

fn voice_talking_capturing() -> App {
    let mut app = voice_page(AudioPage::Talking);
    press(&mut app, Message::Audio(AudioMessage::CaptureKey));
    app
}

fn voice_talking_voice_activity() -> App {
    voice_page_with(AudioPage::Talking, true, gpu_found(), |settings, _| {
        settings.input_mode = InputMode::VoiceActivity {
            automatic: false,
            threshold_db: -40.0,
        };
    })
}

fn voice_mic_test() -> App {
    let mut app = voice_page_with(AudioPage::MicTest, false, gpu_found(), |_, _| {});
    press(&mut app, Message::Audio(AudioMessage::MicTest));
    app
}

fn voice_levels_loud() -> App {
    let mut app = voice_page(AudioPage::Levels);
    feed(
        &mut app,
        CoreEvent::InputLevel(InputLevel {
            rms_db: -2.0,
            peak_db: -0.5,
            transmitting: true,
        }),
    );
    app
}

fn voice_others() -> App {
    voice_page_with(AudioPage::OtherVoices, true, gpu_found(), |settings, _| {
        settings.remote_cleaning.enabled = true;
    })
}

fn voice_ringtone_sounds_off() -> App {
    voice_page_with(AudioPage::Ringtone, true, gpu_found(), |settings, _| {
        settings.call_sounds.enabled = false;
    })
}

fn voice_diagnostics() -> App {
    let mut app = voice_page(AudioPage::Diagnostics);
    press(&mut app, Message::Audio(AudioMessage::Diagnostics));
    app
}

fn voice_diagnostics_dropouts() -> App {
    let mut app = voice_diagnostics();
    feed(
        &mut app,
        CoreEvent::AudioStats(AudioStats {
            capture_overruns: 3,
            playback_underruns: 5,
            ..AudioStats::default()
        }),
    );
    app
}

/// Device names and a key as long as the core lets a stranger make them.
fn voice_long_names(page: AudioPage) -> App {
    voice_page_with(page, true, gpu_found(), |settings, devices| {
        devices.inputs[0].name = "M".repeat(300);
        devices.outputs[0].name = format!("{} {}", "Speakers".repeat(40), "END");
        settings.push_to_talk.key = Some("W".repeat(300));
    })
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

/// Settings on `page`.
fn settings_page(page: SettingsPage) -> App {
    let mut app = settings();
    press(&mut app, Message::Settings(SettingsMessage::Page(page)));
    app
}

fn settings_appearance() -> App {
    settings_page(SettingsPage::Appearance)
}

fn settings_notifications() -> App {
    settings_page(SettingsPage::Notifications)
}

/// The themes folder the tests share: Daylight, the Duo pair, Faint and Paper.
fn theme_fixtures() -> twicecat_app::themes::Loaded {
    twicecat_app::themes::load_dir(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/themes"
    )))
}

/// A report line for a file that was refused.
fn refused_line(name: &str) -> twicecat_app::themes::ReportLine {
    use twicecat_app::theme::Refusal;
    use twicecat_app::themes::{FileProblem, Note, ReportLine};
    ReportLine {
        name: Some(name.to_owned()),
        note: Note::Problem(FileProblem::Refused(Refusal::MixedBackgrounds)),
    }
}

/// The Appearance page with the fixture themes read and `selected` stored.
fn themes_page(selected: Option<&str>, mode: iced::theme::Mode) -> App {
    let mut app = settings_appearance();
    let mut settings = app.ui_settings;
    settings.theme_name =
        selected.map(|n| twicecat_core::api::ThemeName::try_from(n).expect("a valid name"));
    app.set_ui_settings(settings);
    press(&mut app, Message::ThemeMode(mode));
    let mut loaded = theme_fixtures();
    loaded.report.push(refused_line("Mixed"));
    app.set_themes(loaded);
    app
}

/// Daylight selected; the menu lists the four themes; the report has an
/// adjusted file and a refused one.
fn settings_appearance_themes() -> App {
    themes_page(Some("Daylight"), iced::theme::Mode::Dark)
}

/// Duo selected with the stored choice System, the system in light mode.
fn settings_appearance_pair() -> App {
    themes_page(Some("Duo"), iced::theme::Mode::Light)
}

/// The same pair, the system in dark mode.
fn settings_appearance_pair_dark() -> App {
    themes_page(Some("Duo"), iced::theme::Mode::Dark)
}

/// The selected theme was refused: Twicecat is shown, with the reason.
fn settings_appearance_fallback() -> App {
    themes_page(Some("Mixed"), iced::theme::Mode::Dark)
}

/// A read of the themes folder is running.
fn settings_appearance_loading() -> App {
    let mut app = themes_page(Some("Daylight"), iced::theme::Mode::Dark);
    press(&mut app, Message::Settings(SettingsMessage::ReloadThemes));
    app
}

/// Keywords that are long and have no break point, and a save that failed.
fn settings_notifications_long() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::NotificationSettings(twicecat_core::api::NotificationSettingsView {
            group_default: RoomNotificationModeView::Mute,
            dm_default: RoomNotificationModeView::AllMessages,
            keywords: vec!["k".repeat(64), "release".to_owned()],
        }),
    );
    press(&mut app, Message::Settings(SettingsMessage::Open));
    press(
        &mut app,
        Message::Settings(SettingsMessage::Page(SettingsPage::Notifications)),
    );
    press(&mut app, Message::Settings(SettingsMessage::Saved(false)));
    app
}

/// The Voice & audio page: the stored settings and the devices echoed, so
/// the basics are shown rather than a line saying they are loading.
fn settings_voice_audio() -> App {
    let mut app = settings();
    feed(&mut app, CoreEvent::AudioDevices(devices()));
    let mut stored = ptt_settings(Some("F13"));
    stored.input_device = Some(AudioDeviceId::from("in-usb"));
    stored.output_device = Some(AudioDeviceId::from("out-usb"));
    feed(&mut app, CoreEvent::AudioSettings(stored));
    press(
        &mut app,
        Message::Settings(SettingsMessage::Page(SettingsPage::VoiceAudio)),
    );
    app
}

/// A page, with the Settings switch on.
fn settings_advanced(build: fn() -> App) -> App {
    let mut app = build();
    advanced(&mut app, true);
    app
}

fn cameras() -> VideoDevices {
    let mode = |width, height, fps| VideoMode { width, height, fps };
    VideoDevices {
        cameras: vec![
            VideoDevice {
                id: VideoDeviceId::from("cam-usb"),
                name: "USB webcam".to_owned(),
                is_default: true,
                modes: vec![
                    mode(1920, 1080, 30),
                    mode(1280, 720, 30),
                    mode(640, 480, 30),
                ],
            },
            VideoDevice {
                id: VideoDeviceId::from("cam-built-in"),
                name: "Integrated camera".to_owned(),
                is_default: false,
                modes: vec![mode(1280, 720, 30)],
            },
        ],
    }
}

fn encoders() -> VideoEncoders {
    let limits = |codec, max_height, tested| CodecLimits {
        codec,
        max_width: max_height * 16 / 9,
        max_height,
        max_fps: 60,
        tested,
    };
    VideoEncoders {
        encoders: vec![
            VideoEncoderInfo {
                backend: EncoderBackend::Nvenc,
                gpu: Some("NVIDIA GeForce RTX 4070".to_owned()),
                hardware: true,
                codecs: vec![
                    limits(VideoCodec::H264, 2160, true),
                    limits(VideoCodec::Av1, 2160, false),
                ],
            },
            VideoEncoderInfo {
                backend: EncoderBackend::Software,
                gpu: None,
                hardware: false,
                codecs: vec![limits(VideoCodec::Vp8, 1080, true)],
            },
        ],
        in_use: None,
    }
}

/// The Video page of Settings, with a camera list and the encoders found.
fn settings_video() -> App {
    let mut app = settings();
    feed(&mut app, CoreEvent::VideoDevices(cameras()));
    feed(&mut app, CoreEvent::VideoSettings(VideoSettings::default()));
    feed(&mut app, CoreEvent::VideoEncoders(encoders()));
    press(
        &mut app,
        Message::Settings(SettingsMessage::Page(SettingsPage::Video)),
    );
    app
}

/// The same with the advanced switch on and tuned values in both kinds.
fn settings_video_advanced() -> App {
    let mut app = settings_video();
    advanced(&mut app, true);
    feed(
        &mut app,
        CoreEvent::VideoSettings(VideoSettings {
            upload_cap_kbps: Some(4000),
            camera_tuning: VideoTuning {
                width: Some(1280),
                height: Some(720),
                fps: Some(30),
                bitrate_kbps: Some(1700),
                layers: Some(3),
                lower_layers: vec![LayerTuning {
                    bitrate_kbps: Some(600),
                    ..LayerTuning::default()
                }],
                degradation: Some(DegradationChoice::Balanced),
                ..VideoTuning::default()
            },
            screen_tuning: VideoTuning {
                codec: Some(VideoCodec::Vp9),
                scalability_mode: Some("L3T3_KEY".to_owned()),
                ..VideoTuning::default()
            },
            ..VideoSettings::default()
        }),
    );
    app
}

/// In a call with video, the advanced page also shows what is being sent.
fn settings_video_advanced_in_call() -> App {
    let mut app = main_call_video();
    advanced(&mut app, true);
    feed(&mut app, CoreEvent::VideoDevices(cameras()));
    feed(&mut app, CoreEvent::VideoSettings(VideoSettings::default()));
    let mut found = encoders();
    found.in_use = Some(EncoderBackend::Nvenc);
    feed(&mut app, CoreEvent::VideoEncoders(found));
    feed(
        &mut app,
        CoreEvent::VideoStats(VideoStats {
            tracks: vec![
                VideoTrackStats {
                    track: VideoTrackId::from("me-Camera"),
                    direction: VideoDirection::Sent,
                    width: 1280,
                    height: 720,
                    fps: 29.6,
                    bitrate_kbps: 1650,
                    frame_ms: Some(2.4),
                    limitation: QualityLimitation::Bandwidth,
                    freezes: 0,
                    implementation: Some("NVIDIA H264 Encoder".to_owned()),
                    hardware: true,
                },
                VideoTrackStats {
                    track: VideoTrackId::from("bea-Camera"),
                    direction: VideoDirection::Received,
                    width: 640,
                    height: 360,
                    fps: 24.0,
                    bitrate_kbps: 620,
                    frame_ms: Some(1.1),
                    limitation: QualityLimitation::None,
                    freezes: 2,
                    implementation: Some("libvpx".to_owned()),
                    hardware: false,
                },
            ],
        }),
    );
    press(&mut app, Message::Settings(SettingsMessage::Open));
    press(
        &mut app,
        Message::Settings(SettingsMessage::Page(SettingsPage::Video)),
    );
    app
}

fn settings_keyboard() -> App {
    settings_page(SettingsPage::Keyboard)
}

fn settings_sessions() -> App {
    settings_page(SettingsPage::Sessions)
}

fn settings_about() -> App {
    settings_page(SettingsPage::About)
}

fn device(id: &str, name: Option<&str>, current: bool, trust: DeviceTrust) -> DeviceSummary {
    DeviceSummary {
        device_id: DeviceId::from(id),
        display_name: name.map(str::to_owned),
        last_seen_ms: if current {
            Some(now_ms())
        } else {
            Some(now_ms() - 2 * 86_400_000)
        },
        current,
        trust,
    }
}

fn sessions_with(devices: Vec<DeviceSummary>) -> App {
    let mut app = main_room();
    feed(&mut app, CoreEvent::Devices(devices));
    press(&mut app, Message::Sessions(SessionsMessage::Open));
    // The list the core answers with (the open asked again).
    app
}

fn three_devices() -> Vec<DeviceSummary> {
    vec![
        device(
            "GALLERYDEV",
            Some("Twicecat on this computer"),
            true,
            DeviceTrust::Verified,
        ),
        device("PHONE1", Some("Phone"), false, DeviceTrust::Unverified),
        DeviceSummary {
            last_seen_ms: None,
            ..device("OLDLAPTOP", None, false, DeviceTrust::NoKeys)
        },
    ]
}

fn sessions() -> App {
    let mut app = sessions_with(three_devices());
    feed(&mut app, CoreEvent::Devices(three_devices()));
    app
}

fn sessions_confirm_one() -> App {
    let mut app = sessions();
    press(
        &mut app,
        Message::Sessions(SessionsMessage::SignOutAsk(DeviceId::from("PHONE1"))),
    );
    app
}

fn sessions_confirm_all() -> App {
    let mut app = sessions();
    press(&mut app, Message::Sessions(SessionsMessage::SignOutAllAsk));
    app
}

fn sessions_rename() -> App {
    let mut app = sessions();
    press(
        &mut app,
        Message::Sessions(SessionsMessage::RenameStart(DeviceId::from("PHONE1"))),
    );
    app
}

/// Names and IDs a server could send: very long, with no break point, with a
/// direction override and a line break, and only blanks.
fn sessions_long_names() -> App {
    let mut devices = three_devices();
    devices.push(device(
        &format!("{}ENDMARK", "D".repeat(92)),
        Some(&"N".repeat(200)),
        false,
        DeviceTrust::Unverified,
    ));
    devices.push(device(
        "EVIL1",
        Some("Phone\u{202E}gnp.exe\nsecond line"),
        false,
        DeviceTrust::Verified,
    ));
    devices.push(device("BLANK1", Some("   "), false, DeviceTrust::NoKeys));
    let mut app = sessions_with(devices.clone());
    feed(&mut app, CoreEvent::Devices(devices));
    app
}

fn sessions_recovery(recovery: RecoveryStatus) -> App {
    let mut app = sessions();
    feed(
        &mut app,
        CoreEvent::Security(status(DeviceVerification::Verified, recovery)),
    );
    app
}

fn sessions_recovery_off() -> App {
    sessions_recovery(RecoveryStatus::Disabled)
}

fn sessions_recovery_on() -> App {
    sessions_recovery(RecoveryStatus::Enabled)
}

fn sessions_loading() -> App {
    let mut app = main_room();
    press(&mut app, Message::Sessions(SessionsMessage::Open));
    app
}

fn sessions_notice() -> App {
    let mut app = sessions();
    feed(
        &mut app,
        CoreEvent::ActionFailed {
            room_id: room("lounge"),
            action: twicecat_core::api::ActionKind::Devices,
            error: CoreError::Internal("The session could not be renamed.".to_owned()),
        },
    );
    app
}

/// A sign-out of every other session that the core answered with a password
/// request.
fn sessions_signing_out() -> App {
    let mut app = sessions();
    press(&mut app, Message::Sessions(SessionsMessage::SignOutAllAsk));
    press(
        &mut app,
        Message::Sessions(SessionsMessage::SignOutConfirmed),
    );
    app
}

fn sessions_reauth() -> App {
    let mut app = sessions_signing_out();
    feed(
        &mut app,
        CoreEvent::ReauthRequired {
            action: ReauthAction::DeleteDevices,
        },
    );
    app
}

fn sessions_reauth_failed() -> App {
    let mut app = sessions_reauth();
    if let Some(prompt) = app.sessions.reauth.as_mut() {
        prompt.failed = true;
    }
    app
}

fn sessions_approval() -> App {
    let mut app = sessions_signing_out();
    feed(
        &mut app,
        CoreEvent::OpenAccountManagement(twicecat_core::api::AuthUrl::from(
            "https://account.example.org/sessions?state=dummy".to_owned(),
        )),
    );
    app
}

fn sessions_logout_warning() -> App {
    let mut app = main_room();
    feed(
        &mut app,
        CoreEvent::Devices(vec![device(
            "GALLERYDEV",
            Some("Twicecat on this computer"),
            true,
            DeviceTrust::Verified,
        )]),
    );
    feed(
        &mut app,
        CoreEvent::Security(status(
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        )),
    );
    press(&mut app, Message::LogoutRequested);
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

/// The members of `members`, plus a few made to be confusing: a name that
/// reads like a trust tag, one like a role, and a long ID.
fn rich_members() -> Vec<RoomMemberView> {
    let member =
        |id: UserId, name: Option<&str>, role: Role, level: i64, membership| RoomMemberView {
            user_id: id,
            display_name: name.map(str::to_owned),
            avatar: None,
            power: if matches!(role, Role::Creator) {
                PowerLevelView::Creator
            } else {
                PowerLevelView::Level(level)
            },
            role,
            membership,
        };
    vec![
        member(
            user("bea"),
            Some("Bea Example"),
            Role::Creator,
            100,
            MembershipView::Joined,
        ),
        member(
            user("cy"),
            Some("Cy Example"),
            Role::Admin,
            100,
            MembershipView::Joined,
        ),
        member(
            user("alice"),
            Some("Alice Example"),
            Role::Moderator,
            50,
            MembershipView::Joined,
        ),
        member(
            UserId::from("@mallory:evil.example.org"),
            Some("Verified"),
            Role::User,
            0,
            MembershipView::Joined,
        ),
        member(
            UserId::from("@mallory2:evil.example.org"),
            Some("Admin (75)"),
            Role::User,
            0,
            MembershipView::Joined,
        ),
        member(
            user("me"),
            Some("Me Example"),
            Role::User,
            0,
            MembershipView::Joined,
        ),
        member(
            user("dee"),
            Some("Dee Example"),
            Role::Moderator,
            50,
            MembershipView::Invited,
        ),
        member(
            UserId::from(format!(
                "@{}:long-server.example.org",
                "very-long-user-name-".repeat(6)
            )),
            None,
            Role::User,
            0,
            MembershipView::Joined,
        ),
    ]
}

fn members_open(list: Vec<RoomMemberView>) -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenMembers));
    feed(
        &mut app,
        CoreEvent::RoomMembers {
            room_id: room("lounge"),
            members: list,
            truncated: false,
        },
    );
    app
}

/// The members in a call, with trust for some of them.
fn members_call_trust() -> App {
    let mut app = members_open(rich_members());
    call_events(
        &mut app,
        CallState::Connected {
            room_id: room("lounge"),
            encrypted: true,
        },
    );
    call_participants(&mut app);
    for (name, trust) in [
        ("bea", UserTrust::Verified),
        ("cy", UserTrust::Unverified),
        ("alice", UserTrust::IdentityChanged),
    ] {
        feed(
            &mut app,
            CoreEvent::UserTrust {
                user_id: user(name),
                trust,
            },
        );
    }
    feed(
        &mut app,
        CoreEvent::UserTrust {
            user_id: UserId::from("@mallory:evil.example.org"),
            trust: UserTrust::Unverified,
        },
    );
    app
}

fn members_search() -> App {
    let mut app = members_open(rich_members());
    press(
        &mut app,
        Message::Rooms(RoomsMessage::MemberSearch("alice".to_owned().into())),
    );
    app
}

fn members_none() -> App {
    let mut app = members_open(rich_members());
    press(
        &mut app,
        Message::Rooms(RoomsMessage::MemberSearch("zzzz".to_owned().into())),
    );
    app
}

fn members_loading() -> App {
    let mut app = main_room();
    press(&mut app, Message::Rooms(RoomsMessage::OpenMembers));
    app
}

/// Hundreds of members: only the first page is built.
fn members_many() -> App {
    let list = (0..450)
        .map(|i| RoomMemberView {
            user_id: UserId::from(format!("@person{i:04}:example.org")),
            display_name: Some(format!("Person {i:04}")),
            avatar: None,
            power: PowerLevelView::Level(0),
            role: Role::User,
            membership: MembershipView::Joined,
        })
        .collect();
    members_open(list)
}

fn room_settings_long() -> App {
    let mut app = room_settings();
    feed(
        &mut app,
        CoreEvent::RoomDetails(RoomDetailsView {
            room_id: room("lounge"),
            name: "Lounge".to_owned(),
            topic: Some("T".repeat(300)),
            avatar: None,
            is_encrypted: false,
            alias: Some(format!("#{}:long-server.example.org", "alias-".repeat(30))),
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

fn room_encrypt_confirm() -> App {
    let mut app = room_settings();
    press(&mut app, Message::Rooms(RoomsMessage::EncryptAsk));
    app
}

fn user_card() -> App {
    let mut app = members_open(rich_members());
    press(&mut app, Message::Rooms(RoomsMessage::CloseMembers));
    press(
        &mut app,
        Message::LinkClicked(twicecat_app::rich_text::LinkTarget::User(user("alice"))),
    );
    feed(
        &mut app,
        CoreEvent::UserTrust {
            user_id: user("alice"),
            trust: UserTrust::Unverified,
        },
    );
    app
}

fn user_card_long() -> App {
    let mut app = members_open(rich_members());
    press(&mut app, Message::Rooms(RoomsMessage::CloseMembers));
    press(
        &mut app,
        Message::LinkClicked(twicecat_app::rich_text::LinkTarget::User(UserId::from(
            format!(
                "@{}:long-server.example.org",
                "very-long-user-name-".repeat(6)
            ),
        ))),
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

/// One ready file in the confirmation.
fn upload_single() -> App {
    let mut app = main_room();
    press(
        &mut app,
        Message::FilesChosen(UploadPaths(vec!["/gallery/photo.png".into()])),
    );
    let id = app
        .uploads
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.entries.first().map(|entry| entry.id))
        .expect("the confirmation has a line");
    press(
        &mut app,
        Message::UploadPrepared {
            entry: id,
            outcome: Box::new(Outcome::Ready(Box::new(Prepared {
                name: "photo.png".to_owned(),
                size: 180_000,
                source: Source::Path("/gallery/photo.png".into()),
                mime: "image/png",
                kind: MediaKind::Image,
                width: None,
                height: None,
                thumbnail: None,
                preview: None,
            }))),
        },
    );
    app
}

/// A long file name, a long room name and a room the list lacks.
fn upload_long() -> App {
    let mut app = main_room();
    for summary in &mut app.rooms {
        if summary.id == room("lounge") {
            summary.name = format!("{} lounge", "Extremely long room name ".repeat(12));
        }
    }
    let name = format!(
        "{}.tar.gz",
        "a-very-long-file-name-without-a-space-".repeat(8)
    );
    let path = format!("/gallery/{name}");
    press(
        &mut app,
        Message::FilesChosen(UploadPaths(vec![path.clone().into()])),
    );
    let id = app
        .uploads
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.entries.first().map(|entry| entry.id))
        .expect("the confirmation has a line");
    press(
        &mut app,
        Message::UploadPrepared {
            entry: id,
            outcome: Box::new(Outcome::Ready(Box::new(Prepared {
                name,
                size: 25_000_000,
                source: Source::Path(path.into()),
                mime: "application/gzip",
                kind: MediaKind::File,
                width: None,
                height: None,
                thumbnail: None,
                preview: None,
            }))),
        },
    );
    app
}

/// The room on screen changed after the files were picked.
fn upload_wrong_room() -> App {
    let mut app = upload_single();
    app.selected = Some(room("design"));
    app
}

/// A long file name (no space in it) and a long declared type.
fn viewer_long() -> App {
    let mut app = viewer();
    for item in &mut app.timeline.items {
        if let TimelineItemKind::Message { details, .. } = &mut item.kind
            && let Some(media) = details.media.as_deref_mut()
        {
            media.filename = format!("{}.png", "holiday-photo-from-a-very-long-trip-".repeat(8));
        }
    }
    app
}

fn viewer_loading() -> App {
    let mut app = main_room();
    press(&mut app, Message::MediaClicked("img0".to_owned()));
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

fn switcher_empty() -> App {
    let mut app = switcher();
    press(
        &mut app,
        Message::Switcher(SwitcherMessage::Input("zzzzzz".to_owned())),
    );
    app
}

/// Long room names, a call, and the unread badges of the list.
fn switcher_long() -> App {
    let mut app = main_room();
    for summary in &mut app.rooms {
        if summary.id == room("design") {
            summary.name = "D".repeat(300);
        }
        if summary.id == room("lounge") {
            summary.name = format!("{} end", "word ".repeat(40));
        }
    }
    feed(
        &mut app,
        CoreEvent::ActiveCall {
            room_id: room("design"),
            participants: 3,
        },
    );
    press(
        &mut app,
        Message::Shortcut {
            shortcut: twicecat_app::keys::Shortcut::Switcher,
            captured: false,
        },
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

const LONG_SERVER: &str =
    "https://a-very-long-homeserver-name.and-a-longer-subdomain.example.org:8448/end-of-it";

fn typed(user: &str, server: &str) -> App {
    let mut app = logged_out();
    press(&mut app, Message::UsernameChanged(user.to_owned()));
    press(&mut app, Message::ServerChanged(server.to_owned()));
    app
}

fn login_server() -> App {
    typed("@alice:example.org", LONG_SERVER)
}

fn login_server_open() -> App {
    let mut app = logged_out();
    press(&mut app, Message::ToggleServerField);
    app
}

fn login_server_notice() -> App {
    let mut app = logged_out();
    press(&mut app, Message::OAuthRequested);
    app
}

fn login_busy() -> App {
    let mut app = typed("alice", "matrix.example.org");
    press(
        &mut app,
        Message::PasswordChanged(twicecat_core::api::Password::from("gallery-filler")),
    );
    press(&mut app, Message::LoginSubmitted);
    app
}

fn waiting_failed() -> App {
    let mut app = waiting();
    press(&mut app, Message::BrowserOpened { ok: false });
    app
}

fn waiting_opening() -> App {
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
    app
}

fn loading_reconnecting() -> App {
    let mut app = fresh();
    feed(
        &mut app,
        CoreEvent::LoggedIn {
            user_id: user("me"),
            device_id: DeviceId::from("GALLERYDEV"),
            restored: false,
        },
    );
    feed(
        &mut app,
        CoreEvent::SyncState(SyncState::Reconnecting { attempt: 2 }),
    );
    app
}

fn closing() -> App {
    let mut app = fresh();
    press(&mut app, Message::CloseRequested);
    app
}

fn stopped() -> App {
    let mut app = fresh();
    press(&mut app, Message::CoreClosed);
    app
}

// ---- Verification ----------------------------------------------------------

/// A flow with `peer` in `phase`. The wait of the buttons that agree is
/// still running (a card that has just come up).
fn flow(peer: VerificationPeer, phase: VerificationPhase) -> App {
    let mut app = main_room();
    let id = VerificationId::from("verification-1");
    feed(
        &mut app,
        CoreEvent::VerificationRequested {
            id: id.clone(),
            peer,
        },
    );
    feed(&mut app, CoreEvent::Verification { id, phase });
    app
}

fn alice() -> VerificationPeer {
    VerificationPeer {
        user_id: user("alice"),
        device_id: Some(DeviceId::from("PHONE1")),
        device_name: Some("Alice's phone".to_owned()),
        own_account: false,
    }
}

/// The wait is over: the buttons that agree work.
fn armed(mut app: App) -> App {
    if let Some(seq) = app.verification.as_ref().and_then(|m| m.arming_seq()) {
        press(
            &mut app,
            Message::Verification(twicecat_app::verification::VerificationMessage::Armed(seq)),
        );
    }
    app
}

fn verification(phase: VerificationPhase) -> App {
    armed(flow(alice(), phase))
}

/// The same card in the first instant, while the buttons that agree wait.
fn arming(phase: VerificationPhase) -> App {
    flow(alice(), phase)
}

fn own_session(phase: VerificationPhase) -> App {
    armed(flow(
        VerificationPeer {
            user_id: user("me"),
            device_id: Some(DeviceId::from("LAPTOP2")),
            device_name: Some("Work laptop".to_owned()),
            own_account: true,
        },
        phase,
    ))
}

/// A peer whose name and ID are long and hostile-looking.
fn long_peer(phase: VerificationPhase) -> App {
    let long = format!(
        "@{}:a-very-long-homeserver-name.example.org",
        "very-long-localpart-".repeat(6)
    );
    armed(flow(
        VerificationPeer {
            user_id: UserId::from(long.as_str()),
            device_id: None,
            device_name: None,
            own_account: false,
        },
        phase,
    ))
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
fn verification_cancelled_by_us() -> App {
    verification(VerificationPhase::Cancelled {
        reason: CancelReason::ByUs,
    })
}
fn verification_cancelled_timeout() -> App {
    verification(VerificationPhase::Cancelled {
        reason: CancelReason::Timeout,
    })
}
fn verification_requested_arming() -> App {
    arming(VerificationPhase::Requested { incoming: true })
}
fn verification_scanned_arming() -> App {
    arming(VerificationPhase::QrScanned)
}
fn verification_emojis_arming() -> App {
    arming(VerificationPhase::Emojis(emojis()))
}
fn verification_decimals_arming() -> App {
    arming(VerificationPhase::Decimals([1234, 5678, 9012]))
}
fn verification_own_requested() -> App {
    own_session(VerificationPhase::Requested { incoming: true })
}
fn verification_own_emojis() -> App {
    own_session(VerificationPhase::Emojis(emojis()))
}
fn verification_own_done() -> App {
    own_session(VerificationPhase::Done)
}
fn verification_long_requested() -> App {
    long_peer(VerificationPhase::Requested { incoming: true })
}
fn verification_long_emojis() -> App {
    long_peer(VerificationPhase::Emojis(emojis()))
}

// ---- Recovery and banners --------------------------------------------------

/// Not a key: it spells out that it is a stand-in.
pub const DUMMY_KEY: &str = "DUMM YKEY DUMM YKEY DUMM YKEY 0000 0000 0000 0000 0000 0000";

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
    // The banner alone: the Confirm-your-identity screen that a fresh login
    // opens over it is skipped.
    press(
        &mut app,
        Message::Security(twicecat_app::security::SecurityMessage::SkipIdentity),
    );
    app
}

fn update_question() -> App {
    let mut app = main_room();
    app.ui_settings.update_check = UpdateCheckChoice::Unasked;
    app
}

fn update_available() -> App {
    let mut app = main_room();
    app.ui_settings.update_check = UpdateCheckChoice::On;
    feed(
        &mut app,
        CoreEvent::UpdateAvailable {
            version: "0.2.0".into(),
            url: "https://github.com/ga-lep/twicecat/releases/tag/v0.2.0".into(),
            installable: false,
        },
    );
    app
}

fn crash_report() -> App {
    let mut app = main_room();
    let dir = std::env::temp_dir().join("twicecat-gallery-crash");
    app.set_crash_reports(
        dir.clone(),
        Some(twicecat_app::crash::PendingReport {
            path: dir.join("crash-20270115-080000.txt"),
            text: "Twicecat 0.1.0 crash report".into(),
        }),
    );
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
    app.security.notice = Some(
        "Could not reach the server to set up recovery. Check your connection and try again."
            .to_owned(),
    );
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

fn recovery_key_shown() -> App {
    let mut app = recovery_key();
    press(
        &mut app,
        Message::Security(twicecat_app::security::SecurityMessage::RevealToggled(true)),
    );
    app
}

fn recovery_key_saved() -> App {
    let mut app = recovery_key_shown();
    press(
        &mut app,
        Message::Security(twicecat_app::security::SecurityMessage::SavedToggled(true)),
    );
    app
}

fn recovery_key_replacing() -> App {
    let mut app = banner_recovery_off();
    app.security.setup = Some(RecoverySetup::new(RecoveryKey::from(DUMMY_KEY), true));
    app
}

fn recovery_change_key() -> App {
    let mut app = banner_recovery_off();
    feed(
        &mut app,
        CoreEvent::Security(status(
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        )),
    );
    app.security.overlay = Some(Overlay::ChangeKeyConfirm);
    app
}

fn recovery_change_key_working() -> App {
    let mut app = recovery_change_key();
    app.security.busy = Some(twicecat_app::security::Busy::ChangingKey);
    app
}

fn recovery_restore_working() -> App {
    let mut app = recovery_restore();
    app.security.busy = Some(twicecat_app::security::Busy::Restoring);
    app
}

fn recovery_restore_failed() -> App {
    let mut app = recovery_restore();
    app.security.notice = Some("That recovery key was not accepted.".to_owned());
    app
}

fn recovery_reset_working() -> App {
    let mut app = recovery_reset_confirm();
    app.security.busy = Some(twicecat_app::security::Busy::Resetting);
    app
}

fn recovery_reset_approval() -> App {
    let mut app = recovery_reset_working();
    app.security.waiting_approval = true;
    app
}

fn recovery_identity_working() -> App {
    let mut app = recovery_confirm_identity();
    app.security.busy = Some(twicecat_app::security::Busy::Restoring);
    app
}

fn recovery_identity_asked() -> App {
    let mut app = recovery_confirm_identity();
    app.security.asked_other_device = true;
    app
}

fn recovery_identity_no_devices() -> App {
    let mut app = recovery_confirm_identity();
    app.security.status.has_devices_to_verify_against = false;
    app
}

fn recovery_reauth_waiting() -> App {
    let mut app = recovery_reauth();
    if let Some(prompt) = app.security.reauth.as_mut() {
        prompt.waiting = true;
    }
    app
}

fn recovery_reauth_failed() -> App {
    let mut app = recovery_reauth();
    app.security.notice = Some("That password was not accepted.".to_owned());
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
        s("login-server", false, login_server, |a| {
            a.screen == Screen::Login && a.login.server.len() > 60
        }),
        s("login-server-open", false, login_server_open, |a| {
            a.screen == Screen::Login && a.login.server_open
        }),
        s("login-server-notice", false, login_server_notice, |a| {
            a.screen == Screen::Login && a.notice.is_some() && a.login.server.is_empty()
        }),
        s("login-busy", false, login_busy, |a| {
            a.screen == Screen::Login && a.login_in_flight
        }),
        s("waiting", false, waiting, |a| {
            a.screen == Screen::WaitingForBrowser
        }),
        s("waiting-opening", false, waiting_opening, |a| {
            a.screen == Screen::WaitingForBrowser && a.browser_opened.is_none()
        }),
        s("waiting-failed", false, waiting_failed, |a| {
            a.screen == Screen::WaitingForBrowser && a.browser_opened == Some(false)
        }),
        s("loading", false, logged_in_loading, |a| {
            a.screen == Screen::Loading
        }),
        s("loading-reconnecting", false, loading_reconnecting, |a| {
            a.screen == Screen::Loading
                && matches!(a.sync_state, SyncState::Reconnecting { attempt: 2 })
        }),
        s("starting", false, fresh, |a| a.screen == Screen::Starting),
        s("closing", false, closing, |a| {
            a.screen == Screen::ShuttingDown
        }),
        s("stopped", false, stopped, |a| {
            a.screen == Screen::Disconnected
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
        s(
            "main-reply-suggestions",
            true,
            main_reply_suggestions,
            |a| {
                matches!(a.composer.mode, twicecat_app::composer::Mode::Reply(_))
                    && a.composer.suggestions.is_open()
            },
        ),
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
        s("main-call-unencrypted", true, main_call_unencrypted, |a| {
            matches!(
                call_phase(a),
                Some(twicecat_app::calls::CallPhase::Connected { encrypted: false })
            )
        }),
        s("main-call-ptt-muted", true, main_call_ptt_muted, |a| {
            a.ptt.talking && a.calls.call.as_ref().is_some_and(|c| c.muted)
        }),
        s("main-call-rows-open", true, main_call_rows_open, |a| {
            a.calls.rows_open.len() == 2
        }),
        s("main-call-video", true, main_call_video, |a| {
            a.stage_visible() && a.calls.video.tracks.len() == 3
        }),
        s("main-call-video-share", true, main_call_video_share, |a| {
            a.stage_visible()
                && a.calls
                    .video
                    .tracks
                    .iter()
                    .any(|t| t.kind == VideoKind::Screen)
        }),
        s(
            "main-call-video-folded",
            true,
            main_call_video_folded,
            |a| !a.calls.video.stage_open && !a.calls.video.tracks.is_empty(),
        ),
        s("main-call-sharing", true, main_call_sharing, |a| {
            a.calls.video.sharing()
        }),
        s("video-fullscreen", true, video_fullscreen, |a| {
            a.calls.video.fullscreen.is_some()
        }),
        s("share-picker", false, share_picker, |a| {
            a.calls
                .video
                .picker
                .as_ref()
                .is_some_and(|p| p.selected.is_some() && p.sources.is_some())
        }),
        s("share-picker-system", false, share_picker_system, |a| {
            a.calls
                .video
                .picker
                .as_ref()
                .is_some_and(|p| p.system_picker)
        }),
        s("ring-long-id", true, ring_long_id, |a| {
            a.calls.card().is_some_and(|r| r.caller.as_str().len() > 80)
        }),
        s("ring-dm", true, ring_dm, |a| !a.calls.rings.is_empty()),
        s("ring-group", true, ring_group, |a| {
            !a.calls.rings.is_empty()
        }),
        s(
            "audio-devices",
            false,
            || voice_page(AudioPage::Devices),
            |a| a.audio_ui.open && a.audio_ui.page == AudioPage::Devices,
        ),
        s(
            "audio-talking",
            false,
            || voice_page(AudioPage::Talking),
            |a| a.audio_ui.page == AudioPage::Talking && !a.audio_ui.capturing,
        ),
        s(
            "audio-talking-capturing",
            false,
            voice_talking_capturing,
            |a| a.audio_ui.page == AudioPage::Talking && a.audio_ui.capturing,
        ),
        s(
            "audio-talking-voice-activity",
            false,
            voice_talking_voice_activity,
            |a| a.audio_ui.page == AudioPage::Talking,
        ),
        s(
            "audio-microphone-gpu-found",
            false,
            || voice_microphone(gpu_found()),
            |a| {
                a.audio_ui.page == AudioPage::Microphone
                    && a.calls
                        .gpu
                        .as_ref()
                        .is_some_and(|g| !g.available.is_empty())
            },
        ),
        s("audio-microphone-gpu-none", false, voice_gpu_none, |a| {
            a.audio_ui.page == AudioPage::Microphone
                && a.calls.gpu.as_ref().is_some_and(|g| g.available.is_empty())
        }),
        s(
            "audio-microphone-gpu-looking",
            false,
            || voice_microphone(None),
            |a| a.audio_ui.page == AudioPage::Microphone && a.calls.gpu.is_none(),
        ),
        s(
            "audio-microphone-gpu-fallback",
            false,
            voice_gpu_fallback,
            |a| {
                a.audio_ui.page == AudioPage::Microphone
                    && a.calls
                        .gpu
                        .as_ref()
                        .is_some_and(|g| g.running == Some(EnhancerBackend::CpuAfterGpuTooSlow))
            },
        ),
        s(
            "audio-levels",
            false,
            || voice_page(AudioPage::Levels),
            |a| a.audio_ui.page == AudioPage::Levels,
        ),
        s("audio-levels-loud", false, voice_levels_loud, |a| {
            a.audio_ui.page == AudioPage::Levels && a.calls.level.is_some_and(|l| l.rms_db > -3.0)
        }),
        s("audio-mic-test", false, voice_mic_test, |a| {
            a.audio_ui.page == AudioPage::MicTest && a.audio_ui.mic_test.is_some()
        }),
        s(
            "audio-ringtone",
            false,
            || voice_page(AudioPage::Ringtone),
            |a| a.audio_ui.page == AudioPage::Ringtone,
        ),
        s(
            "audio-ringtone-sounds-off",
            false,
            voice_ringtone_sounds_off,
            |a| {
                a.audio_ui.page == AudioPage::Ringtone
                    && a.calls
                        .settings
                        .as_ref()
                        .is_some_and(|s| !s.call_sounds.enabled)
            },
        ),
        s("audio-other-voices", false, voice_others, |a| {
            a.audio_ui.page == AudioPage::OtherVoices
                && a.calls
                    .settings
                    .as_ref()
                    .is_some_and(|s| s.remote_cleaning.enabled)
        }),
        s(
            "audio-announce",
            false,
            || voice_page(AudioPage::Announce),
            |a| a.audio_ui.page == AudioPage::Announce,
        ),
        s("audio-diagnostics", false, voice_diagnostics, |a| {
            a.audio_ui.page == AudioPage::Diagnostics && a.audio_ui.diagnostics
        }),
        s(
            "audio-diagnostics-dropouts",
            false,
            voice_diagnostics_dropouts,
            |a| {
                a.audio_ui.diagnostics
                    && a.calls
                        .stats
                        .is_some_and(|s| s.capture_overruns > 0 && s.playback_underruns > 0)
            },
        ),
        s(
            "audio-devices-long-names",
            false,
            || voice_long_names(AudioPage::Devices),
            |a| a.audio_ui.page == AudioPage::Devices,
        ),
        s(
            "audio-talking-long-key",
            false,
            || voice_long_names(AudioPage::Talking),
            |a| a.audio_ui.page == AudioPage::Talking,
        ),
        s("settings", false, settings, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::Profile
        }),
        s("settings-appearance", false, settings_appearance, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::Appearance
        }),
        s(
            "settings-appearance-themes",
            false,
            settings_appearance_themes,
            |a| a.settings_ui.page == SettingsPage::Appearance && a.theme_fallback().is_none(),
        ),
        s(
            "settings-appearance-pair",
            false,
            settings_appearance_pair,
            |a| a.selected_is_paired(),
        ),
        s(
            "settings-appearance-pair-dark",
            false,
            settings_appearance_pair_dark,
            |a| a.selected_is_paired(),
        ),
        s(
            "settings-appearance-fallback",
            false,
            settings_appearance_fallback,
            |a| a.theme_fallback().is_some(),
        ),
        s(
            "settings-appearance-loading",
            false,
            settings_appearance_loading,
            |a| a.themes_loading,
        ),
        s(
            "settings-notifications",
            false,
            settings_notifications,
            |a| a.settings_ui.open && a.settings_ui.page == SettingsPage::Notifications,
        ),
        s(
            "settings-notifications-long",
            false,
            settings_notifications_long,
            |a| a.settings_ui.error.is_some() && a.settings_ui.page == SettingsPage::Notifications,
        ),
        s("settings-voice-audio", false, settings_voice_audio, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::VoiceAudio
        }),
        s("settings-video", false, settings_video, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::Video
        }),
        s(
            "settings-advanced",
            false,
            || settings_advanced(settings),
            |a| a.ui_settings.advanced_settings && a.settings_ui.page == SettingsPage::Profile,
        ),
        s(
            "settings-appearance-advanced",
            false,
            || settings_advanced(settings_appearance),
            |a| a.ui_settings.advanced_settings && a.settings_ui.page == SettingsPage::Appearance,
        ),
        s(
            "settings-notifications-advanced",
            false,
            || settings_advanced(settings_notifications),
            |a| {
                a.ui_settings.advanced_settings && a.settings_ui.page == SettingsPage::Notifications
            },
        ),
        s(
            "settings-voice-audio-advanced",
            false,
            || settings_advanced(settings_voice_audio),
            |a| a.ui_settings.advanced_settings && a.settings_ui.page == SettingsPage::VoiceAudio,
        ),
        s(
            "settings-video-advanced",
            false,
            settings_video_advanced,
            |a| a.ui_settings.advanced_settings && a.settings_ui.page == SettingsPage::Video,
        ),
        s(
            "settings-video-advanced-call",
            false,
            settings_video_advanced_in_call,
            |a| {
                a.ui_settings.advanced_settings
                    && a.settings_ui.page == SettingsPage::Video
                    && a.calls.call.is_some()
            },
        ),
        s("settings-keyboard", false, settings_keyboard, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::Keyboard
        }),
        s("settings-sessions", false, settings_sessions, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::Sessions
        }),
        s("settings-about", false, settings_about, |a| {
            a.settings_ui.open && a.settings_ui.page == SettingsPage::About
        }),
        s("sessions", false, sessions, |a| {
            a.sessions.open && a.sessions.devices.as_ref().is_some_and(|d| d.len() == 3)
        }),
        s("sessions-loading", false, sessions_loading, |a| {
            a.sessions.open && a.sessions.devices.is_none()
        }),
        s("sessions-confirm-one", false, sessions_confirm_one, |a| {
            a.sessions.confirm.is_some()
        }),
        s("sessions-confirm-all", false, sessions_confirm_all, |a| {
            a.sessions.confirm.is_some()
        }),
        s("sessions-rename", false, sessions_rename, |a| {
            a.sessions.rename.is_some()
        }),
        s("sessions-long-names", false, sessions_long_names, |a| {
            a.sessions.devices.as_ref().is_some_and(|d| d.len() == 6)
        }),
        s("sessions-recovery-off", false, sessions_recovery_off, |a| {
            a.sessions.open && a.security.status.recovery == RecoveryStatus::Disabled
        }),
        s("sessions-recovery-on", false, sessions_recovery_on, |a| {
            a.sessions.open && a.security.status.recovery == RecoveryStatus::Enabled
        }),
        s("sessions-notice", false, sessions_notice, |a| {
            a.sessions.notice.is_some()
        }),
        s("sessions-approval", false, sessions_approval, |a| {
            a.sessions.approval.is_some()
        }),
        s("sessions-reauth", false, sessions_reauth, |a| {
            a.sessions.reauth.is_some()
        }),
        s(
            "sessions-reauth-failed",
            false,
            sessions_reauth_failed,
            |a| a.sessions.reauth.as_ref().is_some_and(|p| p.failed),
        ),
        s(
            "sessions-logout-warning",
            false,
            sessions_logout_warning,
            |a| a.sessions.logout_warning,
        ),
        s("room-settings", false, room_settings, |a| {
            a.rooms_ui.settings.is_some()
        }),
        s("room-leave-confirm", false, room_leave_confirm, |a| {
            a.rooms_ui.settings.is_some()
        }),
        s("members", false, members, |a| {
            a.rooms_ui.members_panel.is_some()
        }),
        s("members-call-trust", false, members_call_trust, |a| {
            a.rooms_ui.members_panel.is_some() && a.calls.call.is_some() && a.user_trust.len() >= 4
        }),
        s("members-search", false, members_search, |a| {
            a.rooms_ui.member_search == "alice"
        }),
        s("members-none", false, members_none, |a| {
            a.rooms_ui.member_search == "zzzz"
        }),
        s("members-loading", false, members_loading, |a| {
            a.rooms_ui.members_panel.is_some() && a.rooms_ui.known.is_none()
        }),
        s("members-many", false, members_many, |a| {
            a.rooms_ui
                .known
                .as_ref()
                .is_some_and(|k| k.members.len() == 450)
        }),
        s("room-settings-long", false, room_settings_long, |a| {
            a.rooms_ui.settings.is_some()
        }),
        s("room-encrypt-confirm", false, room_encrypt_confirm, |a| {
            a.rooms_ui.settings.as_ref().is_some_and(|p| p.encrypt_ask)
        }),
        s("user-card", false, user_card, |a| a.rooms_ui.card.is_some()),
        s("user-card-long", false, user_card_long, |a| {
            a.rooms_ui.card.is_some()
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
        s("upload-single", false, upload_single, |a| {
            a.uploads
                .dialog
                .as_ref()
                .is_some_and(|d| d.sendable() == 1 && !d.loading())
        }),
        s("upload-long", false, upload_long, |a| {
            a.uploads.dialog.as_ref().is_some_and(|d| d.can_send())
        }),
        s("upload-wrong-room", false, upload_wrong_room, |a| {
            a.uploads
                .dialog
                .as_ref()
                .is_some_and(|d| Some(&d.room_id) != a.selected.as_ref())
        }),
        s("viewer-long", false, viewer_long, |a| {
            a.media
                .viewer
                .as_ref()
                .is_some_and(|v| v.decoded().is_some())
        }),
        s("viewer-loading", false, viewer_loading, |a| {
            a.media
                .viewer
                .as_ref()
                .is_some_and(|v| v.decoded().is_none())
        }),
        s("viewer", false, viewer, |a| {
            a.media
                .viewer
                .as_ref()
                .is_some_and(|v| v.decoded().is_some())
        }),
        s("switcher", false, switcher, |a| a.switcher.open),
        s("switcher-empty", false, switcher_empty, |a| {
            a.switcher.open && a.switcher.results(&a.rooms).is_empty()
        }),
        s("switcher-long", false, switcher_long, |a| {
            a.switcher.open && a.calls.count(&room("design")) == 3
        }),
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
        s(
            "verification-requested-arming",
            false,
            verification_requested_arming,
            |a| a.verification.as_ref().is_some_and(|m| !m.armed()),
        ),
        s(
            "verification-scanned-arming",
            false,
            verification_scanned_arming,
            |a| {
                verification_phase_is(a, |p| matches!(p, Phase::QrScanned))
                    && a.verification.as_ref().is_some_and(|m| !m.armed())
            },
        ),
        s(
            "verification-emojis-arming",
            false,
            verification_emojis_arming,
            |a| {
                verification_phase_is(a, |p| matches!(p, Phase::Emojis(_)))
                    && a.verification.as_ref().is_some_and(|m| !m.armed())
            },
        ),
        s(
            "verification-decimals-arming",
            false,
            verification_decimals_arming,
            |a| {
                verification_phase_is(a, |p| matches!(p, Phase::Decimals(_)))
                    && a.verification.as_ref().is_some_and(|m| !m.armed())
            },
        ),
        s(
            "verification-cancelled-us",
            false,
            verification_cancelled_by_us,
            |a| verification_phase_is(a, |p| matches!(p, Phase::Cancelled(_))),
        ),
        s(
            "verification-cancelled-timeout",
            false,
            verification_cancelled_timeout,
            |a| verification_phase_is(a, |p| matches!(p, Phase::Cancelled(_))),
        ),
        s(
            "verification-own-requested",
            false,
            verification_own_requested,
            |a| a.verification.as_ref().is_some_and(|m| m.own_account),
        ),
        s(
            "verification-own-emojis",
            false,
            verification_own_emojis,
            |a| {
                a.verification.as_ref().is_some_and(|m| m.own_account)
                    && verification_phase_is(a, |p| matches!(p, Phase::Emojis(_)))
            },
        ),
        s("verification-own-done", false, verification_own_done, |a| {
            a.verification.as_ref().is_some_and(|m| m.own_account)
                && verification_phase_is(a, |p| matches!(p, Phase::Done))
        }),
        s(
            "verification-long-requested",
            false,
            verification_long_requested,
            |a| {
                a.verification
                    .as_ref()
                    .is_some_and(|m| m.user_id.len() > 100)
            },
        ),
        s(
            "verification-long-emojis",
            false,
            verification_long_emojis,
            |a| {
                a.verification
                    .as_ref()
                    .is_some_and(|m| m.user_id.len() > 100)
                    && verification_phase_is(a, |p| matches!(p, Phase::Emojis(_)))
            },
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
        s("banner-update-question", true, update_question, |a| {
            a.update_question_shown()
        }),
        s("banner-update-available", true, update_available, |a| {
            a.updates.available.is_some()
        }),
        s("banner-crash-report", true, crash_report, |a| {
            a.crash.is_some()
        }),
        s("recovery-working", false, recovery_working, |a| {
            a.security.progress.is_some()
        }),
        s("recovery-key", false, recovery_key, |a| {
            a.security.setup.is_some()
        }),
        s("recovery-key-shown", false, recovery_key_shown, |a| {
            a.security.setup.as_ref().is_some_and(|k| k.revealed)
        }),
        s("recovery-key-saved", false, recovery_key_saved, |a| {
            a.security
                .setup
                .as_ref()
                .is_some_and(|k| k.revealed && k.saved_ticked)
        }),
        s(
            "recovery-key-replacing",
            false,
            recovery_key_replacing,
            |a| a.security.setup.as_ref().is_some_and(|k| k.replaces_old),
        ),
        s("recovery-change-key", false, recovery_change_key, |a| {
            a.security.overlay == Some(Overlay::ChangeKeyConfirm) && a.security.busy.is_none()
        }),
        s(
            "recovery-change-key-working",
            false,
            recovery_change_key_working,
            |a| a.security.busy == Some(twicecat_app::security::Busy::ChangingKey),
        ),
        s(
            "recovery-restore-working",
            false,
            recovery_restore_working,
            |a| a.security.busy == Some(twicecat_app::security::Busy::Restoring),
        ),
        s(
            "recovery-restore-failed",
            false,
            recovery_restore_failed,
            |a| a.security.notice.is_some() && a.security.overlay == Some(Overlay::Restore),
        ),
        s(
            "recovery-reset-working",
            false,
            recovery_reset_working,
            |a| a.security.busy == Some(twicecat_app::security::Busy::Resetting),
        ),
        s(
            "recovery-reset-approval",
            false,
            recovery_reset_approval,
            |a| a.security.waiting_approval,
        ),
        s(
            "recovery-identity-working",
            false,
            recovery_identity_working,
            |a| {
                a.security.overlay == Some(Overlay::ConfirmIdentity)
                    && a.security.busy == Some(twicecat_app::security::Busy::Restoring)
            },
        ),
        s(
            "recovery-identity-asked",
            false,
            recovery_identity_asked,
            |a| a.security.asked_other_device,
        ),
        s(
            "recovery-identity-no-devices",
            false,
            recovery_identity_no_devices,
            |a| !a.security.status.has_devices_to_verify_against,
        ),
        s(
            "recovery-reauth-waiting",
            false,
            recovery_reauth_waiting,
            |a| a.security.reauth.as_ref().is_some_and(|p| p.waiting),
        ),
        s(
            "recovery-reauth-failed",
            false,
            recovery_reauth_failed,
            |a| a.security.reauth.is_some() && a.security.notice.is_some(),
        ),
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
