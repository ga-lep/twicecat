//! Emoji shortcode lookups for the composer (plan Q18), on the `emojis` crate.

/// Most suggestions shown at once.
pub const MAX_SUGGESTIONS: usize = 8;

/// One emoji offered for a typed shortcode prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    /// The emoji itself.
    pub glyph: &'static str,
    /// The shortcode that matched, without colons.
    pub shortcode: &'static str,
}

/// Whether `c` may appear in a shortcode.
pub fn is_shortcode_char(c: char) -> bool {
    matches!(c, 'a'..='z' | '0'..='9' | '_' | '+' | '-')
}

/// At most [`MAX_SUGGESTIONS`] emojis one of whose shortcodes starts with
/// `query`, an exact shortcode first, then in CLDR order. Empty for an empty
/// query or one with a character outside `[a-z0-9_+-]`.
pub fn suggest(query: &str) -> Vec<Suggestion> {
    if query.is_empty() || !query.chars().all(is_shortcode_char) {
        return Vec::new();
    }
    let mut found: Vec<Suggestion> = Vec::with_capacity(MAX_SUGGESTIONS);
    // The exact match leads, whatever its place in the CLDR order.
    if let Some(emoji) = emojis::get_by_shortcode(query)
        && let Some(shortcode) = emoji.shortcodes().find(|code| *code == query)
    {
        found.push(Suggestion {
            glyph: emoji.as_str(),
            shortcode,
        });
    }
    for emoji in emojis::iter() {
        if found.len() == MAX_SUGGESTIONS {
            break;
        }
        if found.iter().any(|s| s.glyph == emoji.as_str()) {
            continue;
        }
        if let Some(shortcode) = emoji.shortcodes().find(|code| code.starts_with(query)) {
            found.push(Suggestion {
                glyph: emoji.as_str(),
                shortcode,
            });
        }
    }
    found
}

/// The emoji for a complete `:shortcode:`.
pub fn exact(token: &str) -> Option<&'static str> {
    let code = token.strip_prefix(':')?.strip_suffix(':')?;
    if code.is_empty() || !code.chars().all(is_shortcode_char) {
        return None;
    }
    emojis::get_by_shortcode(code).map(emojis::Emoji::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_finds_the_emoji_by_shortcode() {
        let found = suggest("smi");
        assert!(found.len() <= MAX_SUGGESTIONS);
        assert!(
            found
                .iter()
                .any(|s| s.glyph == "\u{1F604}" && s.shortcode == "smile"),
            "{found:?}"
        );
    }

    #[test]
    fn an_exact_shortcode_comes_first_and_the_list_is_capped() {
        let found = suggest("smile");
        assert_eq!(found[0].shortcode, "smile");
        assert!(suggest("s").len() == MAX_SUGGESTIONS);
    }

    #[test]
    fn bad_queries_find_nothing() {
        assert!(suggest("").is_empty());
        assert!(suggest("SMI").is_empty());
        assert!(suggest("sm i").is_empty());
        assert!(suggest("zzzzqqqq").is_empty());
    }

    #[test]
    fn each_emoji_is_listed_once() {
        let found = suggest("la");
        let mut glyphs: Vec<_> = found.iter().map(|s| s.glyph).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), found.len());
    }

    #[test]
    fn exact_resolves_a_colon_wrapped_shortcode() {
        assert_eq!(exact(":smile:"), Some("\u{1F604}"));
        assert_eq!(exact(":rocket:"), Some("\u{1F680}"));
        assert_eq!(exact("smile"), None);
        assert_eq!(exact(":smile"), None);
        assert_eq!(exact(":nope_nope:"), None);
        assert_eq!(exact("::"), None);
    }
}
