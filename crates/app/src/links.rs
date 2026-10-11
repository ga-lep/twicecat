//! Link safety for the timeline (plan Q19): an app-side check of every URL
//! before it is opened. Every link is confirmed by the user first.
//!
//! Nothing here logs or `Debug`-prints a URL.

use std::fmt;

use crate::screens::login::host_of;

/// Longest URL accepted, in bytes (the core's limit too).
pub const MAX_URL_BYTES: usize = 2048;

/// The kind of a checked link, which decides what the confirmation says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `https`.
    Https,
    /// Plain `http`, not encrypted.
    Http,
    /// `mailto`, which starts an email draft.
    Mailto,
}

/// A URL that passed [`check_link_url`]. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedUrl {
    url: String,
    host: String,
    kind: LinkKind,
}

impl CheckedUrl {
    /// The URL to open. For `mailto` it is the bare address: the query part
    /// (`cc`, `bcc`, `attach`, `body`) is dropped.
    pub fn as_str(&self) -> &str {
        &self.url
    }

    /// The host (for `mailto`, the domain of the address), lower case and
    /// without port. This is what the confirmation bar shows.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether this is an https, http or mailto link.
    pub fn kind(&self) -> LinkKind {
        self.kind
    }
}

// Manual impl: a URL can carry secrets and is user content.
impl fmt::Debug for CheckedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CheckedUrl(<redacted>)")
    }
}

/// Why a link was not allowed to open. Never carries the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRefused {
    /// Too long, not plain ASCII, or has spaces, controls or backslashes.
    Malformed,
    /// The scheme is not `https`, `http` or `mailto`.
    Scheme,
    /// The URL has a user name or password part.
    Userinfo,
}

/// Checks `url` for opening: `https`, `http` or `mailto` only, ASCII only
/// with no spaces, control characters or backslashes, no userinfo, a plain
/// DNS, IPv4 or bracketed IPv6 host, at most [`MAX_URL_BYTES`] bytes.
///
/// A `mailto` link must be one plain address; its query is dropped.
pub fn check_link_url(url: &str) -> Result<CheckedUrl, LinkRefused> {
    if url.len() > MAX_URL_BYTES
        || !url
            .chars()
            .all(|c| c.is_ascii() && !c.is_ascii_control() && c != ' ' && c != '\\')
    {
        return Err(LinkRefused::Malformed);
    }
    let (scheme, rest) = url.split_once(':').ok_or(LinkRefused::Scheme)?;
    if scheme.eq_ignore_ascii_case("mailto") {
        return check_mailto(rest);
    }
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err(LinkRefused::Scheme);
    }
    let rest = rest.strip_prefix("//").ok_or(LinkRefused::Malformed)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(LinkRefused::Userinfo);
    }
    let host = host_of(authority).map_err(|_| LinkRefused::Malformed)?;
    Ok(CheckedUrl {
        url: url.to_owned(),
        host: host.to_ascii_lowercase(),
        kind: if scheme.eq_ignore_ascii_case("http") {
            LinkKind::Http
        } else {
            LinkKind::Https
        },
    })
}

/// `rest` is what follows `mailto:`. One plain address, headers dropped.
fn check_mailto(rest: &str) -> Result<CheckedUrl, LinkRefused> {
    let address = rest.split(['?', '#']).next().unwrap_or_default();
    let (local, domain) = address.split_once('@').ok_or(LinkRefused::Malformed)?;
    let local_ok = !local.is_empty()
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$&'*+/=^_`{|}~-".contains(&b));
    let domain_ok = domain.split('.').all(is_dns_label_like);
    if !local_ok || !domain_ok {
        return Err(LinkRefused::Malformed);
    }
    Ok(CheckedUrl {
        url: format!("mailto:{address}"),
        host: domain.to_ascii_lowercase(),
        kind: LinkKind::Mailto,
    })
}

/// A non-empty run of letters, digits and inner hyphens.
fn is_dns_label_like(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// What the bar above the composer says about a clicked link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkBar {
    /// Every link asks before opening, naming the host (or mail domain).
    Confirm(CheckedUrl),
    /// The link failed the safety check and was not opened.
    Refused,
    /// The system handler could not be started.
    Failed,
}

/// Opens a checked URL with the system handler.
///
/// This starts a process, so it must only run inside a `Task`, never in
/// `update`. The error is dropped on purpose: it cannot help the user and
/// must not leak the URL.
pub fn open_link(url: &CheckedUrl) -> bool {
    open::that_detached(url.as_str()).is_ok()
}

/// Longest room target text, in bytes (the core's limit too).
pub const MAX_TARGET_BYTES: usize = 512;
/// Most `via` servers kept from a link (the core's limit too).
pub const MAX_VIA: usize = 8;

/// A room to join: the room ID or alias, and servers that can route a join.
#[derive(Clone, PartialEq, Eq)]
pub struct RoomTarget {
    /// `!id:server`, `!id` (room version 12) or `#alias:server`.
    pub id: String,
    /// Servers from a `matrix.to` link.
    pub via: Vec<String>,
}

// Manual impl: aliases and IDs are user content.
impl fmt::Debug for RoomTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RoomTarget(<redacted>, {} via)", self.via.len())
    }
}

/// Whether `name` is a Matrix server name: a DNS name, an IPv4 address or a
/// bracketed IPv6 address, with an optional port.
pub fn valid_server_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    let (host, port) = if let Some(rest) = name.strip_prefix('[') {
        let Some((inner, after)) = rest.split_once(']') else {
            return false;
        };
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        (None, after)
    } else {
        match name.find(':') {
            Some(at) => (Some(&name[..at]), &name[at..]),
            None => (Some(name), ""),
        }
    };
    let port_ok = match port {
        "" => true,
        p => p.strip_prefix(':').is_some_and(|digits| {
            (1..=5).contains(&digits.len())
                && digits.bytes().all(|b| b.is_ascii_digit())
                && digits.parse::<u16>().is_ok_and(|n| n > 0)
        }),
    };
    let host_ok = match host {
        // A bracketed IPv6 address was checked above.
        None => true,
        Some(host) if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') => {
            host.parse::<std::net::Ipv4Addr>().is_ok()
        }
        Some(host) => {
            !host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        }
    };
    port_ok && host_ok
}

/// Parses a user ID, `@localpart:server`, as the core does.
pub fn parse_user_id(text: &str) -> Option<twicecat_core::api::UserId> {
    let text = text.trim();
    let rest = text.strip_prefix('@')?;
    if text.len() > MAX_USER_ID_BYTES {
        return None;
    }
    let (local, server) = rest.split_once(':')?;
    // Historical user IDs allow any visible ASCII; spaces, controls and
    // look-alike or bidirectional characters never.
    let local_ok = !local.is_empty() && local.bytes().all(|b| (0x21..=0x7E).contains(&b));
    (local_ok && valid_server_name(server)).then(|| twicecat_core::api::UserId::from(text))
}

/// Parses `#alias:server`, `!id:server` or a `https://matrix.to/#/...` link
/// to a room or alias, with the same table as the core's `parse_room_target`
/// (the app checks before it sends; the core checks again). Nothing is
/// fetched. Anything else (users, events, other hosts, schemes or
/// arguments) is refused.
pub fn parse_room_target(text: &str) -> Option<RoomTarget> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_TARGET_BYTES || text.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(rest) = text.strip_prefix(MATRIX_TO_PREFIX) {
        let (id_part, query) = match rest.split_once('?') {
            Some((id, query)) => (id, query),
            None => (rest, ""),
        };
        let id = percent_decode(id_part)?;
        if !valid_room_id_or_alias(&id) {
            return None;
        }
        let mut via = Vec::new();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let value = percent_decode(pair.strip_prefix("via=")?)?;
            if !valid_server_name(&value) {
                return None;
            }
            if via.len() < MAX_VIA && !via.contains(&value) {
                via.push(value);
            }
        }
        return Some(RoomTarget { id, via });
    }
    if text.contains("//") || !valid_room_id_or_alias(text) {
        return None;
    }
    Some(RoomTarget {
        id: text.to_owned(),
        via: Vec::new(),
    })
}

const MATRIX_TO_PREFIX: &str = "https://matrix.to/#/";
/// Longest user ID, in bytes (the Matrix limit).
const MAX_USER_ID_BYTES: usize = 255;

/// `!id:server`, `!id` or `#alias:server`: a sigil, something after it, and
/// for an alias a valid server name; a colon in a room ID starts a server
/// name too.
fn valid_room_id_or_alias(id: &str) -> bool {
    if id.len() > MAX_TARGET_BYTES || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let (sigil, rest) = match id.split_at_checked(1) {
        Some((sigil @ ("!" | "#"), rest)) => (sigil, rest),
        _ => return false,
    };
    if rest.is_empty() || rest.contains('/') {
        return false;
    }
    match rest.split_once(':') {
        Some((local, server)) => !local.is_empty() && valid_server_name(server),
        // A room ID of room version 12 has no server part; an alias needs one.
        None => sigil == "!",
    }
}

/// Decodes `%XX` escapes; `None` for a broken escape or non-UTF-8 bytes.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_targets_are_aliases_ids_and_matrix_to_links_only() {
        let ok = |text: &str| parse_room_target(text).unwrap_or_else(|| panic!("{text:?}"));
        assert_eq!(ok("#lounge:localhost").id, "#lounge:localhost");
        assert_eq!(ok("  !abc:localhost ").id, "!abc:localhost");
        assert!(ok("#lounge:localhost").via.is_empty());
        // Room versions 12 and later have IDs without a server part.
        assert_eq!(ok("!abc").id, "!abc");
        assert_eq!(ok("#a:[::1]:8448").id, "#a:[::1]:8448");

        let link = ok("https://matrix.to/#/!abc:localhost?via=a.example&via=b.example");
        assert_eq!(link.id, "!abc:localhost");
        assert_eq!(link.via, ["a.example", "b.example"]);
        assert_eq!(
            ok("https://matrix.to/#/%23lounge:localhost").id,
            "#lounge:localhost"
        );
        assert_eq!(
            ok("https://matrix.to/#/#lounge:localhost").id,
            "#lounge:localhost"
        );
        let many: String = (0..20).map(|i| format!("&via=s{i}.example")).collect();
        let long = ok(&format!(
            "https://matrix.to/#/!abc:localhost?{}",
            &many[1..]
        ));
        assert_eq!(long.via.len(), MAX_VIA);
    }

    #[test]
    fn junk_is_not_a_room_target() {
        for bad in [
            "",
            "   ",
            "lounge",
            "lounge:localhost",
            "#lounge",
            "!",
            "#",
            "@bob:localhost",
            "$event:localhost",
            "#a b:localhost",
            "#a:localhost extra",
            "#a:bad host",
            "#a:",
            "#a:host:99999",
            "http://matrix.to/#/!abc:localhost",
            "https://evil.example/#/!abc:localhost",
            "https://matrix.to.evil.example/#/!abc:localhost",
            "https://matrix.to/#/@bob:localhost",
            "https://matrix.to/#/!abc:localhost/$event:localhost",
            "https://matrix.to/#/!abc:localhost?via=not a server",
            "https://matrix.to/#/!abc:localhost?other=1",
            "matrix:roomid/abc:localhost",
            "https://example.com/room",
            &format!("#{}:localhost", "a".repeat(600)),
        ] {
            assert_eq!(parse_room_target(bad), None, "{bad:?}");
        }
        // Surrounding whitespace, a trailing newline included, is trimmed.
        assert!(parse_room_target("#a:localhost\n").is_some());
    }

    #[test]
    fn user_ids_need_a_local_part_and_a_server() {
        for good in [
            "@bob:localhost",
            " @bob:example.org ",
            "@a.b_c=d-e:host:8448",
        ] {
            assert!(parse_user_id(good).is_some(), "{good:?}");
        }
        assert_eq!(
            parse_user_id(" @bob:localhost ").map(|u| u.as_str().to_owned()),
            Some("@bob:localhost".to_owned())
        );
        for bad in [
            "",
            "@",
            "@:localhost",
            "@bob",
            "@bob:",
            "bob:localhost",
            "@bob:local host",
            "@bo b:localhost",
            "@bob:localhost\n@eve:localhost",
            "@bob\u{202E}:localhost",
            "!room:localhost",
            "@bob:host:99999",
            &format!("@{}:localhost", "a".repeat(300)),
        ] {
            assert!(parse_user_id(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn server_names_are_dns_names_or_addresses_with_a_port() {
        for good in [
            "localhost",
            "example.org",
            "example.org:8448",
            "127.0.0.1",
            "127.0.0.1:8008",
            "[::1]",
            "[2001:db8::1]:8448",
            "a-b.example",
        ] {
            assert!(valid_server_name(good), "{good:?}");
        }
        for bad in [
            "",
            ":8448",
            "example.org:",
            "example.org:0x10",
            "example.org:70000",
            "a b",
            "[::1",
            "[zz]",
            "999.1.1.1",
            "exa_mple.org",
            "example.org/",
            &"a".repeat(300),
        ] {
            assert!(!valid_server_name(bad), "{bad:?}");
        }
    }

    fn ok(url: &str) -> CheckedUrl {
        check_link_url(url).unwrap_or_else(|e| panic!("{url:?} refused: {e:?}"))
    }

    #[test]
    fn web_and_mail_links_pass() {
        assert_eq!(
            ok("https://example.org/a?b=c#d").as_str(),
            "https://example.org/a?b=c#d"
        );
        assert_eq!(ok("HTTP://Example.org:8080/x").host(), "example.org");
        assert_eq!(ok("https://[::1]:8448/").host(), "[::1]");
        assert_eq!(ok("mailto:alice@example.org").host(), "example.org");
    }

    #[test]
    fn mailto_drops_cc_bcc_and_every_other_header() {
        let url = ok("mailto:alice@example.org?cc=a@b.c&bcc=x@y.z&attach=/etc/passwd&body=hi");
        assert_eq!(url.as_str(), "mailto:alice@example.org");
        assert_eq!(ok("mailto:a@b.co#frag").as_str(), "mailto:a@b.co");
    }

    #[test]
    fn mailto_must_be_one_plain_address() {
        for bad in [
            "mailto:",
            "mailto:?to=a@b.c",
            "mailto:a@b.c,d@e.f",
            "mailto:a@b.c%2Cd@e.f",
            "mailto:a@@b.c",
            "mailto:nodomain",
            "mailto:a@",
            "mailto:@b.c",
            "mailto:a@b..c",
        ] {
            assert!(check_link_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn dangerous_schemes_are_refused() {
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "file:///etc/passwd",
            "data:text/html;base64,AAAA",
            "ftp://example.org/",
            "matrix:r/room:example.org",
            "vbscript:x",
            "//example.org/",
            "example.org",
            "",
        ] {
            assert!(check_link_url(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            check_link_url("javascript:alert(1)"),
            Err(LinkRefused::Scheme)
        );
    }

    #[test]
    fn userinfo_is_refused() {
        assert_eq!(
            check_link_url("https://good.example@evil.example/"),
            Err(LinkRefused::Userinfo)
        );
        assert_eq!(
            check_link_url("https://user:pw@evil.example/"),
            Err(LinkRefused::Userinfo)
        );
    }

    #[test]
    fn spaces_controls_backslashes_and_non_ascii_are_refused() {
        for bad in [
            "https://exa mple.org/",
            "https://example.org/a b",
            "https://example.org/\n",
            "https://example.org/\u{0}",
            "https://example.org/\u{7f}",
            "https://example.org\\@evil.example/",
            "https://exämple.org/",
            "https://example.org/\u{202E}",
            "https://example.org/é",
            "https://\u{FF45}xample.org/",
            "https://",
            "https:///path",
            "https://exa_mple.org/",
            "https://example.org:99999/",
        ] {
            assert!(check_link_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_length_is_capped() {
        let long = format!("https://example.org/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!(check_link_url(&long), Err(LinkRefused::Malformed));
        let edge = format!("https://example.org/{}", "a".repeat(MAX_URL_BYTES - 20));
        assert!(check_link_url(&edge).is_ok());
    }

    #[test]
    fn checked_urls_and_refusals_never_print_the_url() {
        let url = ok("https://secret-sentinel.example/token-sentinel");
        let dbg = format!("{url:?} {:?}", check_link_url("javascript:sentinel"));
        assert!(!dbg.contains("sentinel"), "{dbg}");
    }
}
