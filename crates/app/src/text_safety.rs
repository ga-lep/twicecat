//! Sanitising of text that comes from other people (message bodies, room
//! names, display names) before it is drawn on one line.
//!
//! Line breaks could fake another sender's line, and bidi controls or
//! zero-width characters could reorder or hide text. Every remote string goes
//! through [`one_line`] or [`one_line_capped`] before it reaches a widget.

/// Shown in place of a line break.
pub const LINE_BREAK_MARK: char = '\u{23CE}';

/// Shown in place of a control or bidi character.
pub const REPLACEMENT: char = '\u{FFFD}';

/// Shown at the end of a capped string.
pub const ELLIPSIS: char = '\u{2026}';

/// How many characters a room name or sender name may show.
pub const MAX_NAME_CHARS: usize = 256;

/// `text` on one line: line breaks become [`LINE_BREAK_MARK`]; bidi controls
/// and other control characters become [`REPLACEMENT`]; zero-width and other
/// invisible format characters are removed.
///
/// ```
/// assert_eq!(tessera_app::text_safety::one_line("a\nb"), "a\u{23CE}b");
/// ```
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => out.push(LINE_BREAK_MARK),
            '\t' => out.push(' '),
            // Bidi controls: ALM, LRM, RLM, embeddings/overrides, isolates.
            '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => out.push(REPLACEMENT),
            // Invisible: zero-width space/joiners, soft hyphen, word joiner
            // and invisible operators, BOM.
            '\u{200B}'..='\u{200D}' | '\u{00AD}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' => {}
            c if c.is_control() => out.push(REPLACEMENT),
            c => out.push(c),
        }
    }
    out
}

/// [`one_line`], then at most `max_chars` characters (ending in [`ELLIPSIS`]
/// when it was cut), always on a character boundary.
pub fn one_line_capped(text: &str, max_chars: usize) -> String {
    let clean = one_line(text);
    if clean.chars().count() <= max_chars {
        return clean;
    }
    let Some(keep) = max_chars.checked_sub(1) else {
        return String::new();
    };
    let mut cut: String = clean.chars().take(keep).collect();
    cut.push(ELLIPSIS);
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(
            one_line("hello wörld \u{1F600} 你好"),
            "hello wörld \u{1F600} 你好"
        );
        assert_eq!(one_line(""), "");
    }

    #[test]
    fn line_breaks_become_a_visible_mark() {
        for br in ['\n', '\r', '\u{2028}', '\u{2029}', '\u{85}'] {
            let out = one_line(&format!("a{br}@mallory: hi"));
            assert_eq!(out, format!("a{LINE_BREAK_MARK}@mallory: hi"), "{br:?}");
        }
        assert_eq!(one_line("a\r\nb"), "a\u{23CE}\u{23CE}b");
    }

    #[test]
    fn bidi_controls_are_replaced() {
        for c in ('\u{202A}'..='\u{202E}')
            .chain('\u{2066}'..='\u{2069}')
            .chain(['\u{200E}', '\u{200F}', '\u{061C}'])
        {
            assert_eq!(
                one_line(&format!("a{c}b")),
                format!("a{REPLACEMENT}b"),
                "{c:?}"
            );
        }
    }

    #[test]
    fn zero_width_and_format_characters_are_removed() {
        for c in [
            '\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}', '\u{00AD}', '\u{2060}',
        ] {
            assert_eq!(one_line(&format!("a{c}b")), "ab", "{c:?}");
        }
    }

    #[test]
    fn other_control_characters_are_replaced() {
        for c in ['\0', '\u{7}', '\u{1B}', '\u{7F}', '\u{9F}'] {
            assert_eq!(
                one_line(&format!("a{c}b")),
                format!("a{REPLACEMENT}b"),
                "{c:?}"
            );
        }
    }

    #[test]
    fn tab_becomes_a_space() {
        assert_eq!(one_line("a\tb"), "a b");
    }

    #[test]
    fn output_has_no_line_or_bidi_characters() {
        let nasty = "x\n\r\u{2028}\u{2029}\u{202E}\u{2066}\u{200B}\u{0}y";
        let out = one_line(nasty);
        assert!(out.chars().all(|c| !c.is_control()));
        assert!(!out.contains(['\u{202E}', '\u{2066}', '\u{200B}', '\u{2028}']));
    }

    #[test]
    fn capping_cuts_on_a_char_boundary_and_adds_an_ellipsis() {
        assert_eq!(one_line_capped("abc", 3), "abc");
        assert_eq!(one_line_capped("abcd", 3), "ab\u{2026}");
        let long = "é".repeat(1000);
        let out = one_line_capped(&long, MAX_NAME_CHARS);
        assert_eq!(out.chars().count(), MAX_NAME_CHARS);
        assert!(out.ends_with(ELLIPSIS));
        assert_eq!(one_line_capped("anything", 0), "");
        assert_eq!(one_line_capped("abc", 1), "\u{2026}");
    }

    #[test]
    fn capping_counts_after_sanitising() {
        let zw = "\u{200B}".repeat(500);
        assert_eq!(one_line_capped(&format!("{zw}ab"), 3), "ab");
        assert_eq!(one_line_capped("a\nb\nc", 3), "a\u{23CE}\u{2026}");
    }
}
