//! Custom colour themes: JSON files in the user's `themes/` folder, read
//! under hard limits and checked by [`theme::resolve`].
//!
//! A theme file is untrusted input (it may have been copied from anywhere).
//! Only the 15 colour tokens of [`theme::TOKEN_NAMES`] are read, from the
//! `color` group of a file in the design's `tokens.json` format; nothing else
//! in the file has any effect. What is read is bounded (entries, files, size,
//! nesting), never executed, never turned into a path or a float, and never
//! echoed: a [`ReportLine`] holds a fixed reason and, at most, a theme name
//! that passed the name rules.
//!
//! The design's original token names (`LEGACY_ALIASES`) are read as aliases of
//! the neutral ones; if a file has both, the neutral one wins and the report
//! says so.
//!
//! Files are checked before and after they are opened (no symlink, FIFO or
//! device; same file; size). With std alone that cannot be one atomic step:
//! someone who can write the themes folder can still swap a file for a FIFO
//! between the two, which stalls the read (see `read_theme_file`).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use iced::Color;
use serde_json::Value;

use crate::theme::{self, Refusal, Resolved, Tokens};

/// Largest theme file read, in bytes.
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// Most directory entries looked at.
pub const MAX_ENTRIES: usize = 1024;
/// Most `*.json` files taken.
pub const MAX_FILES: usize = 32;

/// What `parse` did not take over from a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParseNotes {
    /// Tokens present but not a plain opaque sRGB hex colour: the built-in
    /// value is used for each.
    pub not_understood: usize,
    /// Names in the `color` group that are not tokens at all (ignored).
    pub unknown: usize,
    /// Older names present next to the neutral name of the same token: the
    /// neutral one is used and the older one ignored.
    pub aliases_ignored: usize,
}

/// Why a file gives no theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProblem {
    /// Not JSON, or nested too deeply.
    NotJson,
    /// More than [`MAX_FILE_BYTES`].
    TooLarge,
    /// A symlink, FIFO, device or directory.
    NotRegular,
    /// The file could not be opened or read.
    Unreadable,
    /// No colour token in a `color` group.
    NoColours,
    /// `theme::resolve` refused the colours.
    Refused(Refusal),
    /// Named `.light` but its background is dark.
    SaysLight,
    /// Named `.dark` but its background is light.
    SaysDark,
    /// Another file has the same name and mode.
    Duplicate,
}

impl FileProblem {
    /// The fixed words for the report.
    pub fn reason(self) -> &'static str {
        match self {
            FileProblem::NotJson => "not valid JSON",
            FileProblem::TooLarge => "larger than 64 KiB",
            FileProblem::NotRegular => "not a regular file",
            FileProblem::Unreadable => "could not be read",
            FileProblem::NoColours => "no colours",
            FileProblem::Refused(r) => r.reason(),
            FileProblem::SaysLight => "says light but its background is dark",
            FileProblem::SaysDark => "says dark but its background is light",
            FileProblem::Duplicate => "same name and mode as another file",
        }
    }
}

/// What a report line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The file gives no theme.
    Problem(FileProblem),
    /// This many colours were made more readable.
    Adjusted(usize),
    /// This many colours were not understood; the built-in ones are used.
    NotUnderstood(usize),
    /// This many names in the colour group are not tokens.
    Unknown(usize),
    /// This many older token names were ignored because the neutral name of
    /// the same token is there too.
    AliasIgnored(usize),
    /// This many files have a name that is not allowed.
    NamesNotAllowed(usize),
    /// This many more `*.json` files were not read.
    MoreFiles(usize),
    /// The folder has more entries than are looked at.
    MoreEntries,
}

/// One line of the report: a fixed reason and a checked name, nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportLine {
    /// The theme name, which passed the name rules (never file content).
    pub name: Option<String>,
    /// What happened.
    pub note: Note,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

impl fmt::Display for ReportLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = &self.name {
            write!(f, "{name}: ")?;
        }
        match self.note {
            Note::Problem(p) => f.write_str(p.reason()),
            Note::Adjusted(n) => write!(f, "{} made more readable", plural(n, "colour", "colours")),
            Note::NotUnderstood(n) => write!(
                f,
                "{} not understood, Twicecat's used",
                plural(n, "colour", "colours")
            ),
            Note::Unknown(n) => write!(f, "{} ignored", plural(n, "unknown name", "unknown names")),
            Note::AliasIgnored(n) => write!(
                f,
                "{} ignored, {} set",
                plural(n, "older colour name", "older colour names"),
                if n == 1 {
                    "its new name is"
                } else {
                    "their new names are"
                }
            ),
            Note::NamesNotAllowed(n) => write!(
                f,
                "{} with a name that is not allowed",
                plural(n, "file", "files")
            ),
            Note::MoreFiles(n) => write!(f, "{n} more files not read"),
            Note::MoreEntries => f.write_str("more than 1,024 entries, the rest not looked at"),
        }
    }
}

/// What a folder gave.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loaded {
    /// The themes that can be shown, in file-name order.
    pub themes: Vec<Resolved>,
    /// What to tell the user.
    pub report: Vec<ReportLine>,
}

impl Loaded {
    /// The report as text: at most `max` lines, then "and N more".
    pub fn shown(&self, max: usize) -> Vec<String> {
        let mut lines: Vec<String> = self
            .report
            .iter()
            .take(max)
            .map(ToString::to_string)
            .collect();
        if self.report.len() > max {
            lines.push(format!("and {} more", self.report.len() - max));
        }
        lines
    }
}

/// Whether `name` may name a theme: 1 to 32 bytes of `[A-Za-z0-9 _-]`, no
/// leading or trailing space, and not "twicecat" or "default" in any case.
pub fn name_allowed(name: &str) -> bool {
    twicecat_core::api::ThemeName::try_from(name).is_ok()
}

/// Names of the design's tokens that no theme uses; ignored without a count.
const KNOWN_UNUSED: [&str; 4] = ["line", "ink-faint", "phosphor", "violet-deep"];

/// The design's original token names, which are read as aliases: (older name,
/// token name). This table is the only place in the app that spells them.
const LEGACY_ALIASES: [(&str, &str); 4] = [
    ("violet-wash", "accent-wash"),
    ("violet", "accent"),
    ("on-violet", "on-accent"),
    ("violet-bright", "accent-bright"),
];

/// `#rrggbb`, or `#rrggbbaa` with `aa = ff`, in ASCII hex; nothing else.
fn parse_hex(text: &str) -> Option<Color> {
    let bytes = text.as_bytes();
    if !matches!(bytes.len(), 7 | 9) || bytes[0] != b'#' {
        return None;
    }
    if !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    if bytes.len() == 9 && !bytes[7..].eq_ignore_ascii_case(b"ff") {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&text[at..at + 2], 16).ok();
    Some(Color::from_rgb8(channel(1)?, channel(3)?, channel(5)?))
}

/// A token's colour: its `$value` as a string, or as an object whose `hex` is
/// the string (and whose `colorSpace`, if there, is `srgb`).
fn read_colour(entry: &Value) -> Option<Color> {
    match entry.as_object()?.get("$value")? {
        Value::String(text) => parse_hex(text),
        Value::Object(value) => {
            if let Some(space) = value.get("colorSpace")
                && space.as_str() != Some("srgb")
            {
                return None;
            }
            parse_hex(value.get("hex")?.as_str()?)
        }
        _ => None,
    }
}

/// Reads the colours of one theme file's bytes. A token that is missing takes
/// the built-in value, silently; one that is present but not understood takes
/// it and is counted. The design's original names are read as aliases (see
/// `LEGACY_ALIASES`).
pub fn parse(bytes: &[u8]) -> Result<(Tokens, ParseNotes), FileProblem> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(FileProblem::TooLarge);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    // serde_json's recursion limit (128) makes deep nesting an error.
    let value: Value = serde_json::from_slice(bytes).map_err(|_| FileProblem::NotJson)?;
    let Some(group) = value.get("color").and_then(Value::as_object) else {
        return Err(FileProblem::NoColours);
    };
    let mut tokens = theme::BUILT_IN;
    let mut notes = ParseNotes::default();
    let mut present = 0;
    for name in theme::TOKEN_NAMES {
        let older = LEGACY_ALIASES
            .iter()
            .find(|(_, token)| *token == name)
            .and_then(|(alias, _)| group.get(*alias));
        // The neutral name wins whenever it is there, valid or not (presence
        // decides); the older name is used only when it is the only one.
        let entry = match (group.get(name), older) {
            (Some(entry), older) => {
                notes.aliases_ignored += usize::from(older.is_some());
                entry
            }
            (None, Some(entry)) => entry,
            (None, None) => continue,
        };
        present += 1;
        match read_colour(entry) {
            Some(colour) => {
                tokens.set(name, colour);
            }
            None => notes.not_understood += 1,
        }
    }
    if present == 0 {
        return Err(FileProblem::NoColours);
    }
    notes.unknown = group
        .keys()
        .filter(|key| {
            let key = key.as_str();
            !key.starts_with('$')
                && !theme::TOKEN_NAMES.contains(&key)
                && !LEGACY_ALIASES.iter().any(|(alias, _)| *alias == key)
                && !KNOWN_UNUSED.contains(&key)
        })
        .count();
    Ok((tokens, notes))
}

/// Whether `opened` (the metadata of the open handle) is the file `looked`
/// (the metadata of the path, without following links) saw: same device and
/// inode. Where that cannot be told, the file type is all there is to compare.
#[cfg(unix)]
fn is_same_file(looked: &fs::Metadata, opened: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    looked.dev() == opened.dev() && looked.ino() == opened.ino()
}

#[cfg(not(unix))]
fn is_same_file(looked: &fs::Metadata, opened: &fs::Metadata) -> bool {
    looked.file_type() == opened.file_type()
}

/// The bytes of a theme file. The path is checked with `symlink_metadata`
/// before it is opened (a FIFO would block `open`; symlinks and devices are
/// refused). The opened handle is then checked again: it must be the same
/// file (device and inode), a regular file, and within the size cap; at most
/// one byte over the cap is read.
///
/// Residual race: std offers no `O_NOFOLLOW | O_NONBLOCK` open without a new
/// dependency, so between the check and the open an attacker who can write
/// the themes folder may swap the file. A swap is noticed after the open (the
/// handle is not the file that was checked and is refused), but a swap to a
/// FIFO can still make the open itself block. That stalls only the
/// `twicecat-themes` thread of [`load_off_thread`], never the window, and
/// needs no `libc`.
fn read_theme_file(path: &Path) -> Result<Vec<u8>, FileProblem> {
    let looked = fs::symlink_metadata(path).map_err(|_| FileProblem::Unreadable)?;
    if !looked.file_type().is_file() {
        return Err(FileProblem::NotRegular);
    }
    if looked.len() > MAX_FILE_BYTES as u64 {
        return Err(FileProblem::TooLarge);
    }
    let file = fs::File::open(path).map_err(|_| FileProblem::Unreadable)?;
    let opened = file.metadata().map_err(|_| FileProblem::Unreadable)?;
    if !opened.is_file() || !is_same_file(&looked, &opened) {
        return Err(FileProblem::NotRegular);
    }
    if opened.len() > MAX_FILE_BYTES as u64 {
        return Err(FileProblem::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FileProblem::Unreadable)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(FileProblem::TooLarge);
    }
    Ok(bytes)
}

/// Reads the themes folder on a thread of its own, named `twicecat-themes`,
/// and hands the result back through a `tokio::sync::oneshot` channel. The UI
/// thread (and the shared [`crate::blocking`] pool) never touches the folder,
/// so a file that stalls its read stalls only this thread. `None` if the
/// thread could not start or panicked.
pub async fn load_off_thread(dir: PathBuf) -> Option<Loaded> {
    load_off_thread_with(move || load_dir(&dir)).await
}

/// [`load_off_thread`] with the loader given, so that tests can stall it.
async fn load_off_thread_with(load: impl FnOnce() -> Loaded + Send + 'static) -> Option<Loaded> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("twicecat-themes".to_owned())
        .spawn(move || {
            // The receiver may be gone (the app closed): nothing to tell.
            let _ = sender.send(load());
        })
        .ok()?;
    // A panic drops the sender without a value.
    receiver.await.ok()
}

/// Which side a file name asks for: `.light`, `.dark` or none.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Suffix {
    Light,
    Dark,
    None,
}

/// A file name `<name>[.light|.dark].json` with a name that passes the rules:
/// the name and its suffix.
fn split_file_name(file_name: &OsStr) -> Option<(&str, Suffix)> {
    let stem = file_name.to_str()?.strip_suffix(".json")?;
    let (name, suffix) = if let Some(name) = stem.strip_suffix(".light") {
        (name, Suffix::Light)
    } else if let Some(name) = stem.strip_suffix(".dark") {
        (name, Suffix::Dark)
    } else {
        (stem, Suffix::None)
    };
    name_allowed(name).then_some((name, suffix))
}

/// Loads the theme files of `dir`, under these limits:
/// - 1,024 directory entries looked at;
/// - 32 `*.json` files with allowed names, taken in byte order of their names,
///   64 KiB each;
/// - no symlink, FIFO or device opened (see `read_theme_file` for the one race
///   that remains).
///
/// Files whose name is not allowed are counted apart and do not use up the
/// 32. A folder that cannot be read gives nothing.
///
/// `<Name>.json`, `<Name>.light.json` and `<Name>.dark.json` all name the theme
/// `<Name>`. When the accepted files of one name are one light and one dark
/// look, they are a pair: both [`Resolved`] entries are returned and both have
/// `both_modes` set. A second file with the same name and the same mode is
/// reported and not used. Names that differ only in ASCII capitals are the
/// same name (the first spelling in byte order is kept). See the module docs
/// for what the report may say.
pub fn load_dir(dir: &Path) -> Loaded {
    let mut loaded = Loaded::default();
    let Ok(entries) = fs::read_dir(dir) else {
        return loaded;
    };
    let mut names: Vec<OsString> = Vec::new();
    let mut bad_names = 0;
    let mut more_entries = false;
    for (seen, entry) in entries.enumerate() {
        // Every entry counts, readable or not, json or not.
        if seen >= MAX_ENTRIES {
            more_entries = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        if !file_name.as_encoded_bytes().ends_with(b".json") {
            continue;
        }
        // The name is checked before the file limit applies, so files that
        // are refused anyway cannot push a valid theme out of the 32.
        if split_file_name(&file_name).is_some() {
            names.push(file_name);
        } else {
            bad_names += 1;
        }
    }
    names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
    let skipped = names.len().saturating_sub(MAX_FILES);
    names.truncate(MAX_FILES);

    for file_name in &names {
        let Some((name, suffix)) = split_file_name(file_name) else {
            continue;
        };
        let label = match suffix {
            Suffix::Light => format!("{name} (light)"),
            Suffix::Dark => format!("{name} (dark)"),
            Suffix::None => name.to_owned(),
        };
        let mut say = |note: Note| {
            loaded.report.push(ReportLine {
                name: Some(label.clone()),
                note,
            });
        };
        let (tokens, notes) =
            match read_theme_file(&dir.join(file_name)).and_then(|bytes| parse(&bytes)) {
                Ok(read) => read,
                Err(problem) => {
                    say(Note::Problem(problem));
                    continue;
                }
            };
        let mut resolved = match theme::resolve(&tokens) {
            Ok(resolved) => resolved,
            Err(refusal) => {
                say(Note::Problem(FileProblem::Refused(refusal)));
                continue;
            }
        };
        let wrong_side = match suffix {
            Suffix::Light if resolved.dark => Some(FileProblem::SaysLight),
            Suffix::Dark if !resolved.dark => Some(FileProblem::SaysDark),
            _ => None,
        };
        if let Some(problem) = wrong_side {
            say(Note::Problem(problem));
            continue;
        }
        // Names that differ only in ASCII capitals are one theme: the first
        // spelling in byte order is kept, so a pair or a duplicate is found
        // whatever the capitals are.
        let name = loaded
            .themes
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map_or(name, |t| t.name.as_str())
            .to_owned();
        if loaded
            .themes
            .iter()
            .any(|t| t.name == name && t.dark == resolved.dark)
        {
            say(Note::Problem(FileProblem::Duplicate));
            continue;
        }
        if !resolved.report.is_empty() {
            say(Note::Adjusted(resolved.report.len()));
        }
        if notes.not_understood > 0 {
            say(Note::NotUnderstood(notes.not_understood));
        }
        if notes.unknown > 0 {
            say(Note::Unknown(notes.unknown));
        }
        if notes.aliases_ignored > 0 {
            say(Note::AliasIgnored(notes.aliases_ignored));
        }
        resolved.name = name;
        loaded.themes.push(resolved);
    }
    // A light and a dark theme of one name are a pair.
    let pairs: Vec<String> = loaded
        .themes
        .iter()
        .filter(|a| {
            loaded
                .themes
                .iter()
                .any(|b| b.name == a.name && b.dark != a.dark)
        })
        .map(|t| t.name.clone())
        .collect();
    for t in &mut loaded.themes {
        t.both_modes = pairs.contains(&t.name);
    }
    for note in [
        (bad_names > 0).then_some(Note::NamesNotAllowed(bad_names)),
        (skipped > 0).then_some(Note::MoreFiles(skipped)),
        more_entries.then_some(Note::MoreEntries),
    ]
    .into_iter()
    .flatten()
    {
        loaded.report.push(ReportLine { name: None, note });
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{BUILT_IN, TOKEN_NAMES};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A fresh directory under the system temp dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Dir {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("twicecat-themes-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create the test dir");
            Dir(path)
        }

        fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).expect("write a test file");
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/themes")
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(fixtures().join(name)).expect("a fixture")
    }

    /// Loads one fixture from `hostile/` as the file `as_name`, timing it.
    fn load_hostile(fixture_name: &str, as_name: &str) -> Loaded {
        let dir = Dir::new();
        dir.put(
            as_name,
            &std::fs::read(fixtures().join("hostile").join(fixture_name)).expect("a fixture"),
        );
        timed(|| load_dir(&dir.0))
    }

    fn timed<T>(f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        assert!(start.elapsed() < Duration::from_secs(1), "took too long");
        out
    }

    fn lines(loaded: &Loaded) -> Vec<String> {
        loaded.report.iter().map(ToString::to_string).collect()
    }

    fn hex(c: iced::Color) -> String {
        let [r, g, b, _] = c.into_rgba8();
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// The built-in colours as a theme file.
    fn built_in_doc_with(changes: &[(&str, &str)]) -> String {
        let mut out = String::from("{\"color\":{");
        for (i, (name, c)) in TOKEN_NAMES.iter().zip(BUILT_IN.as_array()).enumerate() {
            let value = changes
                .iter()
                .find(|(n, _)| n == name)
                .map_or_else(|| hex(c), |(_, v)| (*v).to_owned());
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{name}\":{{\"$value\":\"{value}\"}}"));
        }
        out.push_str("}}");
        out
    }

    // ---- a minimal SHA-256, to check the copied design file -------------

    fn sha256(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut msg = data.to_vec();
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
        for chunk in msg.chunks(64) {
            let mut w = [0u32; 64];
            for (i, b) in chunk.chunks(4).enumerate() {
                w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
                let t1 = v[7]
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v = [
                    t1.wrapping_add(t2),
                    v[0],
                    v[1],
                    v[2],
                    v[3].wrapping_add(t1),
                    v[4],
                    v[5],
                    v[6],
                ];
            }
            for (a, b) in h.iter_mut().zip(v) {
                *a = a.wrapping_add(b);
            }
        }
        h.iter().map(|x| format!("{x:08x}")).collect()
    }

    #[test]
    fn the_sha256_helper_is_right() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ---- the design file and the fixtures ----------------------------------

    #[test]
    fn the_design_tokens_file_is_the_copied_one() {
        let bytes = include_bytes!("../assets/themes/twicecat.tokens.json");
        assert_eq!(bytes.len(), 13_555);
        assert_eq!(
            sha256(bytes),
            "0388d3cff0874d672be530e62d153b8085558331e31d5bad6e67c1dfda8bcf2b"
        );
    }

    #[test]
    fn the_design_tokens_file_parses_to_the_built_in_colours_unchanged() {
        let bytes = include_bytes!("../assets/themes/twicecat.tokens.json");
        let (tokens, notes) = parse(bytes).expect("the design file is a theme");
        assert_eq!(tokens, BUILT_IN);
        assert_eq!(notes, ParseNotes::default(), "line, ink-faint... are known");
        let resolved = theme::resolve(&tokens).expect("it is accepted");
        assert!(resolved.report.is_empty(), "{:?}", resolved.report);
        let built = theme::built_in_resolved();
        assert_eq!(resolved.palette, built.palette);
        assert_eq!(resolved.semantic, built.semantic);
    }

    #[test]
    fn daylight_paper_and_duo_resolve_with_no_adjustment_and_faint_with_one() {
        let loaded = timed(|| load_dir(&fixtures()));
        let names: Vec<(&str, bool)> = loaded
            .themes
            .iter()
            .map(|t| (t.name.as_str(), t.dark))
            .collect();
        assert_eq!(
            names,
            [
                ("Daylight", false),
                ("Duo", true),
                ("Duo", false),
                ("Faint", true),
                ("Paper", false)
            ],
            "{:?}",
            lines(&loaded)
        );
        for t in &loaded.themes {
            match t.name.as_str() {
                "Faint" => assert_eq!(t.report, ["ink"]),
                other => assert!(t.report.is_empty(), "{other}: {:?}", t.report),
            }
            assert_eq!(t.both_modes, t.name == "Duo", "{}", t.name);
        }
        assert_eq!(
            lines(&loaded),
            ["Faint (dark): 1 colour made more readable"]
        );
    }

    // ---- hostile files -----------------------------------------------------

    const SECRETS: [&str; 6] = ["#zzzzzz", "NaN", "rgb(", "FILLER", "\u{ff26}", "display-p3"];

    fn assert_no_leak(loaded: &Loaded) {
        for line in lines(loaded) {
            for secret in SECRETS {
                assert!(!line.contains(secret), "{line:?} leaks {secret:?}");
            }
        }
    }

    fn one_refused(fixture_name: &str, file: &str, reason: &str) {
        let loaded = load_hostile(fixture_name, file);
        assert!(loaded.themes.is_empty(), "{fixture_name} was accepted");
        let label = file
            .trim_end_matches(".json")
            .replace(".light", " (light)")
            .replace(".dark", " (dark)");
        assert_eq!(lines(&loaded), [format!("{label}: {reason}")]);
        assert_no_leak(&loaded);
    }

    #[test]
    fn hostile_all_tokens_the_same_grey_are_refused() {
        one_refused(
            "all_same_grey.json",
            "Grey.json",
            "status colours look too alike",
        );
    }

    #[test]
    fn hostile_status_colours_that_collide_are_refused() {
        for f in ["live_eq_ink.json", "live_eq_ok.json", "warn_near_live.json"] {
            one_refused(f, "Clash.json", "status colours look too alike");
        }
    }

    #[test]
    fn hostile_a_selected_row_that_would_not_show_is_refused() {
        one_refused(
            "wash_eq_surface.json",
            "Flat.json",
            "selected rows would not show",
        );
    }

    #[test]
    fn hostile_light_void_over_dark_surfaces_is_refused() {
        one_refused(
            "light_void_dark_surfaces.json",
            "Mixed.json",
            "backgrounds mix light and dark",
        );
    }

    #[test]
    fn hostile_a_name_that_disagrees_with_the_background_is_refused() {
        one_refused(
            "dark_void_for_light_name.json",
            "X.light.json",
            "says light but its background is dark",
        );
        let dir = Dir::new();
        dir.put("Y.dark.json", &fixture("Paper.light.json"));
        let loaded = load_dir(&dir.0);
        assert!(loaded.themes.is_empty());
        assert_eq!(
            lines(&loaded),
            ["Y (dark): says dark but its background is light"]
        );
    }

    #[test]
    fn hostile_every_token_with_an_alpha_falls_back() {
        let loaded = load_hostile("alpha_01.json", "Faded.json");
        assert_eq!(
            lines(&loaded),
            ["Faded: 15 colours not understood, Twicecat's used"]
        );
        let theme = &loaded.themes[0];
        assert_eq!(theme.palette, theme::built_in_resolved().palette);
        assert_eq!(theme.semantic, theme::built_in_resolved().semantic);
    }

    #[test]
    fn hostile_values_that_are_not_colours_fall_back_each() {
        let bytes = fixture("hostile/garbage_values.json");
        let (tokens, notes) = parse(&bytes).expect("a theme with fallbacks");
        assert_eq!(tokens, BUILT_IN);
        assert_eq!(
            notes.not_understood, 9,
            "NaN, #zzzzzz, #fff, rgb(), empty, number, null, alias, full-width"
        );
        let loaded = load_hostile("garbage_values.json", "Junk.json");
        assert_eq!(
            lines(&loaded),
            ["Junk: 9 colours not understood, Twicecat's used"]
        );
        assert_no_leak(&loaded);
    }

    #[test]
    fn hostile_a_colour_space_other_than_srgb_falls_back() {
        let (tokens, notes) = parse(&fixture("hostile/display_p3.json")).expect("a theme");
        assert_eq!(tokens.accent, BUILT_IN.accent, "the p3 value is not used");
        assert_eq!(notes.not_understood, 1);
        assert_no_leak(&load_hostile("display_p3.json", "P3.json"));
    }

    #[test]
    fn hostile_wrong_types_give_no_colours_or_per_token_fallbacks() {
        for f in [
            "top_level_array.json",
            "color_array.json",
            "color_string.json",
            "empty_object.json",
        ] {
            one_refused(f, "Odd.json", "no colours");
        }
        let loaded = load_hostile("number_token.json", "Odd.json");
        assert_eq!(
            lines(&loaded),
            ["Odd: 2 colours not understood, Twicecat's used"]
        );
    }

    #[test]
    fn hostile_a_file_over_64_kib_is_refused() {
        let dir = Dir::new();
        let mut text = String::from("{\"color\":{},\"x\":\"");
        text.push_str(&"FILLER".repeat(MAX_FILE_BYTES));
        text.truncate(MAX_FILE_BYTES + 1 - 2);
        text.push_str("\"}");
        assert_eq!(text.len(), MAX_FILE_BYTES + 1);
        dir.put("Big.json", text.as_bytes());
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(lines(&loaded), ["Big: larger than 64 KiB"]);
        assert_eq!(parse(text.as_bytes()), Err(FileProblem::TooLarge));
        assert_no_leak(&loaded);
    }

    #[test]
    fn hostile_deep_nesting_is_not_json_and_does_not_overflow_the_stack() {
        let arrays = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let objects = format!(
            "{{\"color\":{}1{}}}",
            "{\"a\":".repeat(10_000),
            "}".repeat(10_000)
        );
        assert!(objects.len() < MAX_FILE_BYTES);
        for (name, text) in [("Arrays.json", arrays), ("Objects.json", objects)] {
            let dir = Dir::new();
            dir.put(name, text.as_bytes());
            let loaded = timed(|| load_dir(&dir.0));
            let stem = name.trim_end_matches(".json");
            assert_eq!(lines(&loaded), [format!("{stem}: not valid JSON")]);
        }
    }

    #[test]
    fn hostile_forty_valid_files_load_thirty_two() {
        let dir = Dir::new();
        for i in 0..40 {
            dir.put(&format!("T{i:02}.json"), built_in_doc_with(&[]).as_bytes());
        }
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(loaded.themes.len(), MAX_FILES);
        assert_eq!(lines(&loaded), ["8 more files not read"]);
        assert_eq!(loaded.themes[0].name, "T00");
    }

    #[test]
    fn thirty_two_badly_named_files_sorted_first_do_not_hide_a_valid_theme() {
        let dir = Dir::new();
        let doc = built_in_doc_with(&[]);
        for i in 0..MAX_FILES {
            dir.put(&format!("!bad{i:02}.json"), doc.as_bytes());
        }
        dir.put("Zed.json", doc.as_bytes());
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(loaded.themes.len(), 1);
        assert_eq!(loaded.themes[0].name, "Zed");
        assert_eq!(lines(&loaded), ["32 files with a name that is not allowed"]);
    }

    #[test]
    fn badly_named_files_do_not_count_against_the_file_limit() {
        let dir = Dir::new();
        let doc = built_in_doc_with(&[]);
        for i in 0..5 {
            dir.put(&format!("!bad{i}.json"), doc.as_bytes());
        }
        for i in 0..MAX_FILES + 2 {
            dir.put(&format!("T{i:02}.json"), doc.as_bytes());
        }
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(loaded.themes.len(), MAX_FILES);
        assert_eq!(
            lines(&loaded),
            [
                "5 files with a name that is not allowed",
                "2 more files not read"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_swapped_in_between_the_check_and_the_open_is_noticed() {
        let dir = Dir::new();
        let a = dir.put("a.txt", b"a");
        let b = dir.put("b.txt", b"b");
        let looked = std::fs::symlink_metadata(&a).expect("metadata");
        let same = std::fs::File::open(&a)
            .and_then(|f| f.metadata())
            .expect("metadata");
        let other = std::fs::File::open(&b)
            .and_then(|f| f.metadata())
            .expect("metadata");
        assert!(is_same_file(&looked, &same));
        assert!(!is_same_file(&looked, &other));
    }

    #[test]
    fn hostile_names_that_are_not_allowed_are_counted_not_shown() {
        let dir = Dir::new();
        let doc = built_in_doc_with(&[]);
        for name in [
            "evil\u{202e}gpj.json",
            "a.b.json",
            "twicecat.json",
            "Default.json",
            " lead.json",
            "trail .json",
            &format!("{}.json", "n".repeat(33)),
            ".json",
        ] {
            dir.put(name, doc.as_bytes());
        }
        dir.put("Fine.json", doc.as_bytes());
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(loaded.themes.len(), 1);
        assert_eq!(lines(&loaded), ["8 files with a name that is not allowed"]);
        assert_no_leak(&loaded);
    }

    #[test]
    fn the_name_rules() {
        for ok in [
            "Nord",
            "a b",
            "A_b-c",
            "x",
            &"n".repeat(32),
            "Twicecat2",
            "Default theme",
        ] {
            assert!(name_allowed(ok), "{ok:?}");
        }
        for bad in [
            "",
            " a",
            "a ",
            "../x",
            "a/b",
            "a\\b",
            "a.b",
            "a\u{202e}b",
            "\u{ff21}",
            "caf\u{e9}",
            "twicecat",
            "TWICECAT",
            "Default",
            "default",
            "CON",
            "nul",
            "Com1",
            "LPT9",
            "Aux",
            "prn",
            &"n".repeat(33),
            "a\nb",
            "a\0b",
        ] {
            assert!(!name_allowed(bad), "{bad:?}");
        }
    }

    #[test]
    fn name_allowed_and_the_core_theme_name_agree() {
        use twicecat_core::api::ThemeName;
        let long = "a".repeat(33);
        for name in [
            "../x",
            "a/b",
            "C:\\x",
            "",
            " lead",
            &long,
            "\u{202e}",
            "TwiceCat",
            "DEFAULT",
            "Day.light",
            "CON",
            "com9",
            "COM0",
            "Daylight",
            "Nord Dark",
            "a_b-c 1",
        ] {
            assert_eq!(
                name_allowed(name),
                ThemeName::try_from(name).is_ok(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn hostile_the_same_name_and_mode_twice_keeps_the_first_by_file_name() {
        let dir = Dir::new();
        dir.put(
            "Nord.json",
            built_in_doc_with(&[("ink", "#eeeeee")]).as_bytes(),
        );
        dir.put(
            "Nord.dark.json",
            built_in_doc_with(&[("ink", "#f5f2ff")]).as_bytes(),
        );
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(loaded.themes.len(), 1);
        assert_eq!(
            hex(loaded.themes[0].palette.text),
            "#f5f2ff",
            "Nord.dark.json sorts first"
        );
        assert_eq!(lines(&loaded), ["Nord: same name and mode as another file"]);
    }

    // ---- neutral names and the design's original names (aliases) -----------

    /// A theme file with the given `color` entries, each `"name": "#hex"`.
    fn doc_of(entries: &[(&str, &str)]) -> String {
        let rows: Vec<String> = entries
            .iter()
            .map(|(n, v)| format!("\"{n}\":{{\"$value\":\"{v}\"}}"))
            .collect();
        format!("{{\"color\":{{{}}}}}", rows.join(","))
    }

    #[test]
    fn the_token_names_are_the_neutral_ones_in_order() {
        assert_eq!(
            TOKEN_NAMES,
            [
                "void",
                "surface",
                "surface-raised",
                "surface-overlay",
                "accent-wash",
                "line-strong",
                "ink",
                "ink-muted",
                "accent",
                "on-accent",
                "accent-bright",
                "focus",
                "live",
                "ok",
                "warn"
            ]
        );
    }

    #[test]
    fn an_older_name_alone_sets_its_token_without_a_note() {
        let text = doc_of(&[
            ("violet", "#112233"),
            ("on-violet", "#445566"),
            ("violet-bright", "#778899"),
            ("violet-wash", "#aabbcc"),
        ]);
        let (tokens, notes) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(hex(tokens.accent), "#112233");
        assert_eq!(hex(tokens.on_accent), "#445566");
        assert_eq!(hex(tokens.accent_bright), "#778899");
        assert_eq!(hex(tokens.accent_wash), "#aabbcc");
        assert_eq!(notes, ParseNotes::default());
    }

    #[test]
    fn the_neutral_name_wins_over_the_older_one_and_the_older_one_is_counted() {
        let text = doc_of(&[("accent", "#112233"), ("violet", "#ff0000")]);
        let (tokens, notes) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(hex(tokens.accent), "#112233");
        assert_eq!(notes.aliases_ignored, 1);
        assert_eq!(notes.not_understood, 0);
        assert_eq!(notes.unknown, 0);
        // The order in the file does not matter.
        let text = doc_of(&[("violet", "#ff0000"), ("accent", "#112233")]);
        let (tokens, notes) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(hex(tokens.accent), "#112233");
        assert_eq!(notes.aliases_ignored, 1);
    }

    #[test]
    fn presence_decides_so_a_bad_neutral_value_does_not_fall_back_to_the_older_name() {
        let text = doc_of(&[("accent", "nonsense"), ("violet", "#ff0000")]);
        let (tokens, notes) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(tokens.accent, BUILT_IN.accent, "the built-in value");
        assert_eq!(notes.not_understood, 1);
        assert_eq!(notes.aliases_ignored, 1);
    }

    #[test]
    fn an_older_name_with_a_bad_value_falls_back_like_any_token() {
        let text = doc_of(&[("violet", "nonsense")]);
        let (tokens, notes) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(tokens.accent, BUILT_IN.accent);
        assert_eq!(notes.not_understood, 1);
        assert_eq!(notes.aliases_ignored, 0);
        assert_eq!(notes.unknown, 0, "an older name is not an unknown one");
    }

    #[test]
    fn hostile_neutral_and_older_names_together_use_the_neutral_ones() {
        let bytes = fixture("hostile/alias_and_neutral.json");
        let (tokens, notes) = parse(&bytes).expect("a theme");
        assert_eq!(tokens, BUILT_IN, "the neutral values win");
        assert_eq!(notes.aliases_ignored, 2);
        assert_eq!(notes.not_understood, 0);
        let loaded = load_hostile("alias_and_neutral.json", "alias_and_neutral.json");
        assert_eq!(loaded.themes.len(), 1);
        assert_eq!(
            lines(&loaded),
            ["alias_and_neutral: 2 older colour names ignored, their new names are set"]
        );
        assert_no_leak(&loaded);
        // One older name gives the singular text.
        assert_eq!(
            ReportLine {
                name: Some("X".to_owned()),
                note: Note::AliasIgnored(1)
            }
            .to_string(),
            "X: 1 older colour name ignored, its new name is set"
        );
    }

    #[test]
    fn the_starting_file_is_the_built_in_colours_under_the_neutral_names() {
        let bytes = include_bytes!("../assets/themes/Twicecat-copy.json");
        let (tokens, notes) = parse(bytes).expect("a theme");
        assert_eq!(tokens, BUILT_IN);
        assert_eq!(notes, ParseNotes::default());
        let text = std::str::from_utf8(bytes).expect("UTF-8");
        // serde_json sorts keys, so the order is checked on the text.
        let at: Vec<usize> = TOKEN_NAMES
            .iter()
            .map(|n| {
                text.find(&format!("\"{n}\":"))
                    .expect("every token is there")
            })
            .collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "TOKEN_NAMES order");
        let value: Value = serde_json::from_slice(bytes).expect("JSON");
        let group = value["color"].as_object().expect("a colour group");
        let tokens_in_file = group.keys().filter(|k| !k.starts_with('$')).count();
        assert_eq!(tokens_in_file, TOKEN_NAMES.len(), "exactly the 15 tokens");
        assert!(!text.to_ascii_lowercase().contains("violet"));
        let resolved = theme::resolve(&tokens).expect("accepted");
        assert!(resolved.report.is_empty());
    }

    // ---- Daylight ----------------------------------------------------------

    /// The 15 values of Daylight, in `TOKEN_NAMES` order.
    const DAYLIGHT: [&str; 15] = [
        "#ffffff", "#f6f7f9", "#eceef2", "#d8dde6", "#d3e8e3", "#5b6672", "#14181d", "#4a5560",
        "#0b6b5c", "#ffffff", "#0b6b5c", "#0b6b5c", "#b3112f", "#17622f", "#8a4b00",
    ];

    #[test]
    fn daylight_loads_as_one_light_theme_with_nothing_adjusted() {
        let dir = Dir::new();
        dir.put("Daylight.light.json", &fixture("Daylight.light.json"));
        let loaded = timed(|| load_dir(&dir.0));
        assert!(loaded.report.is_empty(), "{:?}", lines(&loaded));
        assert_eq!(loaded.themes.len(), 1);
        let day = &loaded.themes[0];
        assert_eq!(day.name, "Daylight");
        assert!(!day.dark);
        assert!(!day.both_modes);
        assert!(day.report.is_empty(), "{:?}", day.report);
    }

    #[test]
    fn daylights_fifteen_values_are_pinned() {
        let (tokens, notes) = parse(&fixture("Daylight.light.json")).expect("a theme");
        assert_eq!(notes, ParseNotes::default());
        let got: Vec<String> = tokens.as_array().into_iter().map(hex).collect();
        assert_eq!(got, DAYLIGHT);
    }

    #[test]
    fn the_drafts_surface_overlay_would_hide_the_hover_row_and_is_refused() {
        // The user's draft had #dfe3ea here, which is under 5 dE from
        // `surface-raised` (#eceef2): a hovered row would not show.
        let mut text = String::from_utf8(fixture("Daylight.light.json")).expect("UTF-8");
        assert!(text.contains("#d8dde6"));
        text = text.replace("#d8dde6", "#dfe3ea");
        let (tokens, _) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(
            theme::resolve(&tokens).map(|_| ()),
            Err(Refusal::SelectionHidden)
        );
    }

    // ---- light and dark pairs, duplicates, single looks --------------------

    /// A light theme file: the Daylight fixture.
    fn light_doc() -> Vec<u8> {
        fixture("Daylight.light.json")
    }

    fn looks(loaded: &Loaded) -> Vec<(String, bool, bool)> {
        loaded
            .themes
            .iter()
            .map(|t| (t.name.clone(), t.dark, t.both_modes))
            .collect()
    }

    #[test]
    fn a_dark_and_a_light_file_of_one_name_are_one_pair() {
        let dir = Dir::new();
        dir.put("Nord.dark.json", built_in_doc_with(&[]).as_bytes());
        dir.put("Nord.light.json", &light_doc());
        let loaded = timed(|| load_dir(&dir.0));
        assert_eq!(
            looks(&loaded),
            [("Nord".into(), true, true), ("Nord".into(), false, true)]
        );
        assert!(loaded.report.is_empty(), "{:?}", lines(&loaded));
        // A plain `Nord.json` that is light pairs with `Nord.dark.json` too.
        let dir = Dir::new();
        dir.put("Nord.dark.json", built_in_doc_with(&[]).as_bytes());
        dir.put("Nord.json", &light_doc());
        let loaded = load_dir(&dir.0);
        assert_eq!(
            looks(&loaded),
            [("Nord".into(), true, true), ("Nord".into(), false, true)]
        );
    }

    #[test]
    fn names_differing_only_in_capitals_are_one_theme() {
        // Same mode: a duplicate, whatever the capitals.
        let dir = Dir::new();
        dir.put("Nord.dark.json", built_in_doc_with(&[]).as_bytes());
        dir.put("nord.dark.json", built_in_doc_with(&[]).as_bytes());
        let loaded = load_dir(&dir.0);
        assert_eq!(looks(&loaded), [("Nord".into(), true, false)]);
        assert_eq!(lines(&loaded).len(), 1, "{:?}", lines(&loaded));
        assert!(lines(&loaded)[0].ends_with("same name and mode as another file"));
        // Other mode: a pair, under the spelling that sorts first.
        let dir = Dir::new();
        dir.put("Nord.dark.json", built_in_doc_with(&[]).as_bytes());
        dir.put("nord.light.json", &light_doc());
        let loaded = load_dir(&dir.0);
        assert_eq!(
            looks(&loaded),
            [("Nord".into(), true, true), ("Nord".into(), false, true)]
        );
        assert!(loaded.report.is_empty(), "{:?}", lines(&loaded));
    }

    #[test]
    fn the_same_name_and_mode_twice_is_a_duplicate_even_when_a_pair_exists() {
        let dir = Dir::new();
        dir.put("Nord.dark.json", built_in_doc_with(&[]).as_bytes());
        dir.put(
            "Nord.json",
            built_in_doc_with(&[("ink", "#eeeeee")]).as_bytes(),
        );
        dir.put("Nord.light.json", &light_doc());
        let loaded = load_dir(&dir.0);
        assert_eq!(
            looks(&loaded),
            [("Nord".into(), true, true), ("Nord".into(), false, true)]
        );
        assert_eq!(hex(loaded.themes[0].palette.text), "#efebfa");
        assert_eq!(lines(&loaded), ["Nord: same name and mode as another file"]);
    }

    #[test]
    fn single_looks_have_no_second_mode() {
        for (file, bytes, dark) in [
            ("Nord.dark.json", built_in_doc_with(&[]).into_bytes(), true),
            ("Nord.json", built_in_doc_with(&[]).into_bytes(), true),
            ("Nord.json", light_doc(), false),
            ("Daylight.light.json", light_doc(), false),
        ] {
            let dir = Dir::new();
            dir.put(file, &bytes);
            let loaded = load_dir(&dir.0);
            assert_eq!(loaded.themes.len(), 1, "{file}");
            assert_eq!(loaded.themes[0].dark, dark, "{file}");
            assert!(!loaded.themes[0].both_modes, "{file}");
            assert!(loaded.report.is_empty(), "{file}: {:?}", lines(&loaded));
        }
    }

    #[test]
    fn a_pair_with_one_refused_side_loads_the_other_alone() {
        let dir = Dir::new();
        dir.put("Nord.dark.json", built_in_doc_with(&[]).as_bytes());
        let clash = String::from_utf8(light_doc())
            .expect("UTF-8")
            .replace("\"#b3112f\"", "\"#14181d\"");
        dir.put("Nord.light.json", clash.as_bytes());
        let loaded = load_dir(&dir.0);
        assert_eq!(looks(&loaded), [("Nord".into(), true, false)]);
        assert_eq!(
            lines(&loaded),
            ["Nord (light): status colours look too alike"]
        );
    }

    #[test]
    fn a_name_that_disagrees_with_the_background_is_refused_for_both_sides() {
        let dir = Dir::new();
        dir.put("X.light.json", built_in_doc_with(&[]).as_bytes());
        dir.put("Y.dark.json", &light_doc());
        let loaded = load_dir(&dir.0);
        assert!(loaded.themes.is_empty());
        assert_eq!(
            lines(&loaded),
            [
                "X (light): says light but its background is dark",
                "Y (dark): says dark but its background is light"
            ]
        );
    }

    #[test]
    fn a_utf8_byte_order_mark_is_skipped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(built_in_doc_with(&[]).as_bytes());
        assert_eq!(parse(&bytes).expect("parsed").0, BUILT_IN);
        let dir = Dir::new();
        dir.put("Bom.json", &bytes);
        assert_eq!(load_dir(&dir.0).themes.len(), 1);
        // Only one, and only at the start.
        let mut twice = vec![0xEF, 0xBB, 0xBF, 0xEF, 0xBB, 0xBF];
        twice.extend_from_slice(built_in_doc_with(&[]).as_bytes());
        assert_eq!(parse(&twice), Err(FileProblem::NotJson));
    }

    #[test]
    fn hex_values_are_strict() {
        let one = |v: &str| {
            parse(built_in_doc_with(&[("ink", v)]).as_bytes())
                .expect("a theme")
                .1
                .not_understood
        };
        assert_eq!(one("#aabbcc"), 0);
        assert_eq!(one("#AABBCC"), 0);
        assert_eq!(one("#aabbccff"), 0);
        assert_eq!(one("#aabbccFF"), 0);
        for bad in [
            "#aabbcc00",
            "#aabbccfe",
            "aabbcc",
            "#aabbc",
            "#aabbccd",
            "#gabbcc",
            " #aabbcc",
            "#aabbcc ",
        ] {
            assert_eq!(one(bad), 1, "{bad:?}");
        }
    }

    #[test]
    fn unknown_names_are_counted_and_unused_design_names_are_not() {
        let text = r##"{"color":{"$type":"color","ink":{"$value":"#ffffff"},
            "line":{"$value":"#2e2748"},"ink-faint":{"$value":"#8a83a8"},
            "phosphor":{"$value":"#f3eeff"},"violet-deep":{"$value":"#4b23b8"},
            "mystery":{"$value":"#010203"},"other":1}}"##;
        let (tokens, notes) = parse(text.as_bytes()).expect("a theme");
        assert_eq!(hex(tokens.ink), "#ffffff");
        assert_eq!(
            notes,
            ParseNotes {
                not_understood: 0,
                unknown: 2,
                aliases_ignored: 0
            }
        );
    }

    #[test]
    fn a_value_may_be_a_string_or_an_object_with_hex_and_the_last_duplicate_wins() {
        let text = r##"{"color":{"ink":{"$value":{"hex":"#112233"}},"ink":{"$value":"#445566"}}}"##;
        assert_eq!(
            hex(parse(text.as_bytes()).expect("a theme").0.ink),
            "#445566"
        );
        let text = r##"{"color":{"ink":{"$value":{"hex":"#112233"}}}}"##;
        assert_eq!(
            hex(parse(text.as_bytes()).expect("a theme").0.ink),
            "#112233"
        );
        let text = r##"{"color":{"ink":{"$value":{"colorSpace":"srgb","hex":"#112233"}}}}"##;
        assert_eq!(
            hex(parse(text.as_bytes()).expect("a theme").0.ink),
            "#112233"
        );
        assert_eq!(parse(b"not json"), Err(FileProblem::NotJson));
        assert_eq!(parse(b""), Err(FileProblem::NotJson));
    }

    #[test]
    fn only_json_files_are_themes_and_a_missing_folder_is_empty() {
        let dir = Dir::new();
        dir.put("notes.txt", b"hello");
        dir.put("Theme.JSON", built_in_doc_with(&[]).as_bytes());
        std::fs::create_dir(dir.0.join("Sub.json")).expect("a directory");
        let loaded = load_dir(&dir.0);
        assert!(loaded.themes.is_empty());
        assert_eq!(
            lines(&loaded),
            ["Sub: not a regular file"],
            "a directory named like a theme"
        );
        let missing = load_dir(&dir.0.join("nope"));
        assert_eq!(missing, Loaded::default());
    }

    #[test]
    fn more_than_the_entry_cap_is_said() {
        let dir = Dir::new();
        for i in 0..MAX_ENTRIES + 3 {
            dir.put(&format!("n{i}.txt"), b"");
        }
        let loaded = timed(|| load_dir(&dir.0));
        assert!(loaded.themes.is_empty());
        assert_eq!(
            lines(&loaded),
            ["more than 1,024 entries, the rest not looked at"]
        );
    }

    #[test]
    fn the_report_is_cut_to_the_lines_asked_for() {
        let dir = Dir::new();
        for i in 0..8 {
            dir.put(&format!("Bad{i}.json"), b"{");
        }
        let loaded = load_dir(&dir.0);
        let shown = loaded.shown(5);
        assert_eq!(shown.len(), 6);
        assert_eq!(shown[0], "Bad0: not valid JSON");
        assert_eq!(shown[5], "and 3 more");
        assert_eq!(loaded.shown(10).len(), 8);
    }

    #[cfg(unix)]
    #[test]
    fn hostile_a_symlink_to_a_valid_theme_is_refused() {
        let dir = Dir::new();
        let target = dir.put("real-target.txt", built_in_doc_with(&[]).as_bytes());
        std::os::unix::fs::symlink(&target, dir.0.join("Link.json")).expect("a symlink");
        let loaded = load_dir(&dir.0);
        assert!(loaded.themes.is_empty());
        assert_eq!(lines(&loaded), ["Link: not a regular file"]);
    }

    #[cfg(unix)]
    #[test]
    fn hostile_a_fifo_is_refused_without_being_opened() {
        let dir = Dir::new();
        let fifo = dir.0.join("x.json");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo is needed for this test");
        assert!(made.success());
        let (tx, rx) = std::sync::mpsc::channel();
        let path = dir.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load_dir(&path));
        });
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(loaded) => {
                assert!(loaded.themes.is_empty());
                assert_eq!(lines(&loaded), ["x: not a regular file"]);
            }
            Err(_) => {
                // Release the blocked reader so the thread ends, then fail.
                let _ = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&fifo);
                panic!("load_dir hung on a FIFO");
            }
        }
    }

    // ---- loading off the UI thread ----

    /// Runs `future` to its end on a thread of its own and sends the result.
    fn run_apart<T: Send + 'static>(
        future: impl std::future::Future<Output = T> + Send + 'static,
    ) -> std::sync::mpsc::Receiver<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(iced::futures::executor::block_on(future));
        });
        rx
    }

    #[test]
    fn a_stalled_loader_does_not_stall_the_caller() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let seen_on = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen = seen_on.clone();
        let result = run_apart(load_off_thread_with(move || {
            *seen.lock().expect("test mutex") = std::thread::current().name().map(str::to_owned);
            let _ = wait.recv();
            Loaded::default()
        }));
        // The loader is stuck, and this thread still runs.
        assert!(
            result
                .recv_timeout(Duration::from_millis(100))
                .is_err_and(|e| e == std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        release.send(()).expect("the loader waits");
        let loaded = result
            .recv_timeout(Duration::from_secs(10))
            .expect("the result arrives once released");
        assert_eq!(loaded, Some(Loaded::default()));
        assert_eq!(
            seen_on.lock().expect("test mutex").as_deref(),
            Some("twicecat-themes")
        );
    }

    #[test]
    fn a_loader_that_panics_gives_none() {
        let result = run_apart(load_off_thread_with(|| -> Loaded { panic!("test panic") }));
        assert_eq!(
            result.recv_timeout(Duration::from_secs(10)).expect("done"),
            None
        );
    }

    #[test]
    fn load_off_thread_reads_the_folder() {
        let dir = Dir::new();
        dir.put(
            "Nord.json",
            built_in_doc_with(&[("ink", "#eeeeee")]).as_bytes(),
        );
        let result = run_apart(load_off_thread(dir.0.clone()));
        let loaded = result
            .recv_timeout(Duration::from_secs(10))
            .expect("done")
            .expect("a result");
        assert_eq!(loaded, load_dir(&dir.0));
        assert_eq!(loaded.themes.len(), 1);
    }

    // ---- docs/themes.md stays in sync ----

    const DOC: &str = include_str!("../../../docs/themes.md");

    #[test]
    fn the_doc_shows_daylight_exactly_as_the_fixture_has_it() {
        let block = DOC
            .split("```json\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("a json block in docs/themes.md");
        let fixture = include_str!("../tests/fixtures/themes/Daylight.light.json");
        assert_eq!(block, fixture);
    }

    #[test]
    fn the_doc_lists_the_tokens_in_order() {
        let section = DOC
            .split("## The 15 tokens")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("the token section");
        let listed: Vec<&str> = section
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .filter_map(|rest| rest.split('`').next())
            .collect();
        assert_eq!(listed, TOKEN_NAMES);
        // And the built-in values in the table are the real ones.
        for (name, color) in TOKEN_NAMES.iter().zip(BUILT_IN.as_array()) {
            let [r, g, b, _] = color.into_rgba8();
            let row = section
                .lines()
                .find(|l| l.starts_with(&format!("| `{name}`")))
                .expect("a row");
            assert!(row.contains(&format!("`#{r:02x}{g:02x}{b:02x}`")), "{row}");
        }
    }

    #[test]
    fn the_doc_names_the_three_folders_and_says_nothing_it_should_not() {
        for path in [
            "~/.config/twicecat/themes/Daylight.light.json",
            "$XDG_CONFIG_HOME/twicecat/themes/",
            "~/Library/Application Support/io.github.ga_lep.Twicecat/themes/Daylight.light.json",
            "%APPDATA%\\ga_lep\\Twicecat\\config\\themes\\Daylight.light.json",
        ] {
            assert!(DOC.contains(path), "{path}");
        }
        let lower = DOC.to_lowercase();
        for banned in ["violet", "dark look only", "dark-only", "dark only"] {
            assert!(!lower.contains(banned), "{banned}");
        }
    }

    #[test]
    fn the_doc_explains_pairs_and_the_choice() {
        for wanted in [
            ".light.json",
            ".dark.json",
            "Follow the system",
            "Reload themes",
            "Open themes folder",
            "--data-dir",
            "--ephemeral",
            "Twicecat-copy.json",
        ] {
            assert!(DOC.contains(wanted), "{wanted}");
        }
    }
}
