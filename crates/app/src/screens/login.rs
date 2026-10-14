//! The login screen, the "continue in your browser" screen, and the guard in
//! front of the browser launcher.
//!
//! The OAuth URL carries `state` and a code challenge, so it is never logged
//! or shown. Before it reaches `open`, [`check_browser_url`] re-checks it
//! (plan Q21): `https`, or `http` only to a loopback host, and no userinfo.

use std::fmt;

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Fill};
use twicecat_core::api::Password;

use crate::state::{App, Message};

/// The login form's fields.
pub struct LoginForm {
    /// Server name or URL; empty means "from the user ID".
    pub server: String,
    /// Localpart or full MXID.
    pub username: String,
    /// The password; cleared on submit and on failure (plan Q19).
    pub password: Password,
}

impl LoginForm {
    /// An empty form.
    pub fn new() -> Self {
        Self {
            server: String::new(),
            username: String::new(),
            password: Password::from(String::new()),
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

/// Width of the form column.
const FORM_WIDTH: f32 = 380.0;

/// The login form.
pub fn view(app: &App) -> Element<'_, Message> {
    let form = &app.login;
    let busy = app.login_in_flight;
    let submit = (!busy).then_some(Message::LoginSubmitted);
    let browser = (!busy && !form.server.trim().is_empty()).then_some(Message::OAuthRequested);

    let mut fields = column![text("Twicecat").size(32)]
        .spacing(12)
        .max_width(FORM_WIDTH);
    if let Some(notice) = &app.notice {
        fields = fields.push(text(notice.as_str()).style(text::danger));
    }
    fields = fields
        .push(
            text_input("Server (optional; taken from your user ID)", &form.server)
                .on_input_maybe((!busy).then_some(Message::ServerChanged))
                .on_submit_maybe(submit.clone())
                .padding(8),
        )
        .push(
            text_input("User name or @user:server", &form.username)
                .on_input_maybe((!busy).then_some(Message::UsernameChanged))
                .on_submit_maybe(submit.clone())
                .padding(8),
        )
        .push(
            text_input("Password", form.password.expose())
                .secure(true)
                .on_input_maybe(
                    (!busy).then_some(|s: String| Message::PasswordChanged(Password::from(s))),
                )
                .on_submit_maybe(submit.clone())
                .padding(8),
        )
        .push(
            button(text("Log in").center().width(Fill))
                .on_press_maybe(submit)
                .width(Fill)
                .padding(8),
        )
        .push(
            button(text("Sign in with browser").center().width(Fill))
                .on_press_maybe(browser)
                .style(button::secondary)
                .width(Fill)
                .padding(8),
        );
    if busy {
        fields = fields.push(text("Signing in\u{2026}"));
    }
    container(fields).center_x(Fill).center_y(Fill).into()
}

/// "Continue in your browser", with **Cancel**, and **Copy link** when the
/// browser could not be started. The URL itself is never shown.
pub fn waiting_view(app: &App) -> Element<'_, Message> {
    let status = match app.browser_opened {
        None => "Opening your browser\u{2026}",
        Some(true) => "Finish signing in in the browser window, then come back here.",
        Some(false) => "The browser could not be started. Copy the link and open it yourself.",
    };
    let mut buttons = row![].spacing(12);
    if app.browser_opened == Some(false) && app.browser_link.is_some() {
        buttons = buttons.push(
            button(text("Copy link"))
                .on_press(Message::CopyLink)
                .padding(8),
        );
    }
    buttons = buttons.push(
        button(text("Cancel"))
            .on_press(Message::CancelOAuth)
            .style(button::secondary)
            .padding(8),
    );
    let content = column![
        text("Continue in your browser").size(24),
        text(status),
        buttons
    ]
    .spacing(12)
    .max_width(FORM_WIDTH);
    container(content).center_x(Fill).center_y(Fill).into()
}

#[cfg(test)]
mod tests {
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
}
