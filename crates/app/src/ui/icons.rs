//! The only place glyphs are defined.
//!
//! Icons are Unicode text today. [`icon`] is the single place that turns one
//! into a widget, so a later move to SVG or an icon font changes this file
//! only. A test fails when any other source file writes a symbol from the
//! arrows-to-dingbats or pictograph ranges outside a comment or a test.

use iced::Element;
use iced::widget::text;

/// An icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// An encrypted room.
    Lock,
    /// An open lock: media that is not encrypted.
    LockOpen,
    /// A call, or a room with a call going.
    Call,
    /// A speaker or the sound level.
    Speaker,
    /// A muted person.
    Muted,
    /// A deafened person.
    Deafened,
    /// The microphone.
    Mic,
    /// The microphone, off.
    MicOff,
    /// Leave a call.
    Leave,
    /// Close a panel.
    Close,
    /// Add, or more.
    Plus,
    /// Remove, or less.
    Minus,
    /// Search.
    Search,
    /// Settings.
    Settings,
    /// People, the member list.
    People,
    /// A thread.
    Thread,
    /// Reply to a message.
    Reply,
    /// React to a message.
    React,
    /// Edit a message.
    Edit,
    /// Delete a message.
    Delete,
    /// Copy.
    Copy,
    /// Attach a file.
    Attach,
    /// The emoji picker.
    Emoji,
    /// Send a message.
    Send,
    /// A security mark.
    Shield,
    /// A warning.
    Warning,
    /// Done, or on.
    Check,
    /// Back, or collapse.
    ChevronLeft,
    /// Forward, or expand.
    ChevronRight,
    /// More actions.
    More,
    /// The graphics card.
    Gpu,
    /// A keyboard key.
    Keyboard,
    /// A gamepad button.
    Gamepad,
    /// A small status dot.
    Dot,
    /// The home view, with no space chosen.
    Home,
    /// The members of a room.
    Members,
    /// A camera.
    Camera,
    /// Sharing a screen or window.
    ScreenShare,
    /// A picture shown over the whole window.
    Fullscreen,
    /// A picture kept in the big place.
    Pin,
    /// Video, as a settings page.
    Video,
}

impl Icon {
    /// Every icon, in declaration order.
    pub const ALL: [Self; 41] = [
        Self::Lock,
        Self::LockOpen,
        Self::Call,
        Self::Speaker,
        Self::Muted,
        Self::Deafened,
        Self::Mic,
        Self::MicOff,
        Self::Leave,
        Self::Close,
        Self::Plus,
        Self::Minus,
        Self::Search,
        Self::Settings,
        Self::People,
        Self::Thread,
        Self::Reply,
        Self::React,
        Self::Edit,
        Self::Delete,
        Self::Copy,
        Self::Attach,
        Self::Emoji,
        Self::Send,
        Self::Shield,
        Self::Warning,
        Self::Check,
        Self::ChevronLeft,
        Self::ChevronRight,
        Self::More,
        Self::Gpu,
        Self::Keyboard,
        Self::Gamepad,
        Self::Dot,
        Self::Home,
        Self::Members,
        Self::Camera,
        Self::ScreenShare,
        Self::Fullscreen,
        Self::Pin,
        Self::Video,
    ];

    /// The Unicode text that draws the icon today.
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Lock => "\u{1F512}",
            Self::LockOpen => "\u{1F513}",
            Self::Call => "\u{1F50A}",
            Self::Speaker => "\u{1F509}",
            Self::Muted => "\u{1F507}",
            Self::Deafened => "\u{1F515}",
            Self::Mic => "\u{1F3A4}",
            Self::MicOff => "\u{1F6AB}",
            Self::Leave => "\u{1F6AA}",
            Self::Close => "\u{2715}",
            Self::Plus => "+",
            Self::Minus => "\u{2212}",
            Self::Search => "\u{1F50D}",
            Self::Settings => "\u{2699}",
            Self::People => "\u{1F465}",
            Self::Thread => "\u{1F9F5}",
            Self::Reply => "\u{21A9}",
            Self::React => "\u{263A}",
            Self::Edit => "\u{270E}",
            Self::Delete => "\u{1F5D1}",
            Self::Copy => "\u{1F4CB}",
            Self::Attach => "\u{1F4CE}",
            Self::Emoji => "\u{1F600}",
            Self::Send => "\u{27A4}",
            Self::Shield => "\u{1F6E1}",
            Self::Warning => "\u{26A0}",
            Self::Check => "\u{2713}",
            Self::ChevronLeft => "\u{2039}",
            Self::ChevronRight => "\u{203A}",
            Self::More => "\u{22EF}",
            Self::Gpu => "\u{1F5A5}",
            Self::Keyboard => "\u{2328}",
            Self::Gamepad => "\u{1F3AE}",
            Self::Dot => "\u{25CF}",
            Self::Home => "\u{2302}",
            Self::Members => "\u{1F464}",
            Self::Camera => "\u{1F4F7}",
            Self::ScreenShare => "\u{1F4FA}",
            Self::Fullscreen => "\u{26F6}",
            Self::Pin => "\u{1F4CC}",
            Self::Video => "\u{1F3A5}",
        }
    }

    /// The words for the icon's tooltip.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lock => "Encrypted",
            Self::LockOpen => "Not encrypted",
            Self::Call => "Call",
            Self::Speaker => "Speaker",
            Self::Muted => "Muted",
            Self::Deafened => "Deafened",
            Self::Mic => "Microphone",
            Self::MicOff => "Microphone off",
            Self::Leave => "Leave",
            Self::Close => "Close",
            Self::Plus => "Add",
            Self::Minus => "Remove",
            Self::Search => "Search",
            Self::Settings => "Settings",
            Self::People => "People",
            Self::Thread => "Thread",
            Self::Reply => "Reply",
            Self::React => "React",
            Self::Edit => "Edit",
            Self::Delete => "Delete",
            Self::Copy => "Copy",
            Self::Attach => "Attach a file",
            Self::Emoji => "Emoji",
            Self::Send => "Send",
            Self::Shield => "Security",
            Self::Warning => "Warning",
            Self::Check => "Done",
            Self::ChevronLeft => "Back",
            Self::ChevronRight => "Forward",
            Self::More => "More",
            Self::Gpu => "Graphics card",
            Self::Keyboard => "Keyboard",
            Self::Gamepad => "Gamepad",
            Self::Dot => "Status",
            Self::Home => "Home",
            Self::Members => "Members",
            Self::Camera => "Camera",
            Self::ScreenShare => "Share your screen",
            Self::Fullscreen => "Full screen",
            Self::Pin => "Pin",
            Self::Video => "Video",
        }
    }
}

/// The widget for `icon`: the one place an [`Icon`] becomes something drawn.
pub fn icon<'a, M: 'a>(icon: Icon) -> Element<'a, M> {
    text(icon.glyph()).into()
}

/// The symbols of `source` that sit outside comments and test code, with
/// their 1-based line numbers. Symbols are the characters of U+2190 to
/// U+2BFF and U+1F300 to U+1FAFF; the ellipsis and the middle dot used in
/// words are outside those ranges. Scanning stops at the first
/// `#[cfg(test)]`, where the test module starts.
#[cfg(test)]
fn symbols_in_code(source: &str) -> Vec<(usize, char)> {
    let is_symbol = |c: char| matches!(c, '\u{2190}'..='\u{2BFF}' | '\u{1F300}'..='\u{1FAFF}');
    let mut found = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            break;
        }
        let chars: Vec<char> = line.chars().collect();
        let (mut i, mut in_string) = (0, false);
        while i < chars.len() {
            let c = chars[i];
            if in_string {
                match c {
                    '\\' => i += 1,
                    '"' => in_string = false,
                    _ if is_symbol(c) => found.push((index + 1, c)),
                    _ => {}
                }
            } else if c == '"' {
                in_string = true;
            } else if c == '/' && chars.get(i + 1) == Some(&'/') {
                break;
            } else if c == '\'' && chars.get(i + 2) == Some(&'\'') {
                // A one-character literal such as 'x' or '"'.
                if is_symbol(chars[i + 1]) {
                    found.push((index + 1, chars[i + 1]));
                }
                i += 2;
            } else if is_symbol(c) {
                found.push((index + 1, c));
            }
            i += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn every_icon_has_a_distinct_glyph_and_a_label() {
        let mut seen = HashSet::new();
        for icon in Icon::ALL {
            assert!(!icon.glyph().is_empty(), "{icon:?}");
            assert!(!icon.label().trim().is_empty(), "{icon:?}");
            assert!(seen.insert(icon.glyph()), "{icon:?} repeats a glyph");
        }
        assert_eq!(Icon::ALL.len(), 41);
        assert_eq!(Icon::Home.label(), "Home");
        assert_eq!(Icon::Members.label(), "Members");
    }

    #[test]
    fn the_old_marks_keep_their_glyphs() {
        assert_eq!(Icon::Lock.glyph(), "\u{1F512}");
        assert_eq!(Icon::Call.glyph(), "\u{1F50A}");
        assert_eq!(Icon::Muted.glyph(), "\u{1F507}");
    }

    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory is readable") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn this_module_is_the_only_place_that_writes_a_symbol() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        assert!(files.len() > 20, "found the sources");
        let mut offences = Vec::new();
        for path in files {
            let rel = path.strip_prefix(&src).expect("under src");
            if rel == Path::new("ui/icons.rs") || rel == Path::new("ui/fonts.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source is UTF-8");
            for (n, symbol) in symbols_in_code(&text) {
                offences.push(format!("{}:{n}: U+{:04X}", rel.display(), symbol as u32));
            }
        }
        assert!(
            offences.is_empty(),
            "symbols outside ui::icons: {offences:#?}"
        );
    }

    #[test]
    fn the_scanner_skips_comments_and_test_code_and_finds_symbols() {
        let sample = "// \u{2715} in a comment\nlet a = \"x\"; // \u{1F512}\nlet b = \"ok \u{2026} \u{B7}\";\n\
                      let c = \"\u{2715}\";\n#[cfg(test)]\nmod tests { \"\u{1F512}\" }\n";
        let found = symbols_in_code(sample);
        assert_eq!(found, vec![(4, '\u{2715}')]);
    }
}
