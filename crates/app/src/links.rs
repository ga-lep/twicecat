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

#[cfg(test)]
mod tests {
    use super::*;

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
