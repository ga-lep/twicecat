//! Crash reports, local only (Phase 7 plan Task 10, Q8 a). A panic hook
//! writes `crash-<UTC time>.txt` in the data directory. The report holds the
//! version, the OS, the CPU level the build targets, the GPU backend in use,
//! the panicking thread, the panic's words and location, and the frames of a
//! backtrace that are in our crates. The words and the thread name are
//! scrubbed: no Matrix IDs, addresses, host names, e-mail addresses, home
//! paths or long tokens. Nothing is sent: the next start shows a bar with
//! **Show report**, **Open an issue** (a new-issue page on GitHub with the
//! report filled in, which the user reads before sending) and **Delete**.

use std::backtrace::Backtrace;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The most reports kept; the oldest go first.
const KEEP: usize = 5;
/// Our crates' symbol prefix, for the backtrace frames kept.
const OURS: &str = "twicecat";
/// The longest issue body put in a link (GitHub refuses very long URLs).
const ISSUE_BODY_MAX: usize = 6_000;

/// What the hook adds that only the app knows (the GPU backend).
static CONTEXT: Mutex<Option<String>> = Mutex::new(None);
/// One report per run: a panic storm writes one file.
static WRITTEN: AtomicBool = AtomicBool::new(false);

/// The GPU backend in use, for the next report (`None`: none).
pub fn set_gpu_backend(backend: Option<String>) {
    *CONTEXT.lock().unwrap_or_else(|p| p.into_inner()) = backend;
}

/// Installs the hook: the default one still prints, then the report is
/// written in `dir` (best effort; a failed write changes nothing).
pub fn install(dir: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if WRITTEN.swap(true, Ordering::SeqCst) {
            return;
        }
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let location = info
            .location()
            .map(|l| format!("{}:{}", our_path(l.file()), l.line()));
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let gpu = CONTEXT.lock().ok().and_then(|c| c.clone());
        let report = Report {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            cpu_level: cpu_level(),
            gpu,
            thread,
            message,
            location,
            backtrace: Backtrace::force_capture().to_string(),
        };
        let _ = write(&dir, &report.render(), &file_name(chrono::Utc::now()));
    }));
}

/// The raw facts of one panic.
pub struct Report {
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub cpu_level: &'static str,
    pub gpu: Option<String>,
    pub thread: String,
    pub message: String,
    pub location: Option<String>,
    /// `Backtrace`'s text.
    pub backtrace: String,
}

impl Report {
    /// The report's text, scrubbed.
    pub fn render(&self) -> String {
        let frames = our_frames(&self.backtrace);
        let mut text = format!(
            "Twicecat {} crash report\n\nVersion: {}\nOS: {} {}\nCPU level: {}\nGPU backend: {}\nThread: {}\nPanic: {}\nAt: {}\n\nBacktrace (Twicecat's frames only):\n",
            self.version,
            self.version,
            self.os,
            self.arch,
            self.cpu_level,
            self.gpu.as_deref().map_or_else(|| "none".to_owned(), scrub),
            scrub(&self.thread),
            scrub(&self.message),
            self.location.as_deref().unwrap_or("unknown"),
        );
        if frames.is_empty() {
            text.push_str("  (none: this build has no symbols)\n");
        }
        for frame in frames {
            text.push_str("  ");
            text.push_str(&frame);
            text.push('\n');
        }
        text
    }
}

/// The level the build was compiled for.
fn cpu_level() -> &'static str {
    if cfg!(target_feature = "avx512f") {
        "x86-64-v4"
    } else if cfg!(target_feature = "avx2") {
        "x86-64-v3"
    } else if cfg!(target_arch = "x86_64") {
        "x86-64"
    } else {
        std::env::consts::ARCH
    }
}

/// `crash-YYYYMMDD-HHMMSS.txt`.
pub fn file_name(at: chrono::DateTime<chrono::Utc>) -> String {
    format!("crash-{}.txt", at.format("%Y%m%d-%H%M%S"))
}

/// A path from our sources from `crates/` on; anything else is not shown.
fn our_path(file: &str) -> String {
    let file = file.replace('\\', "/");
    match file.find("crates/") {
        Some(at) => file[at..].to_owned(),
        None if file.starts_with("/rustc/") || file.contains("/library/") => "<std>".to_owned(),
        None => "<dependency>".to_owned(),
    }
}

/// The frames of `Backtrace`'s text that are in our crates, as
/// `symbol (crates/…:line)`. Symbols of other crates, and the paths of
/// anything outside `crates/`, never reach the report.
pub fn our_frames(backtrace: &str) -> Vec<String> {
    let mut frames = Vec::new();
    let mut lines = backtrace.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let Some((index, symbol)) = line.split_once(": ") else {
            continue;
        };
        if !index.chars().all(|c| c.is_ascii_digit()) || !symbol.starts_with(OURS) {
            continue;
        }
        let at = lines
            .peek()
            .and_then(|next| next.trim().strip_prefix("at "))
            .map(|path| our_path(path.trim()));
        // The symbol itself is ours (`twicecat_*::…`); a closure or generic
        // argument could name a type, so it is cut at the first `<`.
        let symbol = symbol
            .split('<')
            .next()
            .unwrap_or(symbol)
            .trim_end_matches("::");
        frames.push(match at {
            Some(at) => format!("{symbol} ({at})"),
            None => symbol.to_owned(),
        });
    }
    frames
}

/// File extensions that end a word without making it a host name.
const NOT_HOSTS: [&str; 12] = [
    "rs", "toml", "json", "txt", "md", "so", "dll", "exe", "png", "onnx", "ini", "html",
];

/// Removes what could identify a person, a room or a server, word by word.
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let trim: &[char] = &[
            '"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', ';', '<', '>',
        ];
        let start = word.len() - word.trim_start_matches(trim).len();
        let core = word.trim_matches(trim);
        let end = start + core.len();
        out.push_str(&word[..start]);
        out.push_str(&scrub_word(core));
        out.push_str(&word[end..]);
    }
    out
}

fn scrub_word(word: &str) -> String {
    let bare = word.trim_end_matches(['.', ':', '!', '?']);
    let tail = &word[bare.len()..];
    let replaced = if bare.contains("://") {
        Some("<address>")
    } else if bare.len() > 1 && bare.starts_with(['@', '!', '#', '$', '+']) && bare.contains(':') {
        Some("<id>")
    } else if bare.contains('@') && bare.contains('.') {
        Some("<email>")
    } else if is_home_path(bare) {
        Some("<home path>")
    } else if is_host(bare) {
        Some("<host>")
    } else if is_token(bare) {
        Some("<token>")
    } else {
        None
    };
    match replaced {
        Some(r) => format!("{r}{tail}"),
        None => word.to_owned(),
    }
}

fn is_home_path(word: &str) -> bool {
    let w = word.replace('\\', "/");
    let lower = w.to_ascii_lowercase();
    lower.starts_with("/home/")
        || lower.starts_with("/users/")
        || lower.starts_with("/var/home/")
        || lower.starts_with("~/")
        || (lower.len() > 3 && lower[1..].starts_with(":/users/"))
}

fn is_host(word: &str) -> bool {
    let Some((_, tld)) = word.rsplit_once('.') else {
        return false;
    };
    let ok_chars = word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':');
    ok_chars
        && !word.starts_with('.')
        && !word.contains("..")
        && tld.len() >= 2
        && tld
            .split(':')
            .next()
            .is_some_and(|t| t.chars().all(|c| c.is_ascii_alphabetic()))
        && !NOT_HOSTS.contains(&tld.to_ascii_lowercase().as_str())
}

/// A long run of base64 or hex: an access token, a key, a session.
fn is_token(word: &str) -> bool {
    word.len() >= 20
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c))
        && word.chars().any(|c| c.is_ascii_digit())
}

/// Writes `text` as `dir/name` (private on Unix) and keeps the newest
/// [`KEEP`] reports.
pub fn write(dir: &Path, text: &str, name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    use std::io::Write as _;
    options.open(&path)?.write_all(text.as_bytes())?;
    let mut all = reports(dir);
    while all.len() > KEEP {
        let _ = std::fs::remove_file(all.remove(0));
    }
    Ok(())
}

/// The reports in `dir`, oldest first (their names sort by time).
pub fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        })
        .collect();
    all.sort();
    all
}

/// The newest report left by an earlier run, to show at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReport {
    /// Its file.
    pub path: PathBuf,
    /// Its text (at most 64 KiB read).
    pub text: String,
}

/// The newest report in `dir`, if any.
pub fn newest(dir: &Path) -> Option<PendingReport> {
    let path = reports(dir).pop()?;
    let bytes = std::fs::read(&path).ok()?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)]).into_owned();
    Some(PendingReport { path, text })
}

/// Removes every report in `dir`.
pub fn delete_all(dir: &Path) {
    for path in reports(dir) {
        let _ = std::fs::remove_file(path);
    }
}

/// GitHub's new-issue page with `text` as its body (cut to fit a link).
pub fn issue_url(text: &str) -> String {
    let repository = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let mut body: String = text.chars().take(ISSUE_BODY_MAX).collect();
    if body.len() < text.len() {
        body.push_str("\n…(cut; the whole report is in the file)");
    }
    let title = format!("Crash report (Twicecat {})", env!("CARGO_PKG_VERSION"));
    format!(
        "{repository}/issues/new?title={}&body={}",
        percent(&title),
        percent(&format!("```\n{body}\n```"))
    )
}

/// Percent-encodes everything but unreserved characters (RFC 3986).
fn percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The bar's words.
pub const BAR_TEXT: &str =
    "Twicecat ran into an error last time and wrote a report. Nothing was sent.";

/// What the crash bar asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrashMessage {
    /// Open the report in the system's text viewer.
    Show,
    /// GitHub's new-issue page with the report filled in.
    OpenIssue,
    /// Delete the reports; the bar goes.
    Delete,
}

impl crate::state::App {
    /// The report an earlier run left, and where reports go.
    pub fn set_crash_reports(&mut self, dir: PathBuf, pending: Option<PendingReport>) {
        self.crash_dir = Some(dir);
        self.crash = pending;
    }

    pub(crate) fn on_crash(&mut self, message: CrashMessage) -> iced::Task<crate::state::Message> {
        use crate::state::Message;
        match message {
            CrashMessage::Show => match &self.crash {
                Some(report) => {
                    let path = report.path.clone();
                    iced::Task::perform(async move { open::that_detached(&path).is_ok() }, |ok| {
                        Message::LinkOpened { ok }
                    })
                }
                None => iced::Task::none(),
            },
            CrashMessage::OpenIssue => match &self.crash {
                Some(report) => match crate::links::check_link_url(&issue_url(&report.text)) {
                    Ok(url) => crate::state::open_link_task(url),
                    Err(_) => iced::Task::none(),
                },
                None => iced::Task::none(),
            },
            CrashMessage::Delete => {
                if let Some(dir) = &self.crash_dir {
                    delete_all(dir);
                }
                self.crash = None;
                iced::Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_addresses_hosts_paths_and_tokens_are_scrubbed() {
        let cases = [
            (
                "no room !abcDEF:matrix.example.org found",
                "no room <id> found",
            ),
            ("user @alice:example.org left", "user <id> left"),
            ("event $Abc123_xyz:server.net, sorry", "event <id>, sorry"),
            ("alias #general:chat.example.com.", "alias <id>."),
            (
                "GET https://matrix.example.org/_matrix/client/v3/sync?since=s1 failed",
                "GET <address> failed",
            ),
            ("mail bob@example.org now", "mail <email> now"),
            (
                "open /home/william/.local/share/twicecat/x.db",
                "open <home path>",
            ),
            (r"open C:\Users\willi\AppData\x.db", "open <home path>"),
            ("server matrix.org refused", "server <host> refused"),
            (
                "server chat.example.com:8448 refused",
                "server <host> refused",
            ),
            (
                "token syt_YWxpY2U_aBcDeFgHiJkLmNoPqRsT_1a2B3c leaked",
                "token <token> leaked",
            ),
            ("(\"@carol:example.org\")", "(\"<id>\")"),
        ];
        for (raw, want) in cases {
            assert_eq!(scrub(raw), want, "{raw}");
        }
        // Words of the program itself stay: they say what went wrong.
        for kept in [
            "index out of bounds: the len is 3 but the index is 7",
            "called `Option::unwrap()` on a `None` value",
            "crates/app/src/state.rs:120",
            "failed to decode model enc.onnx",
            "config.toml is missing",
        ] {
            assert_eq!(scrub(kept), kept, "{kept}");
        }
    }

    const BACKTRACE: &str = "   0: std::backtrace_rs::backtrace::libunwind::trace
             at /rustc/abc/library/std/src/../../backtrace/src/backtrace/libunwind.rs:117:9
   1: twicecat_app::state::App::update
             at /home/william/RustroverProjects/twicecat/crates/app/src/state.rs:1200:13
   2: core::ops::function::FnOnce::call_once
             at /rustc/abc/library/core/src/ops/function.rs:250:5
   3: twicecat_core::engine::Engine::handle::{{closure}}<matrix_sdk::Client>
             at C:\\Users\\willi\\RustroverProjects\\twicecat\\crates\\core\\src\\engine\\mod.rs:1400:9
   4: matrix_sdk::client::Client::sync
             at /home/william/.cargo/registry/src/index/matrix-sdk-0.19.1/src/client/mod.rs:10:1
   5: twicecat_ptt::spawn
";

    #[test]
    fn only_our_frames_are_kept_with_paths_from_crates_on() {
        assert_eq!(
            our_frames(BACKTRACE),
            [
                "twicecat_app::state::App::update (crates/app/src/state.rs:1200:13)",
                "twicecat_core::engine::Engine::handle::{{closure}} (crates/core/src/engine/mod.rs:1400:9)",
                "twicecat_ptt::spawn",
            ]
        );
    }

    #[test]
    fn a_report_says_what_it_should_and_nothing_more() {
        let report = Report {
            version: "0.1.0",
            os: "linux",
            arch: "x86_64",
            cpu_level: "x86-64-v3",
            gpu: Some("any GPU · WebGPU on Vulkan".into()),
            thread: "sync @alice:example.org".into(),
            message: "no member @bob:example.org in !room:example.org at https://example.org/x"
                .into(),
            location: Some("crates/app/src/state.rs:1200".into()),
            backtrace: BACKTRACE.into(),
        }
        .render();
        for want in [
            "Version: 0.1.0",
            "OS: linux x86_64",
            "CPU level: x86-64-v3",
            "GPU backend: any GPU · WebGPU on Vulkan",
            "Thread: sync <id>",
            "Panic: no member <id> in <id> at <address>",
            "At: crates/app/src/state.rs:1200",
            "twicecat_app::state::App::update (crates/app/src/state.rs:1200:13)",
        ] {
            assert!(report.contains(want), "{want} in\n{report}");
        }
        for never in [
            "alice",
            "bob",
            "example.org",
            "william",
            "willi",
            "matrix_sdk",
            "/rustc/",
            "registry",
        ] {
            assert!(!report.contains(never), "{never} in\n{report}");
        }
    }

    #[test]
    fn reports_are_written_kept_to_five_and_read_back_newest_first() {
        let root = std::env::temp_dir().join(format!("twicecat-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = TempDir(root);
        assert_eq!(newest(dir.path()), None);
        for second in 0..7 {
            let at = chrono::DateTime::from_timestamp(1_800_000_000 + second, 0).unwrap();
            write(dir.path(), &format!("report {second}"), &file_name(at)).unwrap();
        }
        let left = reports(dir.path());
        assert_eq!(left.len(), KEEP);
        let pending = newest(dir.path()).unwrap();
        assert_eq!(pending.text, "report 6");
        assert!(
            pending.path.ends_with("crash-20270115-080006.txt"),
            "{:?}",
            pending.path
        );
        // Other files in the data directory are left alone.
        std::fs::write(dir.path().join("settings.json"), "{}").unwrap();
        delete_all(dir.path());
        assert!(reports(dir.path()).is_empty());
        assert!(dir.path().join("settings.json").exists());
    }

    /// A folder removed at the end of the test.
    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_issue_link_carries_the_report_encoded_and_cut_to_fit() {
        let url = issue_url("Panic: x & y = 1?\nAt: crates/a.rs:1");
        assert!(url.starts_with(
            "https://github.com/ga-lep/twicecat/issues/new?title=Crash%20report%20%28Twicecat%20"
        ));
        assert!(url.contains("&body=%60%60%60%0APanic%3A%20x%20%26%20y%20%3D%201%3F%0AAt%3A%20crates%2Fa.rs%3A1%0A%60%60%60"));
        assert!(crate::links::check_link_url(&url).is_ok());
        let long = issue_url(&"a".repeat(20_000));
        assert!(long.len() < 7_000, "{}", long.len());
        assert!(long.contains("cut"));
    }

    #[test]
    fn the_bar_shows_an_earlier_report_until_deleted() {
        use crate::bridge::EventsSlot;
        use crate::state::{App, Message, Startup};
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let (_events_tx, events_rx) = tokio::sync::mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let root = std::env::temp_dir().join(format!("twicecat-crash-bar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = TempDir(root);
        write(dir.path(), "report", "crash-20270115-080000.txt").unwrap();
        app.set_crash_reports(dir.path().to_path_buf(), newest(dir.path()));
        assert_eq!(app.crash.as_ref().map(|r| r.text.as_str()), Some("report"));
        // Show and Open an issue leave it; Delete removes the files too.
        let _ = app.update(Message::Crash(CrashMessage::Show));
        let _ = app.update(Message::Crash(CrashMessage::OpenIssue));
        assert!(app.crash.is_some());
        let _ = app.update(Message::Crash(CrashMessage::Delete));
        assert!(app.crash.is_none());
        assert!(reports(dir.path()).is_empty());
    }
}
