//! The login screen, the "continue in your browser" screen, and the guard in
//! front of the browser launcher.
//!
//! The OAuth URL carries `state` and a code challenge, so it is never logged
//! or shown. Before it reaches `open`, [`check_browser_url`] re-checks it
//! (plan Q21): `https`, or `http` only to a loopback host, and no userinfo.

use std::fmt;

use iced::widget::button::Status;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Element, Fill, Theme};
use twicecat_core::api::Password;

use crate::state::{App, Message};
use crate::ui::icons::{self, Icon};
use crate::ui::kit::{self, Severity};
use crate::ui::{fonts, tokens};
use crate::verification::elide_middle;

/// The login form's fields.
pub struct LoginForm {
    /// Server name or URL; empty means "from the user ID".
    pub server: String,
    /// Localpart or full MXID.
    pub username: String,
    /// The password; cleared on submit and on failure (plan Q19).
    pub password: Password,
    /// The user opened **Use another server**.
    pub server_open: bool,
}

impl LoginForm {
    /// An empty form.
    pub fn new() -> Self {
        Self {
            server: String::new(),
            username: String::new(),
            password: Password::from(String::new()),
            server_open: false,
        }
    }
}

impl Default for LoginForm {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for LoginForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginForm")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("password", &self.password)
            .finish()
    }
}

/// A URL that passed [`check_browser_url`]. It is the only thing
/// [`open_in_browser`] accepts.
#[derive(Clone, PartialEq, Eq)]
pub struct BrowserUrl(String);

impl BrowserUrl {
    /// The checked URL.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Manual impl: the URL carries OAuth `state`.
impl fmt::Debug for BrowserUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrowserUrl(<redacted>)")
    }
}

/// Why a URL was not allowed to open. Never carries the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlRefused {
    /// Not a plain ASCII `scheme://host[:port]/...` URL.
    Malformed,
    /// The scheme is neither `https` nor `http`.
    Scheme,
    /// The URL has a user name or password part.
    Userinfo,
    /// `http` to a host that is not loopback.
    InsecureHost,
}

impl fmt::Display for UrlRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the sign-in address is malformed",
            Self::Scheme => "the sign-in address does not use https",
            Self::Userinfo => "the sign-in address contains a user name or password",
            Self::InsecureHost => "the sign-in address uses http to a remote host",
        })
    }
}

/// Parses `url` and allows it only if it is `https`, or `http` to
/// `127.0.0.1`, `[::1]` or `localhost`, with no userinfo.
///
/// The check is deliberately stricter than a browser's URL parser: the whole
/// string must be ASCII with no whitespace, control characters or
/// backslashes, and the host must be a plain DNS name, IPv4 address or
/// bracketed IPv6 address.
pub fn check_browser_url(url: &str) -> Result<BrowserUrl, UrlRefused> {
    if !url
        .chars()
        .all(|c| c.is_ascii() && !c.is_ascii_control() && c != ' ' && c != '\\')
    {
        return Err(UrlRefused::Malformed);
    }
    let (scheme, rest) = url.split_once("://").ok_or(UrlRefused::Scheme)?;
    let https = scheme.eq_ignore_ascii_case("https");
    if !https && !scheme.eq_ignore_ascii_case("http") {
        return Err(UrlRefused::Scheme);
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(UrlRefused::Userinfo);
    }
    let host = host_of(authority)?;
    if !https {
        let loopback = ["127.0.0.1", "[::1]", "localhost"]
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(allowed));
        if !loopback {
            return Err(UrlRefused::InsecureHost);
        }
    }
    Ok(BrowserUrl(url.to_owned()))
}

/// The host part of `host[:port]`, checked for shape.
pub(crate) fn host_of(authority: &str) -> Result<&str, UrlRefused> {
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']').ok_or(UrlRefused::Malformed)?;
        let (host, after) = authority.split_at(end + 1);
        let port = match after.strip_prefix(':') {
            Some(port) => Some(port),
            None if after.is_empty() => None,
            None => return Err(UrlRefused::Malformed),
        };
        let inner = &host[1..end];
        let plausible = !inner.is_empty()
            && inner.contains(':')
            && inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
        if !plausible {
            return Err(UrlRefused::Malformed);
        }
        (host, port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if let Some(port) = port {
        let digits =
            !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit());
        if !digits || port.parse::<u16>().is_err() {
            return Err(UrlRefused::Malformed);
        }
    }
    if !host.starts_with('[')
        && (host.len() > crate::links::MAX_HOST_BYTES
            || !host
                .split('.')
                .all(|label| label.len() <= crate::links::MAX_LABEL_BYTES && is_dns_label(label)))
    {
        return Err(UrlRefused::Malformed);
    }
    Ok(host)
}

/// A non-empty run of letters, digits and inner hyphens.
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Opens a checked URL in the default browser.
///
/// This is slow (it starts a process) and must only run inside a `Task`, never
/// in `update`. The error is dropped on purpose: it cannot help the user and
/// must not leak the URL.
pub fn open_in_browser(url: &BrowserUrl) -> bool {
    open::that_detached(url.as_str()).is_ok()
}

/// Width of the login card.
pub const CARD_WIDTH: f32 = 420.0;

/// Widget id of the user field.
pub const USER_ID: &str = "login-user";
/// Widget id of the password field.
pub const PASSWORD_ID: &str = "login-password";
/// Widget id of the server field.
pub const SERVER_ID: &str = "login-server";

/// What the Tab key and the eye can stop at on the login card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStop {
    /// The user name field.
    User,
    /// The password field.
    Password,
    /// **Log in**.
    LogIn,
    /// **Sign in with browser**.
    Browser,
    /// **Use another server**.
    Disclosure,
    /// The server field (only while the disclosure is open).
    Server,
}

impl LoginStop {
    /// Whether this iced's Tab key stops here. Only text inputs take focus;
    /// buttons do not (`iced_widget-0.14.2/src/button.rs` never calls
    /// `Operation::focusable`), so in practice Tab visits the inputs in
    /// [`focus_order`] and the buttons are reached with the mouse.
    pub const fn is_input(self) -> bool {
        matches!(self, Self::User | Self::Password | Self::Server)
    }
}

/// The order of the card, top to bottom: the order the view builds its
/// widgets in, and so the order Tab walks them.
pub fn focus_order(server_open: bool) -> Vec<LoginStop> {
    let mut order = vec![
        LoginStop::User,
        LoginStop::Password,
        LoginStop::LogIn,
        LoginStop::Browser,
        LoginStop::Disclosure,
    ];
    if server_open {
        order.push(LoginStop::Server);
    }
    order
}

/// Whether a notice is about the server field: the words name the server
/// or the network, so the field it belongs to must be in view.
pub fn notice_is_about_server(notice: &str) -> bool {
    let lower = notice.to_lowercase();
    ["server", "network", "sign-in address"]
        .iter()
        .any(|word| lower.contains(word))
}

/// The server field shows when the user opened it, when it holds something,
/// or when the notice is about it.
pub fn server_field_open(opened: bool, server: &str, notice: Option<&str>) -> bool {
    opened || !server.trim().is_empty() || notice.is_some_and(notice_is_about_server)
}

/// Whether Enter may send the password login: an idle form with both fields
/// filled. (**Log in** itself stays pressable when idle, so a click on an
/// empty form still explains what is missing.)
pub fn can_submit(username: &str, password: &str, busy: bool) -> bool {
    !busy && !username.trim().is_empty() && !password.is_empty()
}

/// Shown instead of a destination when the address hides its real server,
/// and as the notice when a sign-in is refused for it.
pub const HIDDEN_SERVER: &str = "This address hides its real server. Enter just the server name.";

/// Shown for a server name that is honest but cannot be used as typed: a
/// character outside letters, digits, dots and hyphens (an underscore, an
/// accent, a second trailing dot), so it is not mistaken for a trick.
pub const UNSUPPORTED_SERVER: &str =
    "Use letters, digits, dots and hyphens only; write international names in punycode.";

/// Shown for `http://` to anywhere but this computer: the core refuses it
/// too, but only after the password has left the form.
pub const INSECURE_SERVER: &str =
    "Plain http:// is only allowed for localhost. Use https:// or just the server name.";

/// How many characters of `scheme://host[:port]` the echo shows before it
/// cuts the middle out.
const ECHO_BUDGET: usize = 120;

/// What the Server field holds, judged the way the core reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerInput {
    /// Nothing (or blanks): the server comes from the user ID.
    Empty,
    /// An address whose real host could be hidden (a userinfo part, a
    /// backslash, a percent sign, blanks, an odd scheme form or an odd
    /// number or port): it is not echoed and no sign-in is sent for it.
    Hidden,
    /// A name with no trick in it that still is not a plain host name:
    /// a character other than letters, digits, dots and hyphens (an
    /// underscore, an accent, an empty label). Refused, with
    /// [`UNSUPPORTED_SERVER`].
    Unsupported,
    /// `http://` to a host that is not this computer (the core's policy).
    /// Refused, with [`INSECURE_SERVER`].
    Insecure,
    /// The place that will be contacted: `scheme://host[:port]` for an
    /// `http(s)` URL, the bare authority for a server name. Not yet cleaned
    /// for display.
    Target(String),
}

/// How an authority reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authority {
    /// An ASCII host of letters, digits, dots and dashes (no empty label), an
    /// `[ipv6]` literal, or a dotted quad in plain decimal, then an optional
    /// `:port` of digits: contacted as shown.
    Plain,
    /// Something that could hide the real host (a userinfo part, a percent
    /// sign, a backslash, `0x7f.1`, a bad port or bracket): never echoed.
    Hides,
    /// A host with a character outside letters, digits, dots and hyphens, or
    /// an empty label, and nothing that looks like a trick.
    Unsupported,
}

/// Reads an authority. The URL parser the SDK falls back to percent-decodes
/// and IDNA-maps the host, reads `0x7f.1` as an address and treats many
/// look-alike characters as separators, so only [`Authority::Plain`] is
/// contacted. A characters-only failure (an underscore, an accent) is
/// [`Authority::Unsupported`], not a trick.
fn read_authority(authority: &str) -> Authority {
    if authority.contains(['@', '\\', '%']) {
        return Authority::Hides;
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let Some((v6, after)) = inner.split_once(']') else {
            return Authority::Hides;
        };
        let literal = !v6.is_empty()
            && v6.contains(':')
            && v6
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
        if !literal {
            return Authority::Hides;
        }
        match after.strip_prefix(':') {
            Some(p) => ("", Some(p)),
            None if after.is_empty() => ("", None),
            None => return Authority::Hides,
        }
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if let Some(p) = port {
        let digits = !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit());
        if !digits || p.parse::<u32>().map_or(true, |n| n > 65535) {
            return Authority::Hides;
        }
    }
    if authority.starts_with('[') {
        return Authority::Plain;
    }
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || host.split('.').any(str::is_empty)
    {
        return Authority::Unsupported;
    }
    // A last label starting with a digit makes the parser read an IPv4
    // address (`127.1`, `0x7f.1`, `2130706433`): only a plain quad is kept.
    let numeric_last = host
        .rsplit('.')
        .next()
        .is_some_and(|l| l.starts_with(|c: char| c.is_ascii_digit()));
    if numeric_last {
        let parts: Vec<&str> = host.split('.').collect();
        let quad = parts.len() == 4
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.bytes().all(|b| b.is_ascii_digit())
                    && (p.len() == 1 || !p.starts_with('0'))
                    && p.parse::<u16>().is_ok_and(|n| n <= 255)
            });
        return if quad {
            Authority::Plain
        } else {
            Authority::Hides
        };
    }
    Authority::Plain
}

/// Whether a host that [`read_authority`] found plain is this computer, as
/// the core's transport policy reads it: `localhost`, a loopback IPv4
/// address or `[::1]`-like loopback IPv6.
fn is_loopback_host(authority: &str) -> bool {
    if let Some(inner) = authority.strip_prefix('[') {
        let v6 = inner.split_once(']').map_or(inner, |(v6, _)| v6);
        return v6
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback());
    }
    let host = authority.split(':').next().unwrap_or_default();
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Characters that draw nothing or reorder what is drawn (zero-width and
/// bidirectional marks, variation selectors, tag characters): a trick, not an
/// international name.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{34f}'
            | '\u{61c}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// Where the authority of an already trimmed `s` is, and its scheme. `Err`
/// is the refusal for a form that cannot be read at all.
fn locate_authority(
    s: &str,
) -> Result<(Option<&'static str>, std::ops::Range<usize>), ServerInput> {
    if s.chars()
        .any(|c| c.is_whitespace() || c.is_control() || is_invisible(c))
    {
        return Err(ServerInput::Hidden);
    }
    let lower = s.to_ascii_lowercase();
    let scheme = ["https", "http"]
        .into_iter()
        .find(|name| lower.strip_prefix(name).is_some_and(|r| r.starts_with(':')));
    // Any other `scheme://` is not a server the core takes.
    if scheme.is_none()
        && lower
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|a| a.contains("://"))
    {
        return Err(ServerInput::Hidden);
    }
    let start = match scheme {
        Some(name) => {
            let rest = &s[name.len() + 1..];
            match rest.strip_prefix("//") {
                Some(r) if !r.starts_with(['/', '\\']) => s.len() - r.len(),
                _ => return Err(ServerInput::Hidden),
            }
        }
        None => 0,
    };
    let end = s[start..]
        .find(['/', '?', '#'])
        .map_or(s.len(), |at| start + at);
    Ok((scheme, start..end))
}

/// The host of `authority` without exactly one trailing dot (`matrix.org.`
/// is the same name as `matrix.org`); `matrix.org..` keeps one and fails.
fn without_one_trailing_dot(authority: &str) -> String {
    match authority.rsplit_once(':') {
        Some((host, port)) if !authority.ends_with(']') && !port.contains(']') => {
            format!("{}:{port}", host.strip_suffix('.').unwrap_or(host))
        }
        _ => authority.strip_suffix('.').unwrap_or(authority).to_owned(),
    }
}

/// Reads the Server field. The core takes a bare server name or an `http(s)`
/// URL (the URL parser also accepts `https:host` and backslashes, and drops
/// tabs and newlines). The authority is the text up to the first `/`, `?` or
/// `#`; a `@` there starts a userinfo part that can be any length and moves
/// the real host to the end, so any `@` or `\\` in the authority, any
/// blank or control character, and an `http(s):` not followed by exactly
/// `//` is [`ServerInput::Hidden`]; so is a percent sign. A host with another
/// odd character (an underscore, an accent) is [`ServerInput::Unsupported`]
/// and `http://` to a host that is not this computer is
/// [`ServerInput::Insecure`]. One trailing dot is taken off the host first.
/// This is a conservative guard in the app; the core is unchanged.
pub fn classify_server(raw: &str) -> ServerInput {
    let s = raw.trim();
    if s.is_empty() {
        return ServerInput::Empty;
    }
    let (scheme, range) = match locate_authority(s) {
        Ok(found) => found,
        Err(refused) => return refused,
    };
    let authority = without_one_trailing_dot(&s[range]);
    match read_authority(&authority) {
        Authority::Hides => return ServerInput::Hidden,
        Authority::Unsupported => return ServerInput::Unsupported,
        Authority::Plain => {}
    }
    if scheme == Some("http") && !is_loopback_host(&authority) {
        return ServerInput::Insecure;
    }
    ServerInput::Target(match scheme {
        Some(name) => format!("{name}://{authority}"),
        None => authority,
    })
}

/// The server to send to the core: the trimmed field with exactly one
/// trailing dot taken off the host, as [`classify_server`] read it. Anything
/// else of what was typed (a path, a port) is left as it was. `None` for an
/// empty field.
pub fn server_to_send(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let Ok((_, range)) = locate_authority(s) else {
        return Some(s.to_owned());
    };
    Some(format!(
        "{}{}{}",
        &s[..range.start],
        without_one_trailing_dot(&s[range.clone()]),
        &s[range.end..]
    ))
}

/// The notice for an input that [`classify_server`] refused, or `None` when
/// it can be used.
pub fn server_refusal(raw: &str) -> Option<&'static str> {
    match classify_server(raw) {
        ServerInput::Hidden => Some(HIDDEN_SERVER),
        ServerInput::Unsupported => Some(UNSUPPORTED_SERVER),
        ServerInput::Insecure => Some(INSECURE_SERVER),
        ServerInput::Empty | ServerInput::Target(_) => None,
    }
}

/// Whether the Server field must stay in view: it holds text, or the notice
/// is about the server. The disclosure button is then off.
pub fn server_held(server: &str, notice: Option<&str>) -> bool {
    !server.trim().is_empty() || notice.is_some_and(notice_is_about_server)
}

/// The line under the server field: where the sign-in will go, cleaned and
/// cut in the middle (never at the end, which holds the real host) when too
/// long; the [`HIDDEN_SERVER`] words for an address that hides its server; or
/// what an empty field means.
pub fn server_echo(server: &str) -> String {
    match classify_server(server) {
        ServerInput::Empty => "Leave it empty to use the server of your user ID.".to_owned(),
        ServerInput::Hidden => HIDDEN_SERVER.to_owned(),
        ServerInput::Unsupported => UNSUPPORTED_SERVER.to_owned(),
        ServerInput::Insecure => INSECURE_SERVER.to_owned(),
        ServerInput::Target(target) => {
            format!(
                "Signs in with the server of {}",
                elide_middle(&target, ECHO_BUDGET)
            )
        }
    }
}

/// A full-width button; the label is centred and never wraps.
fn wide_button<'a>(
    label: &'a str,
    on_press: Option<Message>,
    style: fn(&Theme, Status) -> button::Style,
) -> Element<'a, Message> {
    button(
        text(label)
            .width(Fill)
            .align_x(iced::Center)
            .wrapping(Wrapping::None),
    )
    .padding([tokens::S, tokens::M])
    .width(Fill)
    .on_press_maybe(on_press)
    .style(style)
    .into()
}

/// `content` in a [`kit::card`] of at most [`CARD_WIDTH`], in the middle of
/// the window.
pub fn centered_card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(container(kit::card(content.into())).max_width(CARD_WIDTH))
        .padding(tokens::L)
        .center_x(Fill)
        .center_y(Fill)
        .into()
}

/// The login card.
pub fn view(app: &App) -> Element<'_, Message> {
    let form = &app.login;
    let busy = app.login_in_flight;
    let notice = app.notice.as_deref();
    let open = server_field_open(form.server_open, &form.server, notice);
    let refusal = server_refusal(&form.server);
    let hidden = refusal.is_some();
    let held = server_held(&form.server, notice);
    let enter = (can_submit(&form.username, form.password.expose(), busy) && !hidden)
        .then_some(Message::LoginSubmitted);

    let mut card = column![
        text("Twicecat")
            .size(tokens::DISPLAY)
            .font(fonts::BODY_SEMIBOLD),
        text("Chat and voice calls on Matrix.")
            .size(tokens::CAPTION)
            .style(kit::muted),
    ]
    .spacing(tokens::XS);
    let mut body = column![].spacing(tokens::M).width(Fill);
    if let Some(notice) = notice {
        body = body.push(kit::banner(Severity::Danger, notice));
    }
    body = body
        .push(kit::field(
            "User name",
            text_input("alice or @alice:example.org", &form.username)
                .id(USER_ID)
                .on_input_maybe((!busy).then_some(Message::UsernameChanged))
                .on_submit_maybe(enter.clone())
                .padding(tokens::S),
        ))
        .push(kit::field(
            "Password",
            // Secure: the dots are drawn, the text is never shown or copied.
            text_input("Password", form.password.expose())
                .id(PASSWORD_ID)
                .secure(true)
                .on_input_maybe(
                    (!busy).then_some(|s: String| Message::PasswordChanged(Password::from(s))),
                )
                .on_submit_maybe(enter.clone())
                .padding(tokens::S),
        ))
        .push(wide_button(
            if busy { "Signing in\u{2026}" } else { "Log in" },
            (!busy).then_some(Message::LoginSubmitted),
            kit::primary_style,
        ))
        .push(
            row![
                kit::divider(),
                text("or").size(tokens::CAPTION).style(kit::muted),
                kit::divider()
            ]
            .spacing(tokens::S)
            .align_y(iced::Center),
        )
        .push(wide_button(
            "Sign in with browser",
            (!busy).then_some(Message::OAuthRequested),
            kit::secondary_style,
        ))
        .push(
            button(
                row![
                    icons::icon(if open {
                        Icon::Minus
                    } else {
                        Icon::ChevronRight
                    }),
                    text("Use another server")
                ]
                .spacing(tokens::S),
            )
            .padding([tokens::XS, tokens::S])
            .on_press_maybe((!busy && !held).then_some(Message::ToggleServerField))
            .style(kit::ghost_style),
        );
    // One slot at the end, filled or empty, so the widgets above keep their
    // place in the tree when the field comes and goes.
    body = body.push(if open {
        Element::from(
            column![
                kit::field(
                    "Server",
                    text_input("matrix.example.org", &form.server)
                        .id(SERVER_ID)
                        .on_input_maybe((!busy).then_some(Message::ServerChanged))
                        .on_submit_maybe(enter)
                        .padding(tokens::S)
                ),
                // Where the sign-in will go (the field itself scrolls and
                // would hide the end). A long host is cut in the middle so
                // its last label shows; an address with userinfo is not
                // shown as a destination at all and cannot be sent.
                if let Some(refusal) = refusal {
                    kit::banner(Severity::Danger, refusal)
                } else {
                    text(server_echo(&form.server))
                        .size(tokens::CAPTION)
                        .style(kit::muted)
                        .width(Fill)
                        .wrapping(Wrapping::WordOrGlyph)
                        .into()
                },
            ]
            .spacing(tokens::XS),
        )
    } else {
        Space::new().into()
    });
    card = card.push(body);
    centered_card(card.spacing(tokens::L).width(Fill))
}

/// "Continue in your browser", with **Cancel**, and **Copy link** when the
/// browser could not be started. The URL itself is never shown.
pub fn waiting_view(app: &App) -> Element<'_, Message> {
    let status = match app.browser_opened {
        None => "Opening your browser\u{2026}",
        Some(true) => "Finish signing in in the browser window, then come back here.",
        Some(false) => "The browser could not be started. Copy the link and open it yourself.",
    };
    let mut buttons = row![].spacing(tokens::S);
    if app.browser_opened == Some(false) && app.browser_link.is_some() {
        buttons = buttons.push(kit::secondary_button("Copy link", Some(Message::CopyLink)));
    }
    buttons = buttons.push(kit::ghost_button("Cancel", Some(Message::CancelOAuth)));
    centered_card(
        column![
            text("Continue in your browser")
                .size(tokens::HEADLINE)
                .wrapping(kit::BANNER_WRAPPING),
            if app.browser_opened == Some(false) {
                kit::banner(Severity::Warning, status)
            } else {
                text(status)
                    .width(Fill)
                    .wrapping(kit::BANNER_WRAPPING)
                    .into()
            },
            buttons
        ]
        .spacing(tokens::L)
        .width(Fill),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn ok(url: &str) {
        assert!(check_browser_url(url).is_ok(), "should be allowed: {url}");
    }

    fn refused(url: &str, why: UrlRefused) {
        assert_eq!(
            check_browser_url(url).map(|_| ()),
            Err(why),
            "should be refused: {url:?}"
        );
    }

    #[test]
    fn https_is_allowed() {
        ok("https://auth.example.org/authorize?state=s&code_challenge=c#f");
        ok("https://auth.example.org");
        ok("https://auth.example.org:8448/x");
        ok("HTTPS://Auth.Example.org/x");
        ok("https://203.0.113.7/x");
        ok("https://[2001:db8::1]:443/x");
        ok("https://localhost/x");
    }

    #[test]
    fn http_to_loopback_is_allowed() {
        ok("http://127.0.0.1/x");
        ok("http://127.0.0.1:8008/_matrix?x=1");
        ok("http://[::1]/x");
        ok("http://[::1]:8080/x");
        ok("http://localhost:8080/x");
        ok("HTTP://LocalHost/x");
    }

    #[test]
    fn other_http_is_refused() {
        for url in [
            "http://auth.example.org/x",
            "http://127.0.0.2/x",
            "http://10.0.0.1/x",
            "http://0.0.0.0/x",
            "http://[::2]/x",
            "http://[::ffff:127.0.0.1]/x",
            "http://localhost.evil.com/x",
            "http://127.0.0.1.evil.com/x",
            "http://evil.com/?h=127.0.0.1",
            "http://evil.com/#@127.0.0.1",
        ] {
            refused(url, UrlRefused::InsecureHost);
        }
    }

    #[test]
    fn userinfo_is_refused() {
        refused("https://user:pw@auth.example.org/x", UrlRefused::Userinfo);
        refused("https://user@auth.example.org/x", UrlRefused::Userinfo);
        refused("https://@auth.example.org/x", UrlRefused::Userinfo);
        refused("http://127.0.0.1@evil.com/x", UrlRefused::Userinfo);
        refused("http://127.0.0.1:80@evil.com/x", UrlRefused::Userinfo);
        refused("http://user@127.0.0.1/x", UrlRefused::Userinfo);
        // '@' after the authority is part of the path, not userinfo.
        ok("https://auth.example.org/@user");
        ok("https://auth.example.org/x?a=b@c");
    }

    #[test]
    fn other_schemes_are_refused() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "javascript://auth.example.org/%0aalert(1)",
            "data:text/html,<script>1</script>",
            "file:///etc/passwd",
            "ftp://auth.example.org/x",
            "mailto:a@b.c",
            "ssh://host/x",
            "https:auth.example.org/x",
            "auth.example.org/x",
            "//auth.example.org/x",
            "",
        ] {
            refused(url, UrlRefused::Scheme);
        }
    }

    #[test]
    fn malformed_urls_are_refused() {
        for url in [
            "https://",
            "https:///x",
            "https://:443/x",
            "https://auth.example.org:/x",
            "https://auth.example.org:abc/x",
            "https://auth.example.org:99999/x",
            "https://auth.example.org:80:80/x",
            "https://[::1/x",
            "https://[::1]x/x",
            "https://[g::1]/x",
            "https://[]/x",
            "https://a..b/x",
            "https://auth.example.org./x",
            "http://localhost./x",
            "https://-bad.example.org/x",
            "https://auth.example.org%2f.evil.com/x",
            "https://auth example.org/x",
            "https://auth.example.org/x y",
            "https://auth.example.org/\nx",
            "https://auth.example.org/\tx",
            "https://auth.example.org\\@evil.com/x",
            "https://auth.example.org\\.evil.com/x",
            "https://auth.example.org/\u{e9}",
            "https://\u{e9}.example.org/x",
            " https://auth.example.org/x",
            "https://auth.example.org/x\0",
        ] {
            refused(url, UrlRefused::Malformed);
        }
    }

    #[test]
    fn refusals_and_wrappers_never_show_the_url() {
        let secret = "https://user:pw@auth.example.org/a?state=secret-state-x";
        let err = check_browser_url(secret).expect_err("userinfo");
        assert!(!format!("{err} {err:?}").contains("secret-state-x"));
        let good =
            check_browser_url("https://auth.example.org/a?state=secret-state-x").expect("allowed");
        assert!(!format!("{good:?}").contains("secret-state-x"));
        assert!(good.as_str().contains("secret-state-x"));
    }

    #[test]
    fn login_form_debug_hides_the_password() {
        let mut form = LoginForm::new();
        form.username = "alice".into();
        form.password = Password::from("hunter2-x");
        let dbg = format!("{form:?}");
        assert!(!dbg.contains("hunter2-x"), "{dbg}");
        assert!(dbg.contains("alice"));
    }

    #[test]
    fn the_focus_order_is_user_password_log_in_browser_disclosure_then_server() {
        use LoginStop::*;
        assert_eq!(
            focus_order(false),
            [User, Password, LogIn, Browser, Disclosure]
        );
        assert_eq!(
            focus_order(true),
            [User, Password, LogIn, Browser, Disclosure, Server]
        );
    }

    #[test]
    fn only_the_text_inputs_take_tab_in_this_iced() {
        use LoginStop::*;
        let tabbed: Vec<_> = focus_order(true)
            .into_iter()
            .filter(|s| s.is_input())
            .collect();
        assert_eq!(tabbed, [User, Password, Server]);
    }

    #[test]
    fn the_server_field_opens_on_its_own_for_a_server_or_a_server_notice() {
        assert!(!server_field_open(false, "  ", None));
        assert!(!server_field_open(false, "", Some("Your session expired.")));
        assert!(server_field_open(true, "", None));
        assert!(server_field_open(false, "matrix.example.org", None));
        for notice in [
            "Enter a server name to sign in with the browser.",
            "Sign-in failed: network error: dns error",
            "Sign-in stopped: the sign-in address is malformed.",
            "login failed: the homeserver is not reachable",
        ] {
            assert!(notice_is_about_server(notice), "{notice}");
            assert!(server_field_open(false, "", Some(notice)));
        }
        for notice in ["login failed: wrong password", "You were logged out."] {
            assert!(!notice_is_about_server(notice), "{notice}");
        }
    }

    #[test]
    fn the_server_echo_names_the_host_that_is_contacted() {
        assert!(server_echo("  ").starts_with("Leave it empty"));
        assert_eq!(
            server_echo("matrix.org"),
            "Signs in with the server of matrix.org"
        );
        assert_eq!(
            server_echo(" matrix.org "),
            "Signs in with the server of matrix.org"
        );
        assert_eq!(
            server_echo("https://matrix.example:8448"),
            "Signs in with the server of https://matrix.example:8448"
        );
        // The path is not part of what is contacted first; it is not echoed.
        assert_eq!(
            server_echo("HTTPS://Matrix.Example/x?y=1#z"),
            "Signs in with the server of https://Matrix.Example"
        );
        assert_eq!(
            server_echo("http://localhost:8008"),
            "Signs in with the server of http://localhost:8008"
        );
    }

    #[test]
    fn a_long_host_is_cut_in_the_middle_and_its_last_label_stays_visible() {
        let long = format!("https://{}.example.org:8448", "a".repeat(400));
        let echo = server_echo(&long);
        assert!(
            echo.starts_with("Signs in with the server of https://aaa"),
            "{echo}"
        );
        assert!(echo.ends_with(".example.org:8448"), "{echo}");
        assert!(echo.contains('\u{2026}'), "{echo}");
        assert!(echo.chars().count() < ECHO_BUDGET + 40, "{echo}");
        // A short one is whole.
        assert!(!server_echo("https://a.example.org").contains('\u{2026}'));
    }

    #[test]
    fn hidden_characters_in_the_input_are_refused_not_echoed() {
        for input in [
            "mat\u{200b}rix.\u{202e}org",
            "https://mat\u{2066}rix.org\u{feff}:8448/x",
        ] {
            assert_eq!(classify_server(input), ServerInput::Hidden, "{input:?}");
            assert_eq!(server_echo(input), HIDDEN_SERVER, "{input:?}");
        }
    }

    #[test]
    fn an_address_with_userinfo_is_hidden_not_echoed() {
        let long = format!("https://matrix.org{}@evil.example", "a".repeat(300));
        for input in [
            long.as_str(),
            "https://matrix.org@evil.example",
            "https://user:pw@evil.example",
            "https://@evil.example",
            "http://localhost@evil.example/x",
            "HTTPS://matrix.org@evil.example",
            "matrix.org@evil.example",
            "user@evil.example",
            "https://matrix.org\\@evil.example",
            "https://evil.example\\@matrix.org",
            "https://matrix.org%20@evil.example",
            "https://matrix.org @evil.example",
            "https://matrix.org\t@evil.example",
            "https://matrix.org\u{a0}@evil.example",
            "https:matrix.org@evil.example",
            "https:/matrix.org@evil.example",
            "https:\\\\matrix.org@evil.example",
            "https:///matrix.org@evil.example",
        ] {
            assert_eq!(classify_server(input), ServerInput::Hidden, "{input:?}");
            let echo = server_echo(input);
            assert_eq!(echo, HIDDEN_SERVER, "{input:?}");
            assert!(!echo.contains("Signs in with the server of"));
        }
        // Look-alikes the URL parser maps to another host: always refused
        // (never echoed as a destination), with the notice that fits.
        for input in LOOKALIKES {
            let class = classify_server(input);
            assert!(
                matches!(class, ServerInput::Hidden | ServerInput::Unsupported),
                "{input:?}: {class:?}"
            );
            assert!(server_refusal(input).is_some(), "{input:?}");
            assert!(!server_echo(input).contains("Signs in with"), "{input:?}");
        }
        // Percent signs, look-alike numbers and other schemes hide a host;
        // the rest of the list is plain characters outside the allow-list.
        for input in [
            "https://matrix.org%2eevil.com",
            "matrix.org%2eevil.com",
            "https://0x7f.1",
            "0x7f.1",
            "2130706433",
            "http://127.1",
            "ftp://evil",
            "https://[::1]x:80",
            "matrix.org:80x",
        ] {
            assert_eq!(classify_server(input), ServerInput::Hidden, "{input:?}");
        }
        for input in [
            "https://m\u{430}trix.org",
            "m\u{430}trix.org",
            "https://matrix.org\u{2044}login.evil.com",
            "https://matrix.org\u{2215}x.evil.com",
            "https://matrix.org\u{a789}8448",
        ] {
            assert_eq!(
                classify_server(input),
                ServerInput::Unsupported,
                "{input:?}"
            );
        }
        // '@' after the authority is path, query or fragment, not userinfo.
        assert!(matches!(
            classify_server("https://matrix.org/@user"),
            ServerInput::Target(_)
        ));
        assert!(matches!(
            classify_server("https://matrix.org?a=b@c"),
            ServerInput::Target(_)
        ));
        assert!(matches!(
            classify_server("https://matrix.org#@x"),
            ServerInput::Target(_)
        ));
    }

    /// Typed addresses whose contacted host differs from what is shown.
    pub(crate) const LOOKALIKES: [&str; 14] = [
        "https://m\u{430}trix.org",
        "https://matrix.org\u{2044}login.evil.com",
        "https://matrix.org\u{2215}x.evil.com",
        "https://matrix.org%2eevil.com",
        "https://matrix.org\u{a789}8448",
        "https://0x7f.1",
        "ftp://evil",
        "m\u{430}trix.org",
        "matrix.org%2eevil.com",
        "0x7f.1",
        "2130706433",
        "http://127.1",
        "https://[::1]x:80",
        "matrix.org:80x",
    ];

    #[test]
    fn honest_names_the_allow_list_refuses_get_an_accurate_notice_not_hides_its_server() {
        // A trailing dot is the same name: exactly one is taken off.
        assert_eq!(
            classify_server("matrix.org."),
            ServerInput::Target("matrix.org".into())
        );
        assert_eq!(
            classify_server("https://matrix.org.:8448/x"),
            ServerInput::Target("https://matrix.org:8448".into())
        );
        assert_eq!(
            server_to_send(" matrix.org. ").as_deref(),
            Some("matrix.org")
        );
        assert_eq!(
            server_to_send("https://matrix.org.:8448/x").as_deref(),
            Some("https://matrix.org:8448/x")
        );
        assert_eq!(server_to_send("  "), None);
        assert_eq!(classify_server("matrix.org.."), ServerInput::Unsupported);
        assert_eq!(classify_server("."), ServerInput::Unsupported);
        // An underscore and an international name are refused, and say why.
        for input in [
            "my_synapse",
            "my_synapse.example",
            "b\u{fc}cher.de",
            "https://b\u{fc}cher.de",
        ] {
            assert_eq!(
                classify_server(input),
                ServerInput::Unsupported,
                "{input:?}"
            );
            assert_eq!(server_refusal(input), Some(UNSUPPORTED_SERVER), "{input:?}");
            assert_eq!(server_echo(input), UNSUPPORTED_SERVER);
            assert!(!UNSUPPORTED_SERVER.contains("hides"));
        }
        // The written-out form of the same name is fine.
        assert_eq!(
            classify_server("xn--bcher-kva.de"),
            ServerInput::Target("xn--bcher-kva.de".into())
        );
        // "Hides its real server" stays for the tricks.
        for input in [
            "matrix.org@evil.example",
            "matrix.org\\evil.example",
            "matrix.org%2eevil.com",
            "matrix .org",
            "mat\u{200b}rix.org",
            "ftp://matrix.org",
        ] {
            assert_eq!(server_refusal(input), Some(HIDDEN_SERVER), "{input:?}");
        }
    }

    #[test]
    fn plain_http_is_for_this_computer_only() {
        for input in [
            "http://matrix.org",
            "http://localhost.evil.com",
            "http://192.168.0.5:8008",
            "http://[2001:db8::1]",
            "HTTP://example.org",
        ] {
            assert_eq!(classify_server(input), ServerInput::Insecure, "{input:?}");
            assert_eq!(server_refusal(input), Some(INSECURE_SERVER), "{input:?}");
        }
        for input in [
            "http://localhost:8008",
            "http://LOCALHOST",
            "http://127.0.0.1",
            "http://127.1.2.3:8008",
            "http://[::1]:8008",
            "https://matrix.org",
        ] {
            assert!(
                matches!(classify_server(input), ServerInput::Target(_)),
                "{input:?}"
            );
        }
    }

    #[test]
    fn plain_ascii_servers_and_the_dev_homeserver_stay_targets() {
        for (input, target) in [
            ("matrix.org", "matrix.org"),
            ("matrix.org:8448", "matrix.org:8448"),
            (
                "https://matrix.example:8448/",
                "https://matrix.example:8448",
            ),
            ("[::1]:8008", "[::1]:8008"),
            ("http://localhost:8008", "http://localhost:8008"),
            ("localhost:8008", "localhost:8008"),
            ("127.0.0.1:8008", "127.0.0.1:8008"),
            ("http://127.0.0.1:8008", "http://127.0.0.1:8008"),
        ] {
            assert_eq!(
                classify_server(input),
                ServerInput::Target(target.into()),
                "{input:?}"
            );
        }
        assert_eq!(
            server_echo("matrix.org"),
            "Signs in with the server of matrix.org"
        );
    }

    #[test]
    fn classify_server_trims_and_tells_empty_from_target() {
        assert_eq!(classify_server("   "), ServerInput::Empty);
        assert_eq!(
            classify_server(" matrix.org "),
            ServerInput::Target("matrix.org".into())
        );
        assert_eq!(
            classify_server("https://matrix.example:8448/"),
            ServerInput::Target("https://matrix.example:8448".into())
        );
    }

    #[test]
    fn a_held_field_is_one_with_text_or_a_server_notice() {
        assert!(!server_held("", None));
        assert!(!server_held("  ", Some("You were logged out.")));
        assert!(server_held("matrix.org", None));
        assert!(server_held(
            "",
            Some("login failed: the homeserver is not reachable")
        ));
        assert!(server_held("", Some(HIDDEN_SERVER)));
    }

    #[test]
    fn enter_submits_only_a_complete_idle_form() {
        assert!(can_submit("alice", "pw", false));
        assert!(!can_submit("alice", "pw", true));
        assert!(!can_submit("  ", "pw", false));
        assert!(!can_submit("alice", "", false));
    }
}
