//! The room dialogs (plan Q13, Q14, Task 12; restyled in UI/UX rework
//! Task 15): **New room**, **Direct message**, **Join a room**, the user card
//! of a clicked pill, and the layer that holds them with the room settings and
//! member panels.
//!
//! Every dialog is a [`kit::dialog`]: a header with a close button, a body that
//! scrolls when the window is short, and a footer whose last action is the main
//! one. The layer is always a stack of two (the base first), so opening a dialog
//! keeps the timeline's scroll offset and the composer's focus. Names, IDs
//! and topics are other people's text; the model keeps them raw and every
//! draw goes through [`crate::text_safety`], with a break inside a word when it
//! has none, so the end of an ID is never cut off.

use iced::widget::{Column, checkbox, column, responsive, row, space, stack, text_input};
use iced::{Element, Fill};
use twicecat_core::api::MessageText;

use crate::rooms::{
    CreateDialog, Dialog, InputDialog, JoinBar, MAX_ID_CHARS, RoomsMessage, UserCard,
};
use crate::screens::{members, room_settings};
use crate::state::{App, Message};
use crate::text_safety::{name_or_placeholder, one_line_capped};
use crate::ui::kit::{
    self, Severity, banner, banner_with, dialog, dialog_footer, dialog_header, field, muted,
    primary_button, secondary_button, words,
};
use crate::ui::tokens;
use crate::verification::elide_middle;

/// Widget ID of the new room's name field.
pub const CREATE_NAME_ID: &str = "rooms-create-name";
/// Widget ID of the direct message field.
pub const DM_INPUT_ID: &str = "rooms-dm-input";
/// Widget ID of the join field.
pub const JOIN_INPUT_ID: &str = "rooms-join-input";

/// Width of a dialog card.
pub const CARD_WIDTH: f32 = 460.0;
/// Characters of an ID on the join bar.
const BAR_ID_CHARS: usize = 64;

pub(crate) fn msg(message: RoomsMessage) -> Message {
    Message::Rooms(message)
}

/// The base with the dialog, card or panel on top, or nothing: always a
/// stack of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    stack([base, top(app).unwrap_or_else(|| space().into())]).into()
}

/// What the room layer draws, topmost first. The order is the one Esc closes
/// in (`App::escape` in `rooms.rs`): a screen is closed only when nothing
/// above it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Top {
    /// **New room**, **Direct message** or **Join a room**.
    Dialog,
    /// The user card.
    Card,
    /// The room settings.
    Settings,
    /// The member list.
    Members,
}

/// The screen of the room features that is drawn on top, if any.
pub fn topmost(ui: &crate::rooms::RoomsModel) -> Option<Top> {
    if ui.dialog.is_some() {
        Some(Top::Dialog)
    } else if ui.card.is_some() {
        Some(Top::Card)
    } else if ui.settings.is_some() {
        Some(Top::Settings)
    } else if ui.members_panel.is_some() {
        Some(Top::Members)
    } else {
        None
    }
}

/// The topmost screen of the room features, if any.
fn top(app: &App) -> Option<Element<'_, Message>> {
    let ui = &app.rooms_ui;
    match (topmost(ui)?, &ui.dialog, &ui.card) {
        (Top::Dialog, Some(Dialog::Create(dialog)), _) => {
            Some(responsive(move |size| create_dialog(dialog, size.height)).into())
        }
        (Top::Dialog, Some(Dialog::Dm(dialog)), _) => {
            Some(responsive(move |size| dm_dialog(dialog, size.height)).into())
        }
        (Top::Dialog, Some(Dialog::Join(dialog)), _) => {
            Some(responsive(move |size| join_dialog(dialog, size.height)).into())
        }
        (Top::Card, _, Some(card)) => {
            Some(responsive(move |size| user_card(app, card, size.height)).into())
        }
        (Top::Settings, ..) => room_settings::view(app),
        (Top::Members, ..) => members::view(app),
        _ => None,
    }
}

/// A line under a form: the last failure in a bar of its own, or nothing (a
/// slot that is always there).
pub(crate) fn error_line<'a>(line: Option<&'static str>) -> Element<'a, Message> {
    match line {
        Some(line) => banner(Severity::Danger, line),
        None => space().into(),
    }
}

/// The footer of a form: **Cancel**, then the main action, which reads
/// "Working..." and does nothing while it waits.
fn footer<'a>(
    cancel: RoomsMessage,
    cancel_label: &'a str,
    submit: Option<(&'a str, Message)>,
) -> Element<'a, Message> {
    let cancel = secondary_button(cancel_label, Some(msg(cancel)));
    let main = match submit {
        Some((label, message)) => primary_button(label, Some(message)),
        None => primary_button("Working\u{2026}", None),
    };
    dialog_footer(vec![cancel.into(), main.into()])
}

fn input<'a>(
    placeholder: &'a str,
    value: &str,
    editable: bool,
    on: fn(MessageText) -> RoomsMessage,
    submit: RoomsMessage,
) -> iced::widget::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input_maybe(editable.then_some(move |s: String| msg(on(MessageText::from(s)))))
        .on_submit_maybe(editable.then(|| msg(submit)))
        .padding(tokens::S)
}

fn create_dialog(dialog: &CreateDialog, window_height: f32) -> Element<'_, Message> {
    let form = &dialog.form;
    let editable = !dialog.busy;
    let submit = RoomsMessage::CreateSubmit;
    let name = input(
        "Name",
        &form.name,
        editable,
        RoomsMessage::CreateName,
        submit.clone(),
    )
    .id(CREATE_NAME_ID);
    let topic = input(
        "Topic",
        &form.topic,
        editable,
        RoomsMessage::CreateTopic,
        submit.clone(),
    );
    // The address field has a slot of its own, there or not, so the fields
    // under it keep their places (and their focus) when Public is toggled.
    let alias: Element<'_, Message> = if form.public {
        field(
            "Address",
            input(
                "my-room",
                &form.alias,
                editable,
                RoomsMessage::CreateAlias,
                submit.clone(),
            ),
        )
    } else {
        space().into()
    };
    let invitees = input(
        "@name:server @other:server",
        &form.invitees,
        editable,
        RoomsMessage::CreateInvitees,
        submit,
    );
    let body = Column::new()
        .spacing(tokens::M)
        .push(field("Name", name))
        .push(field("Topic (optional)", topic))
        .push(
            checkbox(form.encrypted)
                .label("Encrypted")
                .on_toggle_maybe(editable.then_some(|on| msg(RoomsMessage::CreateEncrypted(on)))),
        )
        .push(
            words(if form.encrypted {
                "Only the people in the room can read its messages. This cannot be turned off \
                 later."
            } else {
                "The server can read the messages of an unencrypted room. Encryption can be \
                 turned on later, but never off."
            })
            .size(tokens::CAPTION)
            .style(muted),
        )
        .push(
            checkbox(form.public)
                .label("Public (anyone can find and join it)")
                .on_toggle_maybe(editable.then_some(|on| msg(RoomsMessage::CreatePublic(on)))),
        )
        .push(alias)
        .push(field(
            "Invite people (user IDs, separated by spaces)",
            invitees,
        ))
        .push(error_line(dialog.error));
    dialog_of(
        "New room",
        RoomsMessage::CloseDialog,
        body.into(),
        footer(
            RoomsMessage::CloseDialog,
            "Cancel",
            (!dialog.busy).then(|| ("Create", msg(RoomsMessage::CreateSubmit))),
        ),
        window_height,
    )
}

/// A dialog of this module's width.
fn dialog_of<'a>(
    title: &'a str,
    close: RoomsMessage,
    body: Element<'a, Message>,
    footer: Element<'a, Message>,
    window_height: f32,
) -> Element<'a, Message> {
    dialog(
        dialog_header(title, msg(close)),
        body,
        Some(footer),
        CARD_WIDTH,
        window_height,
    )
}

/// What differs between the two one-field dialogs.
struct Field<'a> {
    title: &'a str,
    note: &'a str,
    placeholder: &'a str,
    id: &'static str,
    input: fn(MessageText) -> RoomsMessage,
    submit: RoomsMessage,
    submit_label: &'a str,
}

fn one_field<'a>(
    spec: Field<'a>,
    dialog: &'a InputDialog,
    window_height: f32,
) -> Element<'a, Message> {
    let editable = !dialog.busy;
    let submit_msg = msg(spec.submit.clone());
    let field = input(
        spec.placeholder,
        &dialog.input,
        editable,
        spec.input,
        spec.submit,
    )
    .id(spec.id);
    let body = column![
        words(spec.note).size(tokens::BODY).style(muted),
        field,
        error_line(dialog.error),
    ]
    .spacing(tokens::M);
    dialog_of(
        spec.title,
        RoomsMessage::CloseDialog,
        body.into(),
        footer(
            RoomsMessage::CloseDialog,
            "Cancel",
            (!dialog.busy).then_some((spec.submit_label, submit_msg)),
        ),
        window_height,
    )
}

fn dm_dialog(dialog: &InputDialog, window_height: f32) -> Element<'_, Message> {
    one_field(
        Field {
            title: "Direct message",
            note: "Enter the person's full user ID. If you already have a direct message \
                   with them, it is opened instead (it may not be encrypted).",
            placeholder: "@name:server",
            id: DM_INPUT_ID,
            input: RoomsMessage::DmInput,
            submit: RoomsMessage::DmSubmit,
            submit_label: "Start",
        },
        dialog,
        window_height,
    )
}

fn join_dialog(dialog: &InputDialog, window_height: f32) -> Element<'_, Message> {
    one_field(
        Field {
            title: "Join a room",
            note: "Enter an address (#room:server), a room ID (!id:server) or a matrix.to link.",
            placeholder: "#room:server",
            id: JOIN_INPUT_ID,
            input: RoomsMessage::JoinInput,
            submit: RoomsMessage::JoinSubmit,
            submit_label: "Join",
        },
        dialog,
        window_height,
    )
}

/// The words on the user card: the name of the card's target when the
/// member list knows it, and the ID in full (cleaned and capped, not cut in
/// the middle unless it is absurdly long).
pub fn card_lines(app: &App, card: &UserCard) -> (Option<String>, String) {
    let name = app
        .known_members()
        .iter()
        .find(|m| m.user_id == card.user)
        .and_then(|m| m.display_name.as_deref())
        .filter(|n| !n.trim().is_empty())
        .map(|n| name_or_placeholder(n, crate::rooms::MAX_PILL_CHARS * 2));
    (name, one_line_capped(card.user.as_str(), MAX_ID_CHARS))
}

fn user_card<'a>(app: &'a App, card: &'a UserCard, window_height: f32) -> Element<'a, Message> {
    let (name, id) = card_lines(app, card);
    let avatar = app
        .known_members()
        .iter()
        .find(|m| m.user_id == card.user)
        .and_then(|m| m.avatar.as_ref());
    let shown_name = name.clone().unwrap_or_else(|| id.clone());
    let mut who = Column::new().spacing(tokens::XS).width(Fill);
    if let Some(name) = name {
        // A name that reads like a tag ("Verified") is quoted, so it cannot
        // pass for the trust tag under it.
        who = who.push(words(members::shown_name(&name)).size(tokens::TITLE));
    }
    who = who.push(words(id).size(tokens::BODY).style(muted));
    if let Some(tag) = members::trust_tag(app, &card.user) {
        who = who.push(tag);
    }
    let message = (!card.busy).then(|| ("Message", msg(RoomsMessage::CardMessage)));
    let body = column![
        row![
            kit::avatar_sized(&card.user, &shown_name, avatar, &app.avatars, 48.0),
            who
        ]
        .spacing(tokens::M)
        .align_y(iced::Center),
        error_line(card.error),
    ]
    .spacing(tokens::M);
    let main = match message {
        Some((label, message)) => primary_button(label, Some(message)),
        None => primary_button("Working\u{2026}", None),
    };
    dialog(
        dialog_header("Person", msg(RoomsMessage::CardClose)),
        body.into(),
        Some(dialog_footer(vec![main.into()])),
        CARD_WIDTH,
        window_height,
    )
}

/// The bar above the composer that asks to join the room of a clicked pill.
/// The question has the whole width; the buttons are on a row of their own.
pub fn join_bar(bar: &JoinBar) -> Element<'_, Message> {
    let question = format!("Join {}?", elide_middle(&bar.id, BAR_ID_CHARS));
    banner_with(
        Severity::Info,
        question,
        vec![
            secondary_button("Cancel", Some(msg(RoomsMessage::JoinBarCancel))).into(),
            primary_button("Join", (!bar.busy).then(|| msg(RoomsMessage::JoinBarJoin))).into(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use iced::advanced::widget::Tree;
    use iced::widget::{text, text_editor};
    use twicecat_core::api::{
        MembershipView, PowerLevelView, Role, RoomDetailsView, RoomId, RoomMemberView,
        RoomPermissions, UserId,
    };

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::rooms::{CreateRoomForm, KnownMembers, MEMBERS_PAGE, MembersPanel, SettingsPanel};
    use crate::state::Startup;

    fn app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let (_etx, erx) = tokio::sync::mpsc::channel(1);
        App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(erx),
        })
        .0
    }

    fn room() -> RoomId {
        RoomId::from("!a:x")
    }

    fn settings(ask_encrypt: bool, ask_leave: bool) -> SettingsPanel {
        SettingsPanel {
            room: room(),
            name: "Lounge".into(),
            topic: "Chat".into(),
            name_edited: false,
            topic_edited: false,
            encrypt_ask: ask_encrypt,
            leave_ask: ask_leave,
            avatar_busy: false,
            error: None,
        }
    }

    fn details(encrypted: bool, can: bool) -> RoomDetailsView {
        RoomDetailsView {
            room_id: room(),
            name: "Lounge".into(),
            topic: None,
            avatar: None,
            is_encrypted: encrypted,
            alias: Some("#lounge:x".into()),
            permissions: RoomPermissions {
                can_set_name: can,
                can_set_topic: can,
                can_set_avatar: can,
                can_enable_encryption: can,
                can_invite: can,
            },
        }
    }

    fn members(n: usize) -> KnownMembers {
        KnownMembers {
            room: room(),
            members: (0..n)
                .map(|i| RoomMemberView {
                    user_id: UserId::from(format!("@u{i}:x")),
                    display_name: (i % 2 == 0).then(|| format!("User {i}")),
                    avatar: None,
                    power: PowerLevelView::Level(if i == 0 { 100 } else { 0 }),
                    role: if i == 0 { Role::Admin } else { Role::User },
                    membership: MembershipView::Joined,
                })
                .collect(),
            truncated: n > 5,
        }
    }

    type Step = Box<dyn Fn(&mut App)>;

    /// Every state the room features can show on top of the base.
    fn states() -> Vec<Step> {
        vec![
            Box::new(|_| {}),
            Box::new(|a| {
                a.rooms_ui.dialog = Some(Dialog::Create(CreateDialog {
                    form: CreateRoomForm::default(),
                    error: Some("An error."),
                    busy: false,
                }));
            }),
            Box::new(|a| {
                let form = CreateRoomForm {
                    public: true,
                    encrypted: false,
                    ..CreateRoomForm::default()
                };
                a.rooms_ui.dialog = Some(Dialog::Create(CreateDialog {
                    form,
                    error: None,
                    busy: true,
                }));
            }),
            Box::new(|a| {
                a.rooms_ui.dialog = Some(Dialog::Dm(InputDialog {
                    input: "@bob:x".into(),
                    error: None,
                    busy: false,
                }));
            }),
            Box::new(|a| {
                a.rooms_ui.dialog = Some(Dialog::Join(InputDialog {
                    input: "#r:x".into(),
                    error: Some("Bad."),
                    busy: true,
                }));
            }),
            Box::new(|a| {
                a.rooms_ui.dialog = None;
                a.rooms_ui.card = Some(UserCard {
                    user: UserId::from("@bob:x"),
                    busy: false,
                    error: None,
                });
            }),
            Box::new(|a| {
                a.rooms_ui.card = None;
                a.selected = Some(room());
                a.rooms_ui.settings = Some(settings(false, false));
            }),
            Box::new(|a| {
                a.rooms_ui.details = Some(details(false, true));
                a.rooms_ui.settings = Some(settings(true, false));
            }),
            Box::new(|a| {
                a.rooms_ui.details = Some(details(true, false));
                a.rooms_ui.settings = Some(settings(false, true));
            }),
            Box::new(|a| {
                a.rooms_ui.settings = None;
                a.rooms_ui.members_panel = Some(MembersPanel {
                    room: room(),
                    invite: String::new(),
                    shown: MEMBERS_PAGE,
                    status: None,
                });
            }),
            Box::new(|a| {
                a.rooms_ui.known = Some(members(250));
            }),
            Box::new(|a| {
                a.rooms_ui.members_panel = None;
                a.rooms_ui.known = None;
            }),
        ]
    }

    #[test]
    fn the_base_keeps_its_place_when_the_room_screens_come_and_go() {
        let content = text_editor::Content::new();
        fn build<'a>(app: &'a App, content: &'a text_editor::Content) -> Element<'a, Message> {
            let base = row![text("timeline"), text_editor(content)].into();
            layer(base, app)
        }
        let mut app = app();
        let tags = |tree: &Tree| {
            (
                tree.children.len(),
                tree.children[0].tag,
                tree.children[0]
                    .children
                    .iter()
                    .map(|c| c.tag)
                    .collect::<Vec<_>>(),
            )
        };
        let mut tree = Tree::new(build(&app, &content));
        let before = tags(&tree);
        assert_eq!(before.0, 2, "the base and one layer, always");
        for (i, apply) in states().iter().enumerate() {
            apply(&mut app);
            tree.diff(build(&app, &content));
            assert_eq!(tags(&tree), before, "state {i}");
        }
    }

    #[test]
    fn escape_closes_what_is_drawn_on_top_and_a_question_before_its_panel() {
        let mut app = app();
        app.selected = Some(room());
        app.rooms_ui.dialog = Some(Dialog::Dm(InputDialog {
            input: String::new(),
            error: None,
            busy: false,
        }));
        app.rooms_ui.card = Some(UserCard {
            user: UserId::from("@bob:x"),
            busy: false,
            error: None,
        });
        app.rooms_ui.settings = Some(settings(true, true));
        let mut order = Vec::new();
        while let Some(top) = topmost(&app.rooms_ui) {
            let asking = app
                .rooms_ui
                .settings
                .as_ref()
                .is_some_and(|panel| panel.encrypt_ask || panel.leave_ask);
            let _ = app.update(Message::Rooms(RoomsMessage::Escape));
            let now = topmost(&app.rooms_ui);
            if top == Top::Settings && asking {
                // The question is cancelled; the panel stays, and nothing was
                // confirmed.
                assert_eq!(now, Some(Top::Settings));
                let panel = app.rooms_ui.settings.as_ref().expect("still open");
                assert!(!panel.encrypt_ask && !panel.leave_ask);
                order.push("question");
            } else {
                assert_ne!(now, Some(top), "Esc closed {top:?}");
                order.push(match top {
                    Top::Dialog => "dialog",
                    Top::Card => "card",
                    Top::Settings => "panel",
                    Top::Members => "members",
                });
            }
            assert!(order.len() < 10);
        }
        assert_eq!(order, ["dialog", "card", "question", "panel"]);
    }

    #[test]
    fn the_layer_is_empty_without_a_screen_and_present_with_one() {
        let mut app = app();
        assert!(top(&app).is_none());
        for (i, apply) in states().iter().enumerate().skip(1).take(10) {
            apply(&mut app);
            let open = app.rooms_ui.overlay_open();
            assert_eq!(top(&app).is_some(), open, "state {i}");
        }
    }

    #[test]
    fn every_screen_lays_out() {
        use iced::advanced::layout::Limits;
        use iced::advanced::renderer::Headless;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let renderer = runtime
            .block_on(iced::Renderer::new(
                iced::Font::DEFAULT,
                iced::Pixels(16.0),
                Some("tiny-skia"),
            ))
            .expect("the headless tiny-skia renderer is not available");
        let mut app = app();
        for (i, apply) in states().iter().enumerate() {
            apply(&mut app);
            let mut element = layer(text("base").into(), &app);
            let mut tree = Tree::new(element.as_widget());
            let size = element
                .as_widget_mut()
                .layout(
                    &mut tree,
                    &renderer,
                    &Limits::new(iced::Size::ZERO, iced::Size::new(1000.0, 800.0)),
                )
                .size();
            assert!(size.width > 0.0, "state {i}");
        }
    }

    #[test]
    fn the_card_shows_the_targets_name_and_full_id_not_a_label() {
        let mut app = app();
        app.selected = Some(room());
        app.rooms_ui.known = Some(members(4));
        let card = UserCard {
            user: UserId::from("@u0:x"),
            busy: false,
            error: None,
        };
        let (name, id) = card_lines(&app, &card);
        assert_eq!(name.as_deref(), Some("User 0"));
        assert_eq!(id, "@u0:x");
        let stranger = UserCard {
            user: UserId::from(format!("@{}:evil.example", "a\u{202E}".repeat(400))),
            busy: false,
            error: None,
        };
        let (name, id) = card_lines(&app, &stranger);
        assert_eq!(name, None);
        assert!(!id.contains('\u{202E}'));
        assert_eq!(id.chars().count(), MAX_ID_CHARS);
    }

    #[test]
    fn the_join_bar_names_the_room_and_has_one_join_button() {
        let bar = JoinBar {
            id: "#new:x".into(),
            via: vec![],
            busy: false,
        };
        let mut element = join_bar(&bar);
        let _ = element.as_widget_mut();
        let long = JoinBar {
            id: format!("#{}:server.example", "r".repeat(500)),
            via: vec![],
            busy: true,
        };
        let _ = join_bar(&long);
        let shown = format!("{bar:?}");
        assert!(!shown.contains("new:x"), "{shown}");
    }
}
