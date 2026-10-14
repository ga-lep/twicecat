//! The member list panel (plan Q13, Task 12; restyled in UI/UX rework Task
//! 15): who is in the room, in sections (Admins, Moderators, Members and
//! Invited), with a search field, a trust tag and an "In the call" tag, plus
//! **Invite**. Display only; changing power levels is not part of Phase 5.
//!
//! Names and IDs are other people's text. They pass through
//! [`crate::rooms::member_lines`] (cleaned, capped, an ID cut only in the
//! middle) and are drawn with a break inside a word when there is none, so the
//! end of an ID is never cut off or covered. A name that reads like one of the
//! tags is drawn in quotes ([`shown_name`]), so it cannot pass for a status.

use std::collections::HashSet;

use iced::widget::{Column, column, responsive, row, text_input};
use iced::{Element, Fill};
use twicecat_core::api::{MessageText, RoomMemberView, UserId};

use crate::rooms::{
    MemberSection, MembersPanel, RoomsMessage, member_lines, member_page, role_badge,
};
use crate::screens::main_view::trust_badge;
use crate::screens::room_dialogs::{error_line, msg};
use crate::state::{App, Message};
use crate::ui::icons::Icon;
use crate::ui::kit::{
    self, avatar, dialog, dialog_header, empty_state, icon_tag, muted, primary_button,
    secondary_button, tag, words,
};
use crate::ui::tokens;

/// Widget ID of the search field.
pub const SEARCH_ID: &str = "members-search";
/// Widget ID of the invite field.
pub const INVITE_ID: &str = "members-invite";
/// Width of the panel.
const PANEL_WIDTH: f32 = 520.0;
/// The note under a list that was cut.
pub const TRUNCATED_NOTE: &str = "Showing the first 5,000 members";
/// What the "in the call" tag says.
pub const IN_CALL: &str = "In the call";

/// The title: how many members the list holds.
pub fn title_text(listed: Option<usize>, truncated: bool) -> String {
    match listed {
        None => "Members".to_owned(),
        Some(n) if truncated => format!("Members ({n}+)"),
        Some(n) => format!("Members ({n})"),
    }
}

/// The words of every tag a row can carry, and of the section headings, in
/// lower case. A name equal to one of them is quoted.
const TAG_WORDS: [&str; 16] = [
    "verified",
    "not verified",
    "identity changed",
    "in the call",
    "creator",
    "admin",
    "moderator",
    "invited",
    "member",
    "you",
    "muted",
    "speaking",
    "admins",
    "moderators",
    "members",
    "level",
];

/// Latin letter for a Cyrillic or Greek one that is drawn alike, so
/// "\u{410}dmin" (Cyrillic A) is read as "admin". A short table: only the
/// letters the tag words are made of.
fn latin_look(c: char) -> char {
    match c {
        '\u{430}' | '\u{3b1}' => 'a',
        '\u{435}' | '\u{3b5}' => 'e',
        '\u{43e}' | '\u{3bf}' => 'o',
        '\u{440}' | '\u{3c1}' => 'p',
        '\u{441}' => 'c',
        '\u{443}' | '\u{3c5}' => 'y',
        '\u{445}' | '\u{3c7}' => 'x',
        '\u{456}' | '\u{3b9}' => 'i',
        '\u{43a}' | '\u{3ba}' => 'k',
        '\u{43c}' => 'm',
        '\u{442}' | '\u{3c4}' => 't',
        '\u{43d}' | '\u{3bd}' => 'n',
        '\u{432}' => 'b',
        c => c,
    }
}

/// `name` the way a person would read it against the tag words: lower case,
/// look-alike letters as Latin, blanks collapsed, and anything that is not a
/// letter or a digit trimmed off both ends ("Verified \u{2713}", "\u{2605} Admin").
fn read_as(name: &str) -> String {
    let lower: String = name
        .chars()
        .flat_map(char::to_lowercase)
        .map(latin_look)
        .collect();
    let words: Vec<&str> = lower
        .trim_matches(|c: char| !c.is_alphanumeric())
        .split_whitespace()
        .collect();
    words.join(" ")
}

/// `name` as a row shows it. A name that reads like a tag or a section
/// heading ("Verified", "Verified \u{2713}", "\u{2605} Admin", "Admin (75)",
/// "Admins (2)", "Level 50", "Moderator, invited") is put in quotes, so it
/// cannot pass for the status next to it; any other name is drawn as it is.
pub fn shown_name(name: &str) -> String {
    let plain = read_as(name);
    let level = |prefix: &str| {
        plain
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(" (") || rest.is_empty())
    };
    let lookalike = TAG_WORDS.contains(&plain.as_str())
        || [
            "admin",
            "moderator",
            "creator",
            "admins",
            "moderators",
            "members",
            "invited",
        ]
        .into_iter()
        .any(level)
        || plain.starts_with("level ")
        || plain.ends_with(", invited");
    if lookalike {
        format!("\u{201c}{name}\u{201d}")
    } else {
        name.to_owned()
    }
}

/// The trust tag for `user`, when the core has reported their trust and they
/// have an identity: the words and the colour say the same. Nothing is drawn
/// for a person the core has not reported (the app does not guess).
pub fn trust_tag<'a>(app: &App, user: &UserId) -> Option<Element<'a, Message>> {
    let (_, trust) = app.user_trust.iter().find(|(known, _)| known == user)?;
    let badge = trust_badge(*trust)?;
    Some(kit::severity_tag(badge.severity(), badge.label()))
}

/// The panel, when it is open.
pub fn view(app: &App) -> Option<Element<'_, Message>> {
    let panel: &MembersPanel = app.rooms_ui.members_panel.as_ref()?;
    Some(responsive(move |size| card(app, panel, size.height)).into())
}

/// The people in the call of `room`, as a set.
fn in_call(app: &App, room: &twicecat_core::api::RoomId) -> HashSet<UserId> {
    app.calls
        .call_in(room)
        .map(|call| {
            call.participants
                .iter()
                .map(|p| p.user_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn card<'a>(app: &'a App, panel: &'a MembersPanel, window_height: f32) -> Element<'a, Message> {
    let known = app.rooms_ui.known.as_ref().filter(|k| k.room == panel.room);
    let details = app
        .rooms_ui
        .details
        .as_ref()
        .filter(|d| d.room_id == panel.room);
    let can_invite = details.is_some_and(|d| d.permissions.can_invite);

    let search = text_input("Search by name or user ID", &app.rooms_ui.member_search)
        .id(SEARCH_ID)
        .on_input(|s| msg(RoomsMessage::MemberSearch(MessageText::from(s))))
        .padding(tokens::S);
    let header = column![
        dialog_header(
            title_text(
                known.map(|k| k.members.len()),
                known.is_some_and(|k| k.truncated)
            ),
            msg(RoomsMessage::CloseMembers),
        ),
        search
    ]
    .spacing(tokens::M)
    .into();

    let invite_note: Element<'_, Message> = if details.is_none() {
        words("Checking what you may do\u{2026}")
            .size(tokens::CAPTION)
            .style(muted)
            .into()
    } else if !can_invite {
        words("You cannot invite people to this room.")
            .size(tokens::CAPTION)
            .style(muted)
            .into()
    } else {
        error_line(panel.status)
    };
    let invite = row![
        text_input("Invite by user ID, @name:server", &panel.invite)
            .id(INVITE_ID)
            .on_input_maybe(
                can_invite
                    .then_some(|s: String| msg(RoomsMessage::InviteInput(MessageText::from(s))))
            )
            .on_submit_maybe(can_invite.then(|| msg(RoomsMessage::InviteSubmit)))
            .padding(tokens::S),
        primary_button(
            "Invite",
            can_invite.then(|| msg(RoomsMessage::InviteSubmit))
        ),
    ]
    .spacing(tokens::M)
    .align_y(iced::Center);
    let footer = column![invite_note, invite].spacing(tokens::S).into();

    let body: Element<'_, Message> = match known {
        None => empty_state("Loading members\u{2026}", "Asking the server for the list."),
        Some(known) => list(app, panel, known),
    };
    dialog(header, body, Some(footer), PANEL_WIDTH, window_height)
}

fn list<'a>(
    app: &'a App,
    panel: &'a MembersPanel,
    known: &'a crate::rooms::KnownMembers,
) -> Element<'a, Message> {
    let query = app.rooms_ui.member_search.as_str();
    let page = member_page(&known.members, query, panel.shown);
    if page.matched == 0 {
        return if query.trim().is_empty() {
            empty_state("No members", "The server listed nobody for this room.")
        } else {
            empty_state("No member matches", "Try part of a name or of a user ID.")
        };
    }
    let calling = in_call(app, &panel.room);
    let mut list = Column::new().spacing(tokens::M).width(Fill);
    for group in &page.groups {
        let mut section = Column::new().spacing(tokens::S).width(Fill);
        section = section.push(
            words(format!("{} ({})", group.section.title(), group.total)).size(tokens::BODY_LARGE),
        );
        for &at in &group.indices {
            let member = &known.members[at];
            section = section.push(member_row(
                app,
                member,
                group.section,
                calling.contains(&member.user_id),
            ));
        }
        list = list.push(section);
    }
    if page.matched > panel.shown {
        list = list.push(secondary_button(
            "Show more",
            Some(msg(RoomsMessage::ShowMoreMembers)),
        ));
    }
    if known.truncated {
        list = list.push(words(TRUNCATED_NOTE).size(tokens::CAPTION).style(muted));
    }
    list.into()
}

/// The tag for a role, unless the section already says as much.
fn role_tag(member: &RoomMemberView, section: MemberSection) -> Option<String> {
    let badge = role_badge(member)?;
    let said = match section {
        MemberSection::Admins => "Admin",
        MemberSection::Moderators => "Moderator",
        MemberSection::Members => "",
        MemberSection::Invited => "Invited",
    };
    (badge != said).then_some(badge)
}

fn member_row<'a>(
    app: &'a App,
    member: &'a RoomMemberView,
    section: MemberSection,
    calling: bool,
) -> Element<'a, Message> {
    let (name, id) = member_lines(member);
    let mut who = Column::new().width(Fill).spacing(tokens::XS);
    who = who.push(words(shown_name(&name)).size(tokens::BODY));
    if id != name {
        who = who.push(words(id).size(tokens::CAPTION).style(muted));
    }
    // The tags wrap onto the next line rather than squeeze the words.
    let mut tags = row![].spacing(tokens::XS);
    let mut any = false;
    if let Some(role) = role_tag(member, section) {
        tags = tags.push(tag(role));
        any = true;
    }
    if let Some(trust) = trust_tag(app, &member.user_id) {
        tags = tags.push(trust);
        any = true;
    }
    if calling {
        tags = tags.push(icon_tag(Icon::Speaker, IN_CALL.to_owned(), false));
        any = true;
    }
    if any {
        who = who.push(tags.wrap().vertical_spacing(tokens::XS));
    }
    row![
        avatar(&member.user_id, &name, member.avatar.as_ref(), &app.avatars),
        who
    ]
    .spacing(tokens::M)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_counts_the_listed_members_and_marks_a_cut_list() {
        assert_eq!(title_text(None, false), "Members");
        assert_eq!(title_text(Some(3), false), "Members (3)");
        assert_eq!(title_text(Some(5000), true), "Members (5000+)");
    }

    #[test]
    fn a_name_that_reads_like_a_tag_is_quoted_and_another_is_not() {
        for name in [
            "Verified",
            " verified ",
            "NOT VERIFIED",
            "Identity changed",
            "In the call",
            "Admin",
            "Admin (75)",
            "Moderator (10)",
            "Level 50",
            "Creator",
            "Moderator, invited",
            "Invited",
            "You",
            "Admins (3)",
            "Moderators (12)",
            "Members (40)",
            "Invited (2)",
            "Members",
            "Verified \u{2713}",
            "\u{2605} Admin",
            "\u{2605} Admin \u{2605}",
            "Not  verified",
            "\u{410}dmin",
            "Mod\u{435}rator",
            "\u{3a5}ou",
            "[Verified]",
        ] {
            let shown = shown_name(name);
            assert!(
                shown.starts_with('\u{201c}') && shown.ends_with('\u{201d}'),
                "{name:?} -> {shown:?}"
            );
            assert!(shown.contains(name));
        }
        for name in [
            "Alice Example",
            "Verified Vera",
            "Adminton",
            "Levelled",
            "Bob",
        ] {
            assert_eq!(shown_name(name), name);
        }
    }

    #[test]
    fn the_role_tag_says_only_what_the_section_does_not() {
        use twicecat_core::api::{MembershipView, PowerLevelView, Role};
        let member = |role, power, membership| RoomMemberView {
            user_id: UserId::from("@a:x"),
            display_name: None,
            avatar: None,
            power,
            role,
            membership,
        };
        let admin = member(
            Role::Admin,
            PowerLevelView::Level(100),
            MembershipView::Joined,
        );
        assert_eq!(role_tag(&admin, MemberSection::Admins), None);
        let odd = member(
            Role::Admin,
            PowerLevelView::Level(75),
            MembershipView::Joined,
        );
        assert_eq!(
            role_tag(&odd, MemberSection::Admins).as_deref(),
            Some("Admin (75)")
        );
        let creator = member(
            Role::Creator,
            PowerLevelView::Creator,
            MembershipView::Joined,
        );
        assert_eq!(
            role_tag(&creator, MemberSection::Admins).as_deref(),
            Some("Creator")
        );
        let invited = member(
            Role::User,
            PowerLevelView::Level(0),
            MembershipView::Invited,
        );
        assert_eq!(role_tag(&invited, MemberSection::Invited), None);
        let invited_mod = member(
            Role::Moderator,
            PowerLevelView::Level(50),
            MembershipView::Invited,
        );
        assert_eq!(
            role_tag(&invited_mod, MemberSection::Invited).as_deref(),
            Some("Moderator, invited")
        );
    }

    #[test]
    fn the_cut_list_note_says_5000() {
        assert_eq!(TRUNCATED_NOTE, "Showing the first 5,000 members");
    }
}
