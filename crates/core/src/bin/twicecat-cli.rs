//! `twicecat-cli`: a small command-line front end on the core's command and
//! event API. It never touches the Matrix SDK directly.
//!
//! Message bodies are printed to stdout because that is the program's output;
//! nothing here sends a token, password, or message body to the log.

#[path = "twicecat-cli/calls/mod.rs"]
mod calls;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use twicecat_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, Command, CoreEvent, ListDiff, MessageText, Password, RoomId,
    RoomSummary, SecurityStatus, SyncState, TimelineItem, TimelineItemKind, VirtualAudio,
};
use twicecat_core::logging;
use twicecat_core::paths::DataPaths;
use twicecat_core::secrets::{
    InMemorySecretStore, KEYCHAIN_SERVICE, KeyringSecretStore, SecretStore,
};
use twicecat_core::{Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE};

const HELP: &str = "\
twicecat-cli: headless Matrix client on the Twicecat core

USAGE:
    twicecat-cli [--data-dir <path> | --ephemeral] [--homeserver <url>] [--password-stdin]
                [--silent-audio]

OPTIONS:
    --data-dir <path>     Store session data here (default: the OS data directory;
                          on Linux ~/.local/share/twicecat)
    --homeserver <url>    Use this homeserver (URL or server name) instead of
                          discovering it from the user ID
    --ephemeral           Keep the secret store in memory and use a temporary
                          data directory that is deleted on exit. On exit
                          (/quit, end of input, or Ctrl-C) the session is
                          logged out on the server first.
    --password-stdin      Read the password from the next stdin line instead of
                          prompting on the terminal. If stdin is a terminal the
                          password is echoed; the CLI warns about that. Use it
                          only with piped input.
    --silent-audio        Calls send silence and play nothing; the microphone
                          and speakers are never opened (tests, scripts)
    --help                Show this help

COMMANDS (type them at the prompt):
    /login                Log in with a username and password
    /oauth <server>       Log in through OAuth 2.0 (prints the URL to open)
    /rooms                List rooms: index, name, unread counts
    /select <index|id>    Open a room (index from /rooms, or a room ID)
    /send <text>          Send a message to the open room
    /logout               Log out and delete the stored session
    /quit                 Shut down

CALLS (voice, MatrixRTC; joining opens the microphone and speakers):
    /call [n|id]          Join the call in the open room (or the given one, or
                          answer the last ring)
    /leave                Hang up
    /decline [n|id]       Decline the ring (the last one by default)
    /silence              Stop the ringtone (the call still rings)
    /ringtone [on|off]    Play a ringtone for direct calls
    /who                  List the call's participants
    /mute [on|off]        Mute the microphone (toggles without an argument)
    /deafen [on|off]      Hear nothing and mute the microphone
    /ptt [on|off]         In push-to-talk mode, start or stop transmitting.
                          A terminal cannot see a key being released, so this
                          toggles instead of holding
    /vol <n|user> <pct>   Playback volume of a participant (0-200 %, kept)
    /clean [on|off]       Clean the voices you hear with DeepFilterNet3 (up to
                          4 talking at once; about 30 ms later; off by default)
    /clean limit <dB>     Most noise removed from them (0-100)
    /clean <n|user> always|never|auto
                          Per person (kept); always comes first
    /clean skip neural|any|never
                          Leave alone voices their sender cleans already:
                          with a neural suppressor (default), any, or none
    /clean announce on|off
                          Tell others which suppressor cleans your voice
                          (default on), so they need not clean it again
    /clean outside on|off
                          Your microphone is cleaned before Twicecat (NVIDIA
                          Broadcast, a Krisp app): announced as cleaned
    /clean gpu on|off     Clean them on a GPU (needs a gpu-enhancement build;
                          every talker at once, little CPU; the CPU takes
                          over if no GPU keeps up)

AUDIO SETTINGS (saved; take effect at the next call or mic test):
    /audio                Show the audio settings
    /devices              List microphones and speakers
    /input <n|default>    Choose the microphone (index from /devices)
    /output <n|default>   Choose the speakers
    /ns off|rnnoise|webrtc [low|moderate|high|very_high]|deepfilter [dB] [gpu]
                          Noise suppression: one of them, or off (they do
                          not stack; processing outside Twicecat, such as
                          NVIDIA Broadcast, is up to you). DeepFilterNet with
                          `gpu` runs on the GPU when one starts (see /gpu),
                          else on the CPU
    /gpu                  Which GPU backends can run speech enhancement here,
                          and where DeepFilterNet runs
    /aec [on|off]         Echo cancellation
    /aec system [on|off]  Also cancel other apps' sound (a game) picked up by
                          the microphone (Linux with PipeWire, Windows)
    /agc [on|off|<dB>]    Automatic gain; a number sets the most it may boost
                          (1 to 50 dB, 30 by default) and turns it on
    /gate auto|<dB>|ptt|toggle [ms]
                          Voice activity (automatic or above a level in dBFS),
                          push-to-talk, or toggle-to-talk (in the app a press
                          starts and the next stops), with a release tail in ms
    /mictest [loopback|stop]
                          Test the microphone: levels every second, and with
                          loopback hear yourself

LOGGING:
    Logs go to stderr; the default filter is `warn,twicecat_core=info`.
    Setting RUST_LOG to debug or trace can make the Matrix SDK and HTTP crates
    log sensitive request data (URLs, user and room identifiers). Even at the
    default filter, the SDK's own error lines can contain request URLs, so do
    not share or post stderr unreviewed.

A login that is cancelled or aborted during OAuth may leave a session on the
server; revoke stray sessions from your account settings.
";

const REPL_HINT: &str = "Commands: /login /oauth <server> /rooms /select <n|id> /send <text> \
     /logout /quit; calls: /call /leave /mute /deafen /ptt /who /audio (see --help)";

const START_FAILED_HINT: &str =
    "Could not restore or start a session. Use /login to log in, or /quit to exit.";

/// How long to wait for the engine to finish after `Shutdown`.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(20);

/// How long to wait for the server logout of an ephemeral session on exit.
const LOGOUT_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
struct Options {
    data_dir: Option<String>,
    homeserver: Option<String>,
    ephemeral: bool,
    password_stdin: bool,
    silent_audio: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Run(Options),
    Help,
}

#[derive(Debug, PartialEq)]
enum Repl {
    Login,
    OAuth(String),
    Rooms,
    Select(String),
    Send(String),
    Logout,
    Quit,
    /// A call or audio command, or its usage when the arguments are wrong.
    Call(Result<calls::CallCmd, &'static str>),
    Unknown(String),
    Text(String),
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut options = Options {
        data_dir: None,
        homeserver: None,
        ephemeral: false,
        password_stdin: false,
        silent_audio: false,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Parsed::Help),
            "--ephemeral" => options.ephemeral = true,
            "--password-stdin" => options.password_stdin = true,
            "--silent-audio" => options.silent_audio = true,
            "--data-dir" => {
                let value = iter.next().ok_or("--data-dir needs a path")?;
                options.data_dir = Some(value.clone());
            }
            "--homeserver" => {
                let value = iter.next().ok_or("--homeserver needs a value")?;
                options.homeserver = Some(value.clone());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if options.ephemeral && options.data_dir.is_some() {
        return Err("--ephemeral and --data-dir cannot be combined".to_owned());
    }
    Ok(Parsed::Run(options))
}

fn parse_repl(line: &str) -> Repl {
    let Some(rest) = line.strip_prefix('/') else {
        return Repl::Text(line.to_owned());
    };
    let (name, arg) = match rest.split_once(char::is_whitespace) {
        Some((name, arg)) => (name, arg),
        None => (rest, ""),
    };
    match name {
        "login" => Repl::Login,
        "rooms" => Repl::Rooms,
        "logout" => Repl::Logout,
        "quit" => Repl::Quit,
        "oauth" if !arg.trim().is_empty() => Repl::OAuth(arg.trim().to_owned()),
        "select" if !arg.trim().is_empty() => Repl::Select(arg.trim().to_owned()),
        "send" if !arg.is_empty() => Repl::Send(arg.to_owned()),
        _ => match calls::parse(name, arg) {
            Some(parsed) => Repl::Call(parsed),
            None => Repl::Unknown(format!("/{name}")),
        },
    }
}

/// Picks a room by its index in `rooms` or by its room ID.
fn resolve_room(rooms: &[RoomSummary], arg: &str) -> Option<RoomId> {
    if let Ok(index) = arg.parse::<usize>() {
        return rooms.get(index).map(|room| room.id.clone());
    }
    rooms
        .iter()
        .find(|room| room.id.as_str() == arg)
        .map(|room| room.id.clone())
}

/// Message printed instead of a new recovery key.
const RECOVERY_KEY_LINE: &str = "recovery key received, not shown";

/// One line for a security status: states only, nothing secret.
fn security_line(status: &SecurityStatus) -> String {
    format!(
        "[security] recovery: {:?}, device: {:?}, cross-signing: {:?}",
        status.recovery, status.device, status.cross_signing
    )
}

/// One non-secret line for a Phase 5 event, or `None` for any other event.
/// Previews, names, file names, keywords and bytes are never printed.
fn phase5_line(event: &CoreEvent) -> Option<String> {
    Some(match event {
        CoreEvent::Media {
            size,
            result: Ok(bytes),
            ..
        } => format!("[media] {size:?} received ({} bytes)", bytes.len()),
        CoreEvent::Media {
            size,
            result: Err(error),
            ..
        } => format!("[media] {size:?} failed: {error}"),
        CoreEvent::MediaSaved { .. } => "[media] saved".to_owned(),
        CoreEvent::MediaLimits { max_upload_bytes } => match max_upload_bytes {
            Some(max) => format!("[media] upload limit {max} bytes"),
            None => "[media] upload limit unknown".to_owned(),
        },
        CoreEvent::Notification(view) => format!("[notification] {:?}", view.kind),
        CoreEvent::NotificationSettings(settings) => format!(
            "[notifications] groups: {:?}, direct messages: {:?}, {} keywords",
            settings.group_default,
            settings.dm_default,
            settings.keywords.len()
        ),
        CoreEvent::UnreadTotals {
            notifications,
            highlights,
        } => format!("[unread] {notifications} notifications, {highlights} highlights"),
        CoreEvent::RoomCreated { .. } => "[rooms] created".to_owned(),
        CoreEvent::RoomJoined { .. } => "[rooms] joined".to_owned(),
        CoreEvent::RoomDetails(details) => {
            format!("[rooms] details (encrypted: {})", details.is_encrypted)
        }
        CoreEvent::RoomMembers {
            members, truncated, ..
        } => format!("[rooms] {} members (truncated: {truncated})", members.len()),
        CoreEvent::OwnProfile(profile) => {
            format!("[profile] avatar: {}", profile.avatar.is_some())
        }
        CoreEvent::Spaces(spaces) => format!("[spaces] {} spaces", spaces.len()),
        CoreEvent::ThreadDiff { diffs, .. } => format!("[thread] {} changes", diffs.len()),
        CoreEvent::ThreadPagination { state, .. } => format!("[thread] pagination {state:?}"),
        _ => return None,
    })
}

fn status_line(state: &SyncState) -> String {
    match state {
        SyncState::Idle => "[sync] idle".to_owned(),
        SyncState::Running => "[sync] running".to_owned(),
        SyncState::Offline => "[sync] offline".to_owned(),
        SyncState::Reconnecting { attempt } => format!("[sync] reconnecting (attempt {attempt})"),
        SyncState::Error(_) => "[sync] error".to_owned(),
        SyncState::Terminated => "[sync] stopped".to_owned(),
    }
}

/// Bidirectional and invisible formatting characters that can disguise text.
fn is_spoofing_format_char(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}')
}

/// Replaces control and bidi/invisible formatting characters so
/// server-controlled text cannot drive the terminal with escape sequences or
/// reorder and hide what is shown.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || is_spoofing_format_char(c) {
                '?'
            } else {
                c
            }
        })
        .collect()
}

/// What the terminal reader thread is asked to read next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    /// One REPL line (also the password line with `--password-stdin`).
    Line,
    /// One password with echo off.
    Password,
}

/// What the terminal reader thread produced.
#[derive(Debug, PartialEq, Eq)]
enum Input {
    Line(String),
    Password(Option<String>),
}

/// The terminal, as the single reader thread sees it.
trait Source {
    fn read_line(&mut self) -> Option<String>;
    fn read_password(&mut self) -> Option<String>;
}

struct TerminalSource;

impl Source for TerminalSource {
    fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }

    fn read_password(&mut self) -> Option<String> {
        rpassword::prompt_password("Password: ").ok()
    }
}

/// The only code that reads the terminal. It serves one request at a time, so
/// a password read and a line read can never overlap and a password line can
/// never be returned as a REPL line. Ends on EOF or when the receiver is gone.
fn reader_loop(
    requests: std::sync::mpsc::Receiver<Request>,
    out: mpsc::Sender<Input>,
    mut source: impl Source,
) {
    while let Ok(request) = requests.recv() {
        let input = match request {
            Request::Line => match source.read_line() {
                Some(line) => Input::Line(line),
                None => return,
            },
            Request::Password => Input::Password(source.read_password()),
        };
        if out.blocking_send(input).is_err() {
            return;
        }
    }
}

/// Decides when a line read may be outstanding, so the terminal has exactly
/// one reader at a time.
#[derive(Debug, Default)]
struct Gate {
    line_outstanding: bool,
    password_pending: bool,
}

impl Gate {
    /// A line request to issue now, if any. At most one is ever outstanding,
    /// and none while a password is being read.
    fn line_request(&mut self, reading: bool) -> Option<Request> {
        if reading && !self.line_outstanding && !self.password_pending {
            self.line_outstanding = true;
            Some(Request::Line)
        } else {
            None
        }
    }

    /// Routes reader output headed for the REPL: only lines get through.
    fn repl_line(&mut self, input: Input) -> Option<String> {
        match input {
            Input::Line(line) => {
                self.line_outstanding = false;
                Some(line)
            }
            Input::Password(_) => None,
        }
    }

    /// Takes the terminal for a password. `false` if a line read is still
    /// outstanding (the reader would race for the same input).
    fn begin_password(&mut self) -> bool {
        if self.line_outstanding {
            return false;
        }
        self.password_pending = true;
        true
    }

    fn end_password(&mut self) {
        self.password_pending = false;
    }
}

fn format_message(room: &str, sender: &str, body: &str) -> String {
    format!(
        "[{}] {}: {}",
        sanitize(room),
        sanitize(sender),
        sanitize(body)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Waiting for a command.
    Idle,
    /// The next plain line is a username.
    Username,
    /// A login is in flight; stdin is not read until it resolves.
    Busy,
    /// OAuth is waiting for the browser; only `/quit` is useful.
    OAuth,
}

/// What Ctrl-C does right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtrlC {
    /// Shut down at once.
    Quit,
    /// Wait: a hidden password read is pending.
    Defer,
}

/// `rpassword` turns terminal echo off and only restores it when its read
/// returns. The runtime swallows SIGINT, so quitting while that read is
/// blocked would leave the terminal with echo off. Defer until it returns.
fn ctrl_c_action(hidden_password_pending: bool) -> CtrlC {
    if hidden_password_pending {
        CtrlC::Defer
    } else {
        CtrlC::Quit
    }
}

struct Selected {
    id: RoomId,
    name: String,
}

struct Cli {
    core: CoreHandle,
    requests: std::sync::mpsc::Sender<Request>,
    inputs: mpsc::Receiver<Input>,
    /// One message per Ctrl-C.
    interrupts: mpsc::Receiver<()>,
    gate: Gate,
    password_stdin: bool,
    logged_in: bool,
    mode: Mode,
    /// Ctrl-C arrived while a hidden password read was pending.
    interrupted: bool,
    rooms: Vec<RoomSummary>,
    selected: Option<Selected>,
    timeline: Vec<TimelineItem>,
    printed: HashSet<String>,
    calls: calls::Calls,
}

/// What the loop should do after handling one input.
enum Flow {
    Continue,
    Quit,
}

impl Cli {
    async fn run(&mut self) {
        // The audio settings first, so call commands can edit them.
        for command in [Command::GetAudioSettings, Command::RestoreSession] {
            if self.core.commands.send(command).await.is_err() {
                return;
            }
        }
        self.mode = Mode::Busy;
        loop {
            let reading = self.mode != Mode::Busy;
            if let Some(request) = self.gate.line_request(reading)
                && self.requests.send(request).is_err()
            {
                return;
            }
            let waiting = self.gate.line_outstanding;
            tokio::select! {
                event = self.core.events.recv() => match event {
                    Some(event) => self.on_event(event),
                    None => return,
                },
                Some(()) = self.interrupts.recv() => {
                    println!("Interrupted.");
                    return;
                }
                input = self.inputs.recv(), if waiting => match input {
                    Some(input) => {
                        if let Some(line) = self.gate.repl_line(input)
                            && matches!(self.on_line(&line).await, Flow::Quit)
                        {
                            return;
                        }
                    }
                    None => return,
                },
            }
        }
    }

    fn prompt_username(&mut self) {
        eprint!("Username: ");
        self.mode = Mode::Username;
    }

    fn interrupt_action(&self) -> CtrlC {
        // With --password-stdin the read is a plain line read: echo is not
        // involved, so Ctrl-C need not wait for it.
        ctrl_c_action(self.gate.password_pending && !self.password_stdin)
    }

    async fn read_password(&mut self) -> Option<Password> {
        if !self.gate.begin_password() {
            return None;
        }
        let request = if self.password_stdin {
            Request::Line
        } else {
            Request::Password
        };
        let password = if self.requests.send(request).is_ok() {
            loop {
                tokio::select! {
                    biased;
                    Some(()) = self.interrupts.recv() => {
                        if self.interrupt_action() == CtrlC::Quit {
                            self.interrupted = true;
                            break None;
                        }
                        if !self.interrupted {
                            eprintln!(
                                "\nInterrupted. Press Enter to end the password prompt; \
                                 the CLI then exits."
                            );
                        }
                        self.interrupted = true;
                    }
                    input = self.inputs.recv() => break match input {
                        Some(Input::Line(line)) if self.password_stdin => {
                            Some(line.trim_end_matches(['\r', '\n']).to_owned())
                        }
                        Some(Input::Password(password)) if !self.password_stdin => password,
                        _ => None,
                    },
                }
            }
        } else {
            None
        };
        self.gate.end_password();
        password.map(Password::from)
    }

    /// Logs out on the server and waits (bounded) for the engine's answer.
    async fn logout_and_wait(&mut self) {
        if !self.logged_in || self.core.commands.send(Command::Logout).await.is_err() {
            return;
        }
        let wait = async {
            while let Some(event) = self.core.events.recv().await {
                if matches!(event, CoreEvent::LoggedOut | CoreEvent::Error(_)) {
                    return;
                }
            }
        };
        if tokio::time::timeout(LOGOUT_WAIT, wait).await.is_err() {
            eprintln!("twicecat-cli: server logout did not finish within {LOGOUT_WAIT:?}");
        }
    }

    async fn send(&mut self, command: Command) -> Flow {
        match self.core.commands.send(command).await {
            Ok(()) => Flow::Continue,
            Err(_) => Flow::Quit,
        }
    }

    async fn on_line(&mut self, line: &str) -> Flow {
        let line = line.trim_end_matches(['\r', '\n']);
        if self.mode == Mode::Username && !line.starts_with('/') {
            let username = line.trim().to_owned();
            if username.is_empty() {
                self.prompt_username();
                return Flow::Continue;
            }
            let password = self.read_password().await;
            if self.interrupted {
                println!("Interrupted.");
                return Flow::Quit;
            }
            let Some(password) = password else {
                println!("No password given.");
                self.mode = Mode::Idle;
                return Flow::Continue;
            };
            self.mode = Mode::Busy;
            return self
                .send(Command::LoginPassword {
                    username,
                    password,
                    server: None,
                })
                .await;
        }
        match parse_repl(line) {
            Repl::Login => {
                self.prompt_username();
                Flow::Continue
            }
            Repl::OAuth(server) => {
                self.mode = Mode::OAuth;
                self.send(Command::LoginOAuth { server }).await
            }
            Repl::Rooms => {
                self.print_rooms();
                Flow::Continue
            }
            Repl::Select(arg) => self.select(&arg).await,
            Repl::Send(text) => {
                self.send(Command::SendMessage {
                    text: MessageText::from(text),
                })
                .await
            }
            Repl::Logout => self.send(Command::Logout).await,
            Repl::Quit => Flow::Quit,
            Repl::Call(Ok(cmd)) => {
                let selected = self.selected.as_ref().map(|s| &s.id);
                let reply = self.calls.command(cmd, &self.rooms, selected);
                for line in reply.lines {
                    println!("{line}");
                }
                for command in reply.commands {
                    if matches!(self.send(command).await, Flow::Quit) {
                        return Flow::Quit;
                    }
                }
                Flow::Continue
            }
            Repl::Call(Err(usage)) => {
                println!("Usage: {usage}");
                Flow::Continue
            }
            Repl::Unknown(name) => {
                println!(
                    "Unknown or incomplete command {}. {REPL_HINT}",
                    sanitize(&name)
                );
                Flow::Continue
            }
            Repl::Text(_) => {
                println!("{REPL_HINT}");
                Flow::Continue
            }
        }
    }

    async fn select(&mut self, arg: &str) -> Flow {
        let Some(id) = resolve_room(&self.rooms, arg) else {
            println!("No such room. Use /rooms to list them.");
            return Flow::Continue;
        };
        let name = self
            .rooms
            .iter()
            .find(|room| room.id == id)
            .map(|room| room.name.clone())
            .unwrap_or_default();
        self.timeline.clear();
        self.printed.clear();
        println!("Opening {}", sanitize(&name));
        self.selected = Some(Selected {
            id: id.clone(),
            name,
        });
        self.send(Command::SelectRoom(id)).await
    }

    fn print_rooms(&self) {
        if self.rooms.is_empty() {
            println!("No rooms.");
        }
        for (index, room) in self.rooms.iter().enumerate() {
            let in_call = match self.calls.count(&room.id) {
                0 => String::new(),
                n => format!(", {n} in call"),
            };
            let voice = if room.is_call_room { "  [voice]" } else { "" };
            println!(
                "{index}  {}{voice}  (unread {}, mentions {}, notifications {}{in_call})",
                sanitize(&room.name),
                room.unread_messages,
                room.unread_mentions,
                room.unread_notifications
            );
        }
    }

    fn on_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::NoSession => {
                println!("No stored session.");
                self.prompt_username();
            }
            CoreEvent::LoggedIn {
                user_id, restored, ..
            } => {
                let how = if restored { " (restored)" } else { "" };
                println!("Logged in as {}{how}", sanitize(user_id.as_str()));
                self.logged_in = true;
                self.mode = Mode::Idle;
            }
            CoreEvent::LoginFailed(error) => {
                println!("Login failed: {error}. Use /login to try again.");
                self.mode = Mode::Idle;
            }
            CoreEvent::OAuthUrl(url) => {
                println!("Open this URL in your browser to log in:\n{}", url.expose());
            }
            CoreEvent::LoggedOut => {
                println!("Logged out. Use /login to log in again.");
                self.logged_in = false;
                self.rooms.clear();
                self.selected = None;
                self.timeline.clear();
                self.printed.clear();
                self.mode = Mode::Idle;
            }
            CoreEvent::SessionExpired => {
                println!("Session expired. Use /login to log in again.");
                self.logged_in = false;
                self.rooms.clear();
                self.selected = None;
                self.mode = Mode::Idle;
            }
            CoreEvent::SyncState(state) => println!("{}", status_line(&state)),
            CoreEvent::RoomListChanged(diffs) => {
                for diff in &diffs {
                    ListDiff::apply(diff, &mut self.rooms);
                }
            }
            // Room list paging and avatars are for the graphical app.
            CoreEvent::RoomListLoaded { .. }
            | CoreEvent::Avatar { .. }
            | CoreEvent::Pagination { .. }
            | CoreEvent::Typing { .. }
            | CoreEvent::Members { .. }
            | CoreEvent::DraftLoaded { .. }
            | CoreEvent::ActionFailed { .. } => {}
            CoreEvent::Security(status) => println!("{}", security_line(&status)),
            // The key is never printed.
            CoreEvent::RecoveryKeyReady(_) => println!("{RECOVERY_KEY_LINE}"),
            // Recovery progress, re-authentication, verification (including
            // QR codes), devices and trust are for the graphical app.
            CoreEvent::RecoveryProgress(_)
            | CoreEvent::Recovered
            | CoreEvent::IdentityReset
            | CoreEvent::ReauthRequired { .. }
            | CoreEvent::OpenAccountManagement(_)
            | CoreEvent::VerificationRequested { .. }
            | CoreEvent::Verification { .. }
            | CoreEvent::Devices(_)
            | CoreEvent::UserTrust { .. } => {}
            // One short line each; see `phase5_line`.
            event @ (CoreEvent::Media { .. }
            | CoreEvent::MediaSaved { .. }
            | CoreEvent::MediaLimits { .. }
            | CoreEvent::Notification(_)
            | CoreEvent::NotificationSettings(_)
            | CoreEvent::UnreadTotals { .. }
            | CoreEvent::RoomCreated { .. }
            | CoreEvent::RoomJoined { .. }
            | CoreEvent::RoomDetails(_)
            | CoreEvent::RoomMembers { .. }
            | CoreEvent::OwnProfile(_)
            | CoreEvent::Spaces(_)
            | CoreEvent::ThreadDiff { .. }
            | CoreEvent::ThreadPagination { .. }) => {
                if let Some(line) = phase5_line(&event) {
                    println!("{line}");
                }
            }
            CoreEvent::TimelineDiff { room_id, diffs } => self.on_timeline(&room_id, &diffs),
            CoreEvent::SendFailed { room_id, error } => {
                println!("Send failed in {}: {error}", sanitize(room_id.as_str()));
            }
            CoreEvent::Error(error) => {
                eprintln!("error: {error}");
                // A restore or login in flight that ends in a plain error
                // (not LoginFailed or NoSession) must not strand the CLI in
                // Busy, where it never reads stdin.
                if self.mode == Mode::Busy && !self.logged_in {
                    println!("{START_FAILED_HINT}");
                    self.mode = Mode::Idle;
                }
            }
            CoreEvent::CallState(_)
            | CoreEvent::CallParticipants { .. }
            | CoreEvent::ActiveCall { .. }
            | CoreEvent::IncomingCall { .. }
            | CoreEvent::RingingStopped { .. }
            | CoreEvent::CallDeclined { .. }
            | CoreEvent::AudioDevices(_)
            | CoreEvent::AudioSettings(_)
            | CoreEvent::InputLevel(_)
            | CoreEvent::AudioStats(_)
            | CoreEvent::GpuEnhancement(_) => {
                for line in self.calls.event(event, &self.rooms) {
                    println!("{line}");
                }
            }
        }
    }

    fn on_timeline(&mut self, room_id: &RoomId, diffs: &[ListDiff<TimelineItem>]) {
        let Some(selected) = &self.selected else {
            return;
        };
        if &selected.id != room_id {
            return;
        }
        for diff in diffs {
            ListDiff::apply(diff, &mut self.timeline);
        }
        for item in &self.timeline {
            if let TimelineItemKind::Message { sender, body, .. } = &item.kind
                && self.printed.insert(item.id.clone())
            {
                println!("{}", format_message(&selected.name, sender.as_str(), body));
            }
        }
    }
}

/// `--silent-audio`: captures silence and drops what it would play.
struct Silence;

impl VirtualAudio for Silence {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        frame.fill(0.0);
    }

    fn playback(&self, _: &[f32; AUDIO_FRAME_LEN]) {}
}

/// A fresh, unpredictable path under the system temp dir; the core creates it
/// with owner-only permissions when it first needs it.
fn ephemeral_dir() -> PathBuf {
    let mut bytes = [0_u8; 16];
    rand::fill(&mut bytes[..]);
    let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::env::temp_dir().join(format!("twicecat-cli-{suffix}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    twicecat_core::require_cpu_level();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("twicecat-cli: {message}\nTry --help.");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = logging::init(logging::DEFAULT_FILTER) {
        eprintln!("twicecat-cli: logging not started: {error}");
    }

    let (data, secrets, cleanup): (DataPaths, Arc<dyn SecretStore>, Option<PathBuf>) =
        if options.ephemeral {
            let dir = ephemeral_dir();
            (
                DataPaths::at(&dir),
                Arc::new(InMemorySecretStore::new()),
                Some(dir),
            )
        } else {
            let data = match options.data_dir {
                Some(dir) => DataPaths::at(dir),
                None => match DataPaths::from_os() {
                    Some(data) => data,
                    None => {
                        eprintln!("twicecat-cli: no OS data directory; pass --data-dir");
                        return ExitCode::FAILURE;
                    }
                },
            };
            (
                data,
                Arc::new(KeyringSecretStore::new(KEYCHAIN_SERVICE)),
                None,
            )
        };

    let core = Core::start(
        CoreConfig {
            data,
            homeserver_override: options.homeserver,
            room_list_page_size: DEFAULT_ROOM_LIST_PAGE_SIZE,
            max_upload_bytes_override: None,
            audio: if options.silent_audio {
                AudioBackend::Virtual(Arc::new(Silence))
            } else {
                AudioBackend::System
            },
            call_service_override: None,
        },
        secrets,
    );

    if options.password_stdin && std::io::stdin().is_terminal() {
        eprintln!(
            "twicecat-cli: warning: --password-stdin with a terminal on stdin; \
             the password will be echoed as you type it"
        );
    }

    let (requests, request_rx) = std::sync::mpsc::channel();
    let (input_tx, inputs) = mpsc::channel(1);
    // A plain thread, not a tokio task: a blocked terminal read must not keep
    // the runtime from shutting down.
    std::thread::spawn(move || reader_loop(request_rx, input_tx, TerminalSource));

    // Ctrl-C is delivered to the loop as a message, so it can be deferred
    // while a hidden password read is pending (see `ctrl_c_action`).
    let (interrupt_tx, interrupts) = mpsc::channel(1);
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            // Full means one is already queued.
            let _ = interrupt_tx.try_send(());
        }
    });

    let mut cli = Cli {
        core,
        requests,
        inputs,
        interrupts,
        gate: Gate::default(),
        password_stdin: options.password_stdin,
        logged_in: false,
        mode: Mode::Busy,
        interrupted: false,
        rooms: Vec::new(),
        selected: None,
        timeline: Vec::new(),
        printed: HashSet::new(),
        calls: calls::Calls::default(),
    };
    cli.run().await;

    // An ephemeral session is thrown away, so revoke it on the server too.
    if cleanup.is_some() {
        cli.logout_and_wait().await;
    }
    // Ask the engine to stop and wait until it has released the store.
    let _ = cli.core.commands.send(Command::Shutdown).await;
    let drain = async { while cli.core.events.recv().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_WAIT, drain).await.is_err() {
        eprintln!("twicecat-cli: engine did not stop within {SHUTDOWN_WAIT:?}");
    }
    if let Some(dir) = cleanup
        && let Err(error) = std::fs::remove_dir_all(&dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("twicecat-cli: could not remove the temporary data dir: {error}");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn room(id: &str, name: &str) -> RoomSummary {
        RoomSummary::new(RoomId::from(id), name.to_owned())
    }

    #[test]
    fn parses_all_flags() {
        let parsed = parse_args(&args(&[
            "--homeserver",
            "http://h",
            "--ephemeral",
            "--password-stdin",
            "--silent-audio",
        ]));
        assert_eq!(
            parsed,
            Ok(Parsed::Run(Options {
                data_dir: None,
                homeserver: Some("http://h".into()),
                ephemeral: true,
                password_stdin: true,
                silent_audio: true,
            }))
        );
    }

    #[test]
    fn data_dir_flag() {
        let parsed = parse_args(&args(&["--data-dir", "/d"]));
        assert_eq!(
            parsed,
            Ok(Parsed::Run(Options {
                data_dir: Some("/d".into()),
                homeserver: None,
                ephemeral: false,
                password_stdin: false,
                silent_audio: false,
            }))
        );
    }

    #[test]
    fn defaults_and_help() {
        assert_eq!(
            parse_args(&[]),
            Ok(Parsed::Run(Options {
                data_dir: None,
                homeserver: None,
                ephemeral: false,
                password_stdin: false,
                silent_audio: false,
            }))
        );
        assert_eq!(parse_args(&args(&["--help"])), Ok(Parsed::Help));
    }

    #[test]
    fn rejects_bad_flags() {
        assert!(parse_args(&args(&["--nope"])).is_err());
        assert!(parse_args(&args(&["--data-dir"])).is_err());
        assert!(parse_args(&args(&["--ephemeral", "--data-dir", "/d"])).is_err());
    }

    #[test]
    fn help_warns_about_debug_logging() {
        assert!(HELP.contains("RUST_LOG"));
        assert!(HELP.contains("debug"));
        assert!(HELP.contains("trace"));
        assert!(HELP.contains("stderr"));
    }

    #[test]
    fn repl_lines() {
        assert_eq!(parse_repl("/login"), Repl::Login);
        assert_eq!(
            parse_repl("/oauth matrix.org"),
            Repl::OAuth("matrix.org".into())
        );
        assert_eq!(parse_repl("/rooms"), Repl::Rooms);
        assert_eq!(parse_repl("/select 2"), Repl::Select("2".into()));
        assert_eq!(
            parse_repl("/send hello  world "),
            Repl::Send("hello  world ".into())
        );
        assert_eq!(parse_repl("/logout"), Repl::Logout);
        assert_eq!(parse_repl("/quit"), Repl::Quit);
        assert_eq!(parse_repl("/bogus x"), Repl::Unknown("/bogus".into()));
        assert_eq!(parse_repl("plain"), Repl::Text("plain".into()));
        assert_eq!(parse_repl("/oauth"), Repl::Unknown("/oauth".into()));
        assert_eq!(parse_repl("/send"), Repl::Unknown("/send".into()));
    }

    #[test]
    fn resolves_rooms_by_index_or_id() {
        let rooms = [room("!a:x", "A"), room("!b:x", "B")];
        assert_eq!(resolve_room(&rooms, "1"), Some(RoomId::from("!b:x")));
        assert_eq!(resolve_room(&rooms, "!a:x"), Some(RoomId::from("!a:x")));
        assert_eq!(resolve_room(&rooms, "2"), None);
        assert_eq!(resolve_room(&rooms, "nope"), None);
    }

    #[test]
    fn status_and_message_lines() {
        assert_eq!(status_line(&SyncState::Running), "[sync] running");
        assert_eq!(
            status_line(&SyncState::Reconnecting { attempt: 2 }),
            "[sync] reconnecting (attempt 2)"
        );
        assert_eq!(format_message("R", "@a:x", "hi"), "[R] @a:x: hi");
    }

    #[test]
    fn control_characters_are_neutralised() {
        assert_eq!(
            format_message("R\x1b[2J", "@a:x", "a\nb\x07"),
            "[R?[2J] @a:x: a?b?"
        );
    }

    #[test]
    fn bidi_and_invisible_characters_are_neutralised() {
        for c in ('\u{200B}'..='\u{200F}')
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2066}'..='\u{2069}')
            .chain(['\u{FEFF}'])
        {
            assert_eq!(sanitize(&format!("a{c}b")), "a?b", "U+{:04X}", c as u32);
        }
        assert_eq!(sanitize("plain \u{e9}\u{4e2d}"), "plain \u{e9}\u{4e2d}");
    }

    /// Serves scripted lines and passwords; reading the wrong kind panics.
    struct Fake {
        lines: Vec<&'static str>,
        passwords: Vec<&'static str>,
    }

    impl Source for Fake {
        fn read_line(&mut self) -> Option<String> {
            assert!(!self.lines.is_empty(), "a line was read out of turn");
            Some(self.lines.remove(0).to_owned())
        }

        fn read_password(&mut self) -> Option<String> {
            assert!(
                !self.passwords.is_empty(),
                "a password was read out of turn"
            );
            Some(self.passwords.remove(0).to_owned())
        }
    }

    fn run_reader(requests: &[Request], fake: Fake) -> Vec<Input> {
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let (out_tx, mut out_rx) = mpsc::channel(8);
        for request in requests {
            request_tx.send(*request).expect("queue request");
        }
        drop(request_tx);
        reader_loop(request_rx, out_tx, fake);
        let mut all = Vec::new();
        while let Ok(input) = out_rx.try_recv() {
            all.push(input);
        }
        all
    }

    #[test]
    fn password_is_read_apart_from_repl_lines() {
        let out = run_reader(
            &[Request::Line, Request::Password, Request::Line],
            Fake {
                lines: vec!["bob", "/rooms"],
                passwords: vec!["s3cret"],
            },
        );
        assert_eq!(
            out,
            [
                Input::Line("bob".into()),
                Input::Password(Some("s3cret".into())),
                Input::Line("/rooms".into()),
            ]
        );
    }

    #[test]
    fn gate_never_routes_a_password_to_the_repl() {
        let mut gate = Gate::default();
        assert_eq!(gate.repl_line(Input::Password(Some("s3cret".into()))), None);
        assert_eq!(gate.repl_line(Input::Password(None)), None);
        assert_eq!(gate.repl_line(Input::Line("hi".into())), Some("hi".into()));
    }

    #[test]
    fn gate_allows_one_reader_at_a_time() {
        let mut gate = Gate::default();
        assert_eq!(gate.line_request(false), None);
        assert_eq!(gate.line_request(true), Some(Request::Line));
        // A line read is in flight: no second one, and no password read.
        assert_eq!(gate.line_request(true), None);
        assert!(!gate.begin_password());
        assert_eq!(
            gate.repl_line(Input::Line("bob".into())),
            Some("bob".into())
        );
        assert!(gate.begin_password());
        // While the password is read, no line read may start.
        assert_eq!(gate.line_request(true), None);
        gate.end_password();
        assert_eq!(gate.line_request(true), Some(Request::Line));
    }

    /// A `Cli` wired to dead channels, for state-transition tests.
    fn test_cli(mode: Mode, logged_in: bool) -> (Cli, mpsc::Sender<()>) {
        let (commands, _command_rx) = mpsc::channel(8);
        let (_event_tx, events) = mpsc::channel(8);
        let (requests, _request_rx) = std::sync::mpsc::channel();
        let (_input_tx, inputs) = mpsc::channel(1);
        let (interrupt_tx, interrupts) = mpsc::channel(1);
        let cli = Cli {
            core: CoreHandle { commands, events },
            requests,
            inputs,
            interrupts,
            gate: Gate::default(),
            password_stdin: false,
            logged_in,
            mode,
            interrupted: false,
            rooms: Vec::new(),
            selected: None,
            timeline: Vec::new(),
            printed: HashSet::new(),
            calls: calls::Calls::default(),
        };
        (cli, interrupt_tx)
    }

    #[test]
    fn security_line_prints_states_only() {
        use twicecat_core::api::{
            CrossSigning, DeviceVerification, RecoveryStatus, SecurityStatus,
        };
        let status = SecurityStatus {
            recovery: RecoveryStatus::Incomplete,
            device: DeviceVerification::Unverified,
            cross_signing: CrossSigning::MissingKeys,
            has_devices_to_verify_against: true,
            identity_exists: true,
        };
        assert_eq!(
            security_line(&status),
            "[security] recovery: Incomplete, device: Unverified, cross-signing: MissingKeys"
        );
    }

    #[test]
    fn phase5_events_print_one_line_without_content() {
        use twicecat_core::api::{
            EventId, MediaBytes, MediaSize, NotificationKind, NotificationView, OwnProfileView,
            PaginationState, RoomNotificationModeView, SpaceView,
        };
        let room = || RoomId::from("!r:x");
        let events = vec![
            CoreEvent::Media {
                room_id: room(),
                item_id: "secret-name-x.png".into(),
                size: MediaSize::Full,
                result: Ok(MediaBytes::from(b"secret-body-x".to_vec())),
            },
            CoreEvent::MediaSaved {
                room_id: room(),
                item_id: "secret-name-x.png".into(),
            },
            CoreEvent::MediaLimits {
                max_upload_bytes: Some(1024),
            },
            CoreEvent::Notification(NotificationView {
                room_id: room(),
                event_id: EventId::from("$e"),
                room_name: "secret-name-x".into(),
                sender: twicecat_core::api::UserId::from("@a:x"),
                sender_name: Some("secret-name-x".into()),
                is_dm: false,
                kind: NotificationKind::Message,
                preview: Some(MessageText::from("secret-body-x")),
            }),
            CoreEvent::NotificationSettings(twicecat_core::api::NotificationSettingsView {
                group_default: RoomNotificationModeView::AllMessages,
                dm_default: RoomNotificationModeView::Mute,
                keywords: vec!["secret-name-x".into()],
            }),
            CoreEvent::UnreadTotals {
                notifications: 3,
                highlights: 1,
            },
            CoreEvent::RoomCreated { room_id: room() },
            CoreEvent::RoomJoined { room_id: room() },
            CoreEvent::RoomDetails(twicecat_core::api::RoomDetailsView {
                room_id: room(),
                name: "secret-name-x".into(),
                topic: None,
                avatar: None,
                is_encrypted: true,
                alias: None,
                permissions: Default::default(),
            }),
            CoreEvent::RoomMembers {
                room_id: room(),
                members: Vec::new(),
                truncated: false,
            },
            CoreEvent::OwnProfile(OwnProfileView {
                display_name: Some("secret-name-x".into()),
                avatar: None,
            }),
            CoreEvent::Spaces(vec![SpaceView {
                id: room(),
                name: "secret-name-x".into(),
                avatar: None,
                level: 0,
                children: 1,
            }]),
            CoreEvent::ThreadDiff {
                room_id: room(),
                root: EventId::from("$r"),
                diffs: Vec::new(),
            },
            CoreEvent::ThreadPagination {
                room_id: room(),
                root: EventId::from("$r"),
                state: PaginationState::Idle,
            },
        ];
        for event in events {
            let line = phase5_line(&event).expect("a line for every Phase 5 event");
            assert!(!line.contains('\n') && !line.is_empty(), "{line}");
            assert!(
                !line.contains("secret-name-x") && !line.contains("secret-body-x"),
                "{line}"
            );
        }
        assert_eq!(
            phase5_line(&CoreEvent::UnreadTotals {
                notifications: 3,
                highlights: 1
            })
            .as_deref(),
            Some("[unread] 3 notifications, 1 highlights")
        );
        assert!(phase5_line(&CoreEvent::LoggedOut).is_none());
    }

    #[test]
    fn recovery_key_event_prints_no_key() {
        use twicecat_core::api::RecoveryKey;
        let (mut cli, _keep) = test_cli(Mode::Idle, true);
        // `on_event` must accept it; the line it prints names no key.
        cli.on_event(CoreEvent::RecoveryKeyReady(RecoveryKey::from(
            "secret-key-x",
        )));
        assert_eq!(RECOVERY_KEY_LINE, "recovery key received, not shown");
    }

    #[test]
    fn error_during_restore_or_login_leaves_busy() {
        let (mut cli, _keep) = test_cli(Mode::Busy, false);
        cli.on_event(CoreEvent::Error(twicecat_core::api::CoreError::Storage(
            "x".into(),
        )));
        assert_eq!(cli.mode, Mode::Idle);
        // Idle means stdin is read again, so /quit and /login work.
        assert!(cli.gate.line_request(cli.mode != Mode::Busy).is_some());
    }

    #[test]
    fn error_keeps_other_modes() {
        let (mut cli, _keep) = test_cli(Mode::Idle, true);
        cli.on_event(CoreEvent::Error(twicecat_core::api::CoreError::NotLoggedIn));
        assert_eq!(cli.mode, Mode::Idle);
        let (mut cli, _keep) = test_cli(Mode::Username, false);
        cli.on_event(CoreEvent::Error(twicecat_core::api::CoreError::NotLoggedIn));
        assert_eq!(cli.mode, Mode::Username);
    }

    #[test]
    fn other_not_logged_in_failures_leave_busy() {
        let (mut cli, _keep) = test_cli(Mode::Busy, false);
        cli.on_event(CoreEvent::LoginFailed(
            twicecat_core::api::CoreError::Cancelled,
        ));
        assert_eq!(cli.mode, Mode::Idle);
        let (mut cli, _keep) = test_cli(Mode::Busy, false);
        cli.on_event(CoreEvent::NoSession);
        assert_eq!(cli.mode, Mode::Username);
        let (mut cli, _keep) = test_cli(Mode::Busy, false);
        cli.on_event(CoreEvent::SessionExpired);
        assert_eq!(cli.mode, Mode::Idle);
    }

    #[tokio::test]
    async fn quit_works_after_a_failed_restore() {
        let (mut cli, _keep) = test_cli(Mode::Busy, false);
        cli.on_event(CoreEvent::Error(twicecat_core::api::CoreError::Storage(
            "x".into(),
        )));
        assert!(matches!(cli.on_line("/quit\n").await, Flow::Quit));
    }

    #[test]
    fn ctrl_c_waits_for_a_hidden_password_read() {
        assert_eq!(ctrl_c_action(true), CtrlC::Defer);
        assert_eq!(ctrl_c_action(false), CtrlC::Quit);
        let (mut cli, _keep) = test_cli(Mode::Idle, false);
        assert_eq!(cli.interrupt_action(), CtrlC::Quit);
        assert!(cli.gate.begin_password());
        assert_eq!(cli.interrupt_action(), CtrlC::Defer);
        // A password read from stdin does not touch terminal echo.
        cli.password_stdin = true;
        assert_eq!(cli.interrupt_action(), CtrlC::Quit);
    }

    struct BlockedPassword(std::sync::mpsc::Receiver<()>);

    impl Source for BlockedPassword {
        fn read_line(&mut self) -> Option<String> {
            None
        }

        fn read_password(&mut self) -> Option<String> {
            self.0.recv().ok();
            Some("pw".to_owned())
        }
    }

    #[tokio::test]
    async fn ctrl_c_during_a_password_read_waits_for_the_read_then_quits() {
        let (mut cli, interrupt_tx) = test_cli(Mode::Username, false);
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let (input_tx, inputs) = mpsc::channel(1);
        cli.requests = request_tx;
        cli.inputs = inputs;
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || reader_loop(request_rx, input_tx, BlockedPassword(release_rx)));
        interrupt_tx.send(()).await.expect("queue interrupt");
        release_tx.send(()).expect("release the read");
        let flow = cli.on_line("bob\n").await;
        // The read finished (so the terminal was restored), then the CLI quits.
        assert!(cli.interrupted);
        assert!(!cli.gate.password_pending);
        assert!(matches!(flow, Flow::Quit));
    }

    #[test]
    fn help_documents_password_stdin_echo_and_ephemeral_logout() {
        assert!(HELP.contains("echoed"));
        assert!(HELP.contains("logged out on the server"));
        assert!(HELP.contains("Ctrl-C"));
    }
}
