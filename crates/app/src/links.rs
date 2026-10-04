//! Link safety for the timeline (plan Q19): an app-side check of every URL
//! before it is opened, and the "does the text lie about the target" test.
//!
//! Nothing here logs or `Debug`-prints a URL.

use std::fmt;

use crate::screens::login::host_of;

/// Longest URL accepted, in bytes (the core's limit too).
pub const MAX_URL_BYTES: usize = 2048;

/// A URL that passed [`check_link_url`]. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedUrl {
    url: String,
    host: String,
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

/// Whether a link whose visible text is `text` and whose target is `url`
/// needs the user's confirmation: the text looks like a web address with a
/// different host than the target, or like a piece of one (a split anchor
/// such as `bank<a href=..>.example</a>`) that names no host at all. Plain
/// words, `mailto` targets and equal hosts never need it.
///
/// Look-alike dots (U+3002, U+FF0E, U+FF61) read as `.` and trailing
/// `.,;:!?)` is ignored. A caller that shortens the text for display should
/// also ask about the whole text and combine the answers with "or".
pub fn needs_confirmation(text: &str, url: &str) -> bool {
    let Some(target) = web_host(url, true) else {
        return false;
    };
    let shown = normalize_text(text);
    match web_host(&shown, false) {
        Some(shown) => shown != target,
        None => is_address_fragment(&shown),
    }
}

/// `text` trimmed, with look-alike dots turned into `.` and without
/// trailing sentence punctuation.
fn normalize_text(text: &str) -> String {
    let dotted: String = text
        .trim()
        .chars()
        .map(|c| match c {
            '\u{3002}' | '\u{FF0E}' | '\u{FF61}' => '.',
            c => c,
        })
        .collect();
    dotted
        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')'])
        .to_owned()
}

/// Text without whitespace that has a dot, a slash or a scheme but no
/// parsable host: the rest of an address cut in two. Version numbers
/// (`v1.2.3`) are not addresses.
fn is_address_fragment(text: &str) -> bool {
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return false;
    }
    let digits = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let version_like =
        !digits.starts_with('.') && digits.chars().all(|c| c.is_ascii_digit() || c == '.');
    !version_like && text.contains(['.', '/', ':'])
}

/// The comparable host of something that looks like a web address.
///
/// With `scheme_required` (the target) only `http(s)://` qualifies. For
/// visible text, a bare `host.tld/...` qualifies too, and any other scheme
/// is read as the address it pretends to be. Text with whitespace never
/// does. The host is lower case, without userinfo, port and a leading
/// `www.`; non-ASCII hosts are kept as they are, so they never equal an
/// ASCII target. `text` is normalised by [`normalize_text`] first when it
/// is visible text; a trailing dot of a host is dropped on both sides.
fn web_host(text: &str, scheme_required: bool) -> Option<String> {
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let (has_scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => {
            if scheme_required
                && !scheme.eq_ignore_ascii_case("http")
                && !scheme.eq_ignore_ascii_case("https")
            {
                return None;
            }
            (true, rest)
        }
        None if scheme_required => return None,
        None => (false, text),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let host = host.to_lowercase();
    let host = host.trim_end_matches('.');
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    if !has_scheme {
        // Bare text must look like `name.tld`, or it is just words.
        let labels: Vec<&str> = host.split('.').collect();
        let tld = labels.last().copied().unwrap_or_default();
        let looks_like_host = labels.len() >= 2
            && labels.iter().all(|l| !l.is_empty())
            && tld.chars().count() >= 2
            && tld.chars().all(char::is_alphabetic);
        if !looks_like_host {
            return None;
        }
    }
    Some(host.to_owned())
}

/// What the bar above the composer says about a clicked link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkBar {
    /// The text names another host than the target; asks before opening.
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

    #[test]
    fn a_different_host_in_the_text_needs_confirmation() {
        assert!(needs_confirmation(
            "https://bank.example/login",
            "https://evil.example/login"
        ));
        assert!(needs_confirmation("bank.example", "https://evil.example/"));
        assert!(needs_confirmation(
            "  www.bank.example/path  ",
            "https://evil.example/"
        ));
        assert!(needs_confirmation(
            "https://good.example@evil.example",
            "https://good.example/"
        ));
        assert!(needs_confirmation(
            "http://bank.example",
            "https://bank.example.evil.example/"
        ));
        // A homoglyph host never equals the ASCII target.
        assert!(needs_confirmation(
            "https://b\u{0430}nk.example/",
            "https://bank.example/"
        ));
    }

    #[test]
    fn the_same_host_non_urls_and_mailto_do_not() {
        assert!(!needs_confirmation(
            "https://example.org/a",
            "https://example.org/b?x=1"
        ));
        assert!(!needs_confirmation("Example.ORG", "https://example.org/"));
        assert!(!needs_confirmation(
            "example.org:8080",
            "https://example.org:9090/"
        ));
        assert!(!needs_confirmation(
            "www.example.org",
            "https://example.org/"
        ));
        assert!(!needs_confirmation("click here", "https://evil.example/"));
        assert!(!needs_confirmation(
            "see bank.example now",
            "https://evil.example/"
        ));
        assert!(!needs_confirmation("version 1.2", "https://evil.example/"));
        assert!(!needs_confirmation("v1.2", "https://evil.example/"));
        assert!(!needs_confirmation("", "https://evil.example/"));
        assert!(!needs_confirmation(
            "alice@example.org",
            "mailto:alice@example.org"
        ));
        assert!(!needs_confirmation(
            "bank.example",
            "mailto:alice@evil.example"
        ));
        // An unparsable target is refused at click time, not here.
        assert!(!needs_confirmation("bank.example", "javascript:x"));
    }

    #[test]
    fn a_split_anchor_fragment_asks() {
        // `bank<a href="https://evil.example/">.example</a>`: the link text
        // is only the second half.
        assert!(needs_confirmation(".example", "https://evil.example/"));
        assert!(needs_confirmation("/login", "https://evil.example/"));
        assert!(needs_confirmation("https://", "https://evil.example/"));
        assert!(needs_confirmation("ftp://bank", "https://evil.example/"));
    }

    #[test]
    fn look_alike_dots_count_as_dots() {
        for dot in ['\u{3002}', '\u{FF0E}', '\u{FF61}'] {
            let shown = format!("bank{dot}example");
            assert!(needs_confirmation(&shown, "https://evil.example/"), "{dot}");
            assert!(
                !needs_confirmation(&shown, "https://bank.example/"),
                "{dot}"
            );
            let fragment = format!("{dot}example");
            assert!(needs_confirmation(&fragment, "https://evil.example/"));
        }
    }

    #[test]
    fn trailing_punctuation_does_not_hide_the_host() {
        for tail in [".", ",", ";", ":", "!", "?", ")", ").", "\u{3002}"] {
            let shown = format!("bank.example{tail}");
            assert!(
                needs_confirmation(&shown, "https://evil.example/"),
                "{tail}"
            );
            assert!(
                !needs_confirmation(&shown, "https://bank.example/"),
                "{tail}"
            );
        }
        assert!(needs_confirmation(
            "(https://bank.example/x).",
            "https://evil.example/"
        ));
    }

    #[test]
    fn plain_words_and_versions_still_do_not_ask() {
        assert!(!needs_confirmation("click here", "https://evil.example/"));
        assert!(!needs_confirmation("here", "https://evil.example/"));
        assert!(!needs_confirmation("v1.2.3", "https://evil.example/"));
        assert!(!needs_confirmation("3.14", "https://evil.example/"));
    }
}
