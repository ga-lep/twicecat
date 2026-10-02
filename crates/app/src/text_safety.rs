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

/// How many combining marks may follow one base character; more are dropped.
pub const MAX_COMBINING_MARKS: usize = 4;

/// How many characters a message body may show.
pub const MAX_BODY_CHARS: usize = 4096;

/// `text` on one line: line breaks become [`LINE_BREAK_MARK`]; bidi controls,
/// other control characters and blank fillers become [`REPLACEMENT`];
/// zero-width and other invisible format characters are removed; more than
/// [`MAX_COMBINING_MARKS`] combining marks in a row are dropped.
///
/// ```
/// assert_eq!(tessera_app::text_safety::one_line("a\nb"), "a\u{23CE}b");
/// ```
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Combining marks since the last base character that was written. Hidden
    // characters write nothing, so they cannot reset it.
    let mut marks = 0usize;
    for c in text.chars() {
        let written = match c {
            '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => Some(LINE_BREAK_MARK),
            '\t' => Some(' '),
            // Bidi controls: ALM, LRM, RLM, embeddings/overrides, isolates.
            '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => Some(REPLACEMENT),
            // Blank-rendering fillers: Hangul fillers, half-width filler,
            // braille blank. Visible as a gap, so shown as a mark.
            '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' | '\u{2800}' => Some(REPLACEMENT),
            // Invisible: zero-width space/joiners, soft hyphen, word joiner
            // and invisible operators, deprecated format controls, Mongolian
            // vowel separator, combining grapheme joiner, interlinear
            // annotation controls, variation selectors, tag characters, BOM.
            '\u{200B}'..='\u{200D}'
            | '\u{00AD}'
            | '\u{2060}'..='\u{2064}'
            | '\u{206A}'..='\u{206F}'
            | '\u{180E}'
            | '\u{034F}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{E0000}'..='\u{E007F}'
            | '\u{E0100}'..='\u{E01EF}'
            | '\u{FEFF}' => None,
            c if is_combining_mark(c) => {
                if marks < MAX_COMBINING_MARKS {
                    marks += 1;
                    out.push(c);
                }
                continue;
            }
            c if c.is_control() => Some(REPLACEMENT),
            c => Some(c),
        };
        if let Some(c) = written {
            marks = 0;
            out.push(c);
        }
    }
    out
}

/// Combining diacritical marks (basic, extended, supplement), marks for
/// symbols and half marks.
fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
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

    #[test]
    fn invisible_format_characters_are_removed() {
        let all = ('\u{E0000}'..='\u{E007F}')
            .chain('\u{206A}'..='\u{206F}')
            .chain(['\u{180E}', '\u{034F}'])
            .chain('\u{FFF9}'..='\u{FFFB}')
            .chain('\u{FE00}'..='\u{FE0F}')
            .chain('\u{E0100}'..='\u{E01EF}');
        for c in all {
            assert_eq!(one_line(&format!("a{c}b")), "ab", "{:?}", c);
        }
    }

    #[test]
    fn blank_fillers_are_replaced() {
        for c in ['\u{115F}', '\u{1160}', '\u{3164}', '\u{FFA0}', '\u{2800}'] {
            assert_eq!(
                one_line(&format!("a{c}b")),
                format!("a{REPLACEMENT}b"),
                "{c:?}"
            );
        }
    }

    #[test]
    fn runs_of_combining_marks_are_collapsed() {
        for mark in ['\u{0301}', '\u{1AB0}', '\u{1DC0}', '\u{20D0}', '\u{FE20}'] {
            let four = format!("a{}b", mark.to_string().repeat(MAX_COMBINING_MARKS));
            assert_eq!(one_line(&four), four, "{mark:?} up to the limit is kept");
            let many = format!("a{}b", mark.to_string().repeat(500));
            assert_eq!(one_line(&many), four, "{mark:?}");
        }
    }

    #[test]
    fn combining_marks_are_counted_per_base_and_not_reset_by_hidden_characters() {
        let two = "\u{0301}\u{0302}\u{0303}";
        assert_eq!(one_line(&format!("a{two}b{two}")), format!("a{two}b{two}"));
        // Zero-width characters between marks do not start a new run.
        let sneaky = format!("a{}", "\u{0301}\u{200B}".repeat(50));
        assert_eq!(one_line(&sneaky).chars().count(), 1 + MAX_COMBINING_MARKS);
        // Marks that are different but in one run count together.
        let mixed = "a\u{0301}\u{1AB0}\u{1DC0}\u{20D0}\u{FE20}\u{0302}";
        assert_eq!(one_line(mixed).chars().count(), 1 + MAX_COMBINING_MARKS);
    }
}
