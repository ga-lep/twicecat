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

/// How many tag characters may follow a black flag (U+1F3F4) to spell a
/// subdivision flag such as England; the rest are dropped.
pub const MAX_TAG_CHARS: usize = 8;

/// Shown in place of a name that is empty or blank once sanitised.
pub const NAME_PLACEHOLDER: &str = "(unnamed)";

/// What [`one_line`] last wrote, for the rules that depend on the character
/// before.
#[derive(Default)]
struct Run {
    /// Combining marks since the last base character that was written.
    marks: usize,
    /// A variation selector (U+FE0E/F) was kept after the last base.
    selector: bool,
    /// A joiner (U+200C/D) was kept after the last base.
    joiner: bool,
    /// `Some(n)` right after a black flag and `n` kept tag characters.
    tags: Option<usize>,
}

/// `text` on one line: line breaks become [`LINE_BREAK_MARK`]; bidi controls,
/// other control characters and blank fillers become [`REPLACEMENT`];
/// zero-width and other invisible format characters are removed; more than
/// [`MAX_COMBINING_MARKS`] combining marks in a row are dropped.
///
/// Variation selectors (U+FE0E/F) and joiners (U+200C/D) are needed by emoji
/// and by Persian and Indic text, so one of each is kept per base character
/// and the rest dropped. Tag characters are kept only right after a black
/// flag (subdivision flags), at most [`MAX_TAG_CHARS`] of them.
///
/// ```
/// assert_eq!(twicecat_app::text_safety::one_line("a\nb"), "a\u{23CE}b");
/// ```
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Hidden characters write nothing, so they cannot reset the mark count.
    let mut run = Run::default();
    for c in text.chars() {
        let previous_tags = run.tags.take();
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
            // Kept once per base: emoji/text presentation selectors.
            '\u{FE0E}' | '\u{FE0F}' => {
                if !run.selector {
                    run.selector = true;
                    out.push(c);
                }
                continue;
            }
            // Kept once per base: zero-width (non-)joiner.
            '\u{200C}' | '\u{200D}' => {
                if !run.joiner {
                    run.joiner = true;
                    out.push(c);
                }
                continue;
            }
            // Kept only straight after a black flag.
            '\u{E0020}'..='\u{E007F}' => {
                if let Some(n) = previous_tags.filter(|n| *n < MAX_TAG_CHARS) {
                    run.tags = Some(n + 1);
                    out.push(c);
                }
                continue;
            }
            // Invisible: zero-width space, soft hyphen, word joiner and
            // invisible operators, deprecated format controls, Khmer inherent
            // vowels, Mongolian free variation selectors and vowel separator,
            // combining grapheme joiner, musical format controls, object and
            // interlinear annotation controls, other variation selectors,
            // other tag characters, BOM.
            '\u{200B}'
            | '\u{00AD}'
            | '\u{2060}'..='\u{2064}'
            | '\u{206A}'..='\u{206F}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{034F}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{FFF9}'..='\u{FFFC}'
            | '\u{FE00}'..='\u{FE0D}'
            | '\u{E0000}'..='\u{E001F}'
            | '\u{E0100}'..='\u{E01EF}'
            | '\u{FEFF}' => None,
            c if is_combining_mark(c) => {
                if run.marks < MAX_COMBINING_MARKS {
                    run.marks += 1;
                    out.push(c);
                }
                continue;
            }
            c if c.is_control() => Some(REPLACEMENT),
            c => Some(c),
        };
        if let Some(c) = written {
            run = Run {
                tags: (c == '\u{1F3F4}').then_some(0),
                ..Run::default()
            };
            out.push(c);
        }
    }
    out
}

/// Combining marks (Unicode Mn and Me) of the Latin, Greek, Cyrillic, Hebrew,
/// Arabic, Thai and Vedic blocks, the diacritical blocks, and marks for
/// symbols and half marks.
fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05BD}'
            | '\u{05BF}'
            | '\u{05C1}'..='\u{05C2}'
            | '\u{05C4}'..='\u{05C5}'
            | '\u{05C7}'
            | '\u{0610}'..='\u{061A}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{06D6}'..='\u{06DC}'
            | '\u{06DF}'..='\u{06E4}'
            | '\u{06E7}'..='\u{06E8}'
            | '\u{06EA}'..='\u{06ED}'
            | '\u{0E31}'
            | '\u{0E34}'..='\u{0E3A}'
            | '\u{0E47}'..='\u{0E4E}'
            | '\u{1AB0}'..='\u{1AFF}'
            // Vedic extensions: only the marks, the block also has letters.
            | '\u{1CD0}'..='\u{1CD2}'
            | '\u{1CD4}'..='\u{1CE0}'
            | '\u{1CE2}'..='\u{1CE8}'
            | '\u{1CED}'
            | '\u{1CF4}'
            | '\u{1CF8}'..='\u{1CF9}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// [`one_line_capped`], or [`NAME_PLACEHOLDER`] when the result is empty or
/// only blanks (space, U+00A0, U+2000 to U+200A, U+3000), which would
/// otherwise draw as nothing at all.
pub fn name_or_placeholder(text: &str, max_chars: usize) -> String {
    let clean = one_line_capped(text, max_chars);
    let blank = clean
        .chars()
        .all(|c| matches!(c, ' ' | '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{3000}'));
    if blank {
        NAME_PLACEHOLDER.to_owned()
    } else {
        clean
    }
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

/// Splits at the first line break (`\n`, `\r\n`, `\r`, NEL, LS or PS): the
/// line, and the rest after the break when there was one.
fn split_line(text: &str) -> (&str, Option<&str>) {
    let Some((at, c)) = text
        .char_indices()
        .find(|(_, c)| matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'))
    else {
        return (text, None);
    };
    let rest = &text[at + c.len_utf8()..];
    let rest = if c == '\r' {
        rest.strip_prefix('\n').unwrap_or(rest)
    } else {
        rest
    };
    (&text[..at], Some(rest))
}

/// [`one_line`] for text that may span lines: every line break variant
/// becomes `\n` (CRLF counts once), and the result has at most `max_lines`
/// lines and `max_chars` characters (line breaks count). A cut ends in
/// [`ELLIPSIS`], and always falls on a character boundary.
///
/// ```
/// use twicecat_app::text_safety::multi_line_capped;
/// assert_eq!(multi_line_capped("a\r\nb\u{2028}c", 100, 2), "a\nb\u{2026}");
/// ```
pub fn multi_line_capped(text: &str, max_chars: usize, max_lines: usize) -> String {
    if max_chars == 0 || max_lines == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = 0;
    let mut rest = Some(text);
    let mut lines = 0;
    let mut cut = false;
    while let Some(remaining) = rest {
        if lines == max_lines || chars > max_chars {
            cut = true;
            break;
        }
        let (line, next) = split_line(remaining);
        if lines > 0 {
            out.push('\n');
            chars += 1;
        }
        let clean = one_line(line);
        chars += clean.chars().count();
        out.push_str(&clean);
        lines += 1;
        rest = next;
    }
    if chars > max_chars || (cut && chars == max_chars) {
        out = out.chars().take(max_chars - 1).collect();
        out.push(ELLIPSIS);
    } else if cut {
        out.push(ELLIPSIS);
    }
    out
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
            '\u{200B}',
            '\u{FEFF}',
            '\u{00AD}',
            '\u{2060}',
            '\u{17B4}',
            '\u{17B5}',
            '\u{180B}',
            '\u{180C}',
            '\u{180D}',
            '\u{180F}',
            '\u{1D173}',
            '\u{1D17A}',
            '\u{FFFC}',
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
        let all = ('\u{E0000}'..='\u{E001F}')
            .chain('\u{206A}'..='\u{206F}')
            .chain(['\u{180E}', '\u{034F}'])
            .chain('\u{FFF9}'..='\u{FFFB}')
            .chain('\u{FE00}'..='\u{FE0D}')
            .chain('\u{E0100}'..='\u{E01EF}')
            .chain('\u{1D173}'..='\u{1D17A}')
            .chain('\u{180B}'..='\u{180D}');
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

    #[test]
    fn combining_marks_of_other_scripts_count_too() {
        let ranges = [
            ('\u{0483}', '\u{0489}'),
            ('\u{0591}', '\u{05BD}'),
            ('\u{05BF}', '\u{05BF}'),
            ('\u{05C1}', '\u{05C2}'),
            ('\u{05C4}', '\u{05C5}'),
            ('\u{05C7}', '\u{05C7}'),
            ('\u{0610}', '\u{061A}'),
            ('\u{064B}', '\u{065F}'),
            ('\u{0670}', '\u{0670}'),
            ('\u{06D6}', '\u{06DC}'),
            ('\u{06DF}', '\u{06E4}'),
            ('\u{06E7}', '\u{06E8}'),
            ('\u{06EA}', '\u{06ED}'),
            ('\u{0E31}', '\u{0E31}'),
            ('\u{0E34}', '\u{0E3A}'),
            ('\u{0E47}', '\u{0E4E}'),
            ('\u{1CD0}', '\u{1CD2}'),
            ('\u{1CD4}', '\u{1CE0}'),
            ('\u{1CE2}', '\u{1CE8}'),
            ('\u{1CED}', '\u{1CED}'),
            ('\u{1CF4}', '\u{1CF4}'),
            ('\u{1CF8}', '\u{1CF9}'),
        ];
        for (first, last) in ranges {
            for mark in first..=last {
                let many = format!("a{}b", mark.to_string().repeat(50));
                assert_eq!(
                    one_line(&many).chars().count(),
                    2 + MAX_COMBINING_MARKS,
                    "{mark:?}"
                );
            }
        }
    }

    #[test]
    fn interleaving_covered_and_newly_covered_marks_does_not_reset_the_count() {
        let nasty = format!("a{}", ("\u{0301}".repeat(4) + "\u{0483}").repeat(50));
        let out = one_line(&nasty);
        assert_eq!(out.chars().count(), 1 + MAX_COMBINING_MARKS);
        assert!(out.starts_with('a'));
    }

    #[test]
    fn names_that_are_blank_after_sanitising_show_a_placeholder() {
        for name in [
            "",
            " ",
            "\u{3000}",
            "\u{2000}\u{200A}",
            "\u{00A0}",
            "\u{200B}\u{FEFF}",
            " \u{3000}\u{00A0}\u{2003}",
        ] {
            assert_eq!(
                name_or_placeholder(name, MAX_NAME_CHARS),
                NAME_PLACEHOLDER,
                "{name:?}"
            );
        }
        assert_eq!(name_or_placeholder("Alice", MAX_NAME_CHARS), "Alice");
        assert_eq!(name_or_placeholder(" a ", MAX_NAME_CHARS), " a ");
    }

    #[test]
    fn emoji_and_joiner_text_survives_unchanged() {
        for text in [
            "\u{2764}\u{FE0F}",
            "1\u{FE0F}\u{20E3}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "\u{1F441}\u{FE0F}\u{200D}\u{1F5E8}\u{FE0F}",
            "\u{0645}\u{06CC}\u{200C}\u{062E}\u{0648}\u{0627}\u{0647}\u{0645}",
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        ] {
            assert_eq!(one_line(text), text, "{text:?}");
        }
    }

    #[test]
    fn runs_of_selectors_and_joiners_collapse_to_one_per_base() {
        assert_eq!(
            one_line(&format!("a{}b", "\u{FE0F}".repeat(500))),
            "a\u{FE0F}b"
        );
        assert_eq!(
            one_line(&format!("a{}b", "\u{FE0E}".repeat(500))),
            "a\u{FE0E}b"
        );
        assert_eq!(
            one_line(&format!("a{}b", "\u{200D}".repeat(500))),
            "a\u{200D}b"
        );
        assert_eq!(
            one_line(&format!("a{}b", "\u{200C}".repeat(500))),
            "a\u{200C}b"
        );
        assert_eq!(one_line(&"\u{FE0F}".repeat(500)), "\u{FE0F}");
        let mixed = "\u{FE0F}\u{200D}\u{FE0E}\u{200C}".repeat(200);
        assert!(one_line(&format!("a{mixed}")).chars().count() <= 3);
    }

    #[test]
    fn tag_characters_only_survive_after_a_black_flag() {
        let tags: String = ('\u{E0020}'..='\u{E007F}').collect();
        assert_eq!(one_line(&format!("a{tags}b")), "ab");
        assert_eq!(one_line(&tags), "");
        // A tag after a flag followed by a letter and more tags: the later ones go.
        assert_eq!(
            one_line("\u{1F3F4}\u{E0067}x\u{E0062}"),
            "\u{1F3F4}\u{E0067}x"
        );
        // A long run after a flag is cut.
        let flag = format!("\u{1F3F4}{tags}{tags}{tags}");
        assert!(one_line(&flag).chars().count() <= 1 + MAX_TAG_CHARS);
        // Hidden characters in between interrupt the sequence.
        assert_eq!(one_line("\u{1F3F4}\u{200B}\u{E0067}"), "\u{1F3F4}");
    }

    #[test]
    fn multi_line_keeps_line_breaks_and_normalises_variants() {
        assert_eq!(multi_line_capped("a\nb", 100, 10), "a\nb");
        for br in ["\r\n", "\r", "\u{85}", "\u{2028}", "\u{2029}", "\n"] {
            assert_eq!(
                multi_line_capped(&format!("a{br}b"), 100, 10),
                "a\nb",
                "{br:?}"
            );
        }
        assert_eq!(multi_line_capped("a\n\nb", 100, 10), "a\n\nb");
        assert_eq!(multi_line_capped("", 100, 10), "");
    }

    #[test]
    fn multi_line_replaces_bidi_and_removes_invisible() {
        assert_eq!(
            multi_line_capped("a\u{202E}b\u{200B}c\nd\u{2066}e", 100, 10),
            format!("a{REPLACEMENT}bc\nd{REPLACEMENT}e")
        );
        assert_eq!(
            multi_line_capped("a\tb\0c", 100, 10),
            format!("a b{REPLACEMENT}c")
        );
    }

    #[test]
    fn multi_line_caps_lines() {
        assert_eq!(multi_line_capped("1\n2\n3", 100, 3), "1\n2\n3");
        assert_eq!(multi_line_capped("1\n2\n3\n4", 100, 3), "1\n2\n3\u{2026}");
        assert_eq!(multi_line_capped("1\n2", 100, 1), "1\u{2026}");
        assert_eq!(multi_line_capped("1\n2", 100, 0), "");
        let many = "x\n".repeat(100_000);
        assert_eq!(multi_line_capped(&many, 10_000, 5).lines().count(), 5);
    }

    #[test]
    fn multi_line_caps_chars_including_line_breaks() {
        assert_eq!(multi_line_capped("abcd", 4, 10), "abcd");
        assert_eq!(multi_line_capped("abcde", 4, 10), "abc\u{2026}");
        assert_eq!(multi_line_capped("ab\ncd", 4, 10), "ab\n\u{2026}");
        assert_eq!(multi_line_capped("abc\nd", 3, 1), "ab\u{2026}");
        assert_eq!(multi_line_capped("abc", 0, 10), "");
        assert_eq!(multi_line_capped("abc", 1, 10), "\u{2026}");
    }

    #[test]
    fn multi_line_cuts_on_char_boundaries() {
        let text = "é\u{1F600}\u{4F60}".repeat(10);
        for max in 0..40 {
            let out = multi_line_capped(&text, max, 3);
            assert!(out.chars().count() <= max, "{max}");
        }
        assert_eq!(
            multi_line_capped("\u{1F600}\u{1F600}\u{1F600}", 2, 3),
            "\u{1F600}\u{2026}"
        );
    }

    #[test]
    fn multi_line_caps_after_sanitising() {
        let zw = "\u{200B}".repeat(500);
        assert_eq!(multi_line_capped(&format!("{zw}ab\nc"), 4, 5), "ab\nc");
    }
}
