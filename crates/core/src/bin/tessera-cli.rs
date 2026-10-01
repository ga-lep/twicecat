//! `tessera-cli`: a small command-line front end on the core's command and
//! event API. It never touches the Matrix SDK directly.
//!
//! Message bodies are printed to stdout because that is the program's output;
//! nothing here sends a token, password, or message body to the log.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tessera_core::api::{
    Command, CoreEvent, ListDiff, MessageText, Password, RoomId, RoomSummary, SyncState,
    TimelineItem, TimelineItemKind,
};
use tessera_core::logging;
use tessera_core::paths::DataPaths;
use tessera_core::secrets::{
    InMemorySecretStore, KEYCHAIN_SERVICE, KeyringSecretStore, SecretStore,
};
use tessera_core::{Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

const HELP: &str = "\
tessera-cli: headless Matrix client on the Tessera core

USAGE:
    tessera-cli [--data-dir <path> | --ephemeral] [--homeserver <url>] [--password-stdin]

OPTIONS:
    --data-dir <path>     Store session data here (default: the OS data directory;
                          on Linux ~/.local/share/tessera)
    --homeserver <url>    Use this homeserver (URL or server name) instead of
                          discovering it from the user ID
    --ephemeral           Keep the secret store in memory and use a temporary
                          data directory that is deleted on exit
    --password-stdin      Read the password from the next stdin line instead of
                          prompting on the terminal
    --help                Show this help

COMMANDS (type them at the prompt):
    /login                Log in with a username and password
    /oauth <server>       Log in through OAuth 2.0 (prints the URL to open)
    /rooms                List rooms: index, name, unread counts
    /select <index|id>    Open a room (index from /rooms, or a room ID)
    /send <text>          Send a message to the open room
    /logout               Log out and delete the stored session
    /quit                 Shut down

LOGGING:
    Logs go to stderr; the default filter is `warn,tessera_core=info`.
    Setting RUST_LOG to debug or trace can make the Matrix SDK and HTTP crates
    log sensitive request data (URLs, user and room identifiers). Even at the
    default filter, the SDK's own error lines can contain request URLs, so do
    not share or post stderr unreviewed.

A login that is cancelled or aborted during OAuth may leave a session on the
server; revoke stray sessions from your account settings.
";

const REPL_HINT: &str =
    "Commands: /login /oauth <server> /rooms /select <n|id> /send <text> /logout /quit";

/// How long to wait for the engine to finish after `Shutdown`.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, PartialEq, Eq)]
struct Options {
    data_dir: Option<String>,
    homeserver: Option<String>,
    ephemeral: bool,
    password_stdin: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Run(Options),
    Help,
}

#[derive(Debug, PartialEq, Eq)]
enum Repl {
    Login,
    OAuth(String),
    Rooms,
    Select(String),
    Send(String),
    Logout,
    Quit,
    Unknown(String),
    Text(String),
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut options = Options {
        data_dir: None,
        homeserver: None,
        ephemeral: false,
        password_stdin: false,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Parsed::Help),
            "--ephemeral" => options.ephemeral = true,
            "--password-stdin" => options.password_stdin = true,
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
        _ => Repl::Unknown(format!("/{name}")),
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

/// Replaces control characters so server-controlled text cannot drive the
/// terminal with escape sequences.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
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

struct Selected {
    id: RoomId,
    name: String,
}

struct Cli {
    core: CoreHandle,
    lines: mpsc::Receiver<String>,
    password_stdin: bool,
    mode: Mode,
    rooms: Vec<RoomSummary>,
    selected: Option<Selected>,
    timeline: Vec<TimelineItem>,
    printed: HashSet<String>,
}

/// What the loop should do after handling one input.
enum Flow {
    Continue,
    Quit,
}

impl Cli {
    async fn run(&mut self) {
        if self
            .core
            .commands
            .send(Command::RestoreSession)
            .await
            .is_err()
        {
            return;
        }
        self.mode = Mode::Busy;
        loop {
            let reading = self.mode != Mode::Busy;
            tokio::select! {
                event = self.core.events.recv() => match event {
                    Some(event) => self.on_event(event),
                    None => return,
                },
                line = self.lines.recv(), if reading => match line {
                    Some(line) => {
                        if matches!(self.on_line(&line).await, Flow::Quit) {
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

    async fn read_password(&mut self) -> Option<Password> {
        if self.password_stdin {
            return self.lines.recv().await.map(Password::from);
        }
        match tokio::task::spawn_blocking(|| rpassword::prompt_password("Password: ")).await {
            Ok(Ok(password)) => Some(Password::from(password)),
            _ => None,
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
            let Some(password) = self.read_password().await else {
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
            println!(
                "{index}  {}  (unread {}, mentions {}, notifications {})",
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
                self.rooms.clear();
                self.selected = None;
                self.timeline.clear();
                self.printed.clear();
                self.mode = Mode::Idle;
            }
            CoreEvent::SessionExpired => {
                println!("Session expired. Use /login to log in again.");
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
            CoreEvent::TimelineDiff { room_id, diffs } => self.on_timeline(&room_id, &diffs),
            CoreEvent::SendFailed { room_id, error } => {
                println!("Send failed in {}: {error}", sanitize(room_id.as_str()));
            }
            CoreEvent::Error(error) => eprintln!("error: {error}"),
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

/// A fresh, unpredictable path under the system temp dir; the core creates it
/// with owner-only permissions when it first needs it.
fn ephemeral_dir() -> PathBuf {
    let mut bytes = [0_u8; 16];
    rand::fill(&mut bytes[..]);
    let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::env::temp_dir().join(format!("tessera-cli-{suffix}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("tessera-cli: {message}\nTry --help.");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = logging::init(logging::DEFAULT_FILTER) {
        eprintln!("tessera-cli: logging not started: {error}");
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
                        eprintln!("tessera-cli: no OS data directory; pass --data-dir");
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
        },
        secrets,
    );

    let (line_tx, lines) = mpsc::channel(64);
    tokio::spawn(async move {
        let mut reader = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if line_tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let mut cli = Cli {
        core,
        lines,
        password_stdin: options.password_stdin,
        mode: Mode::Busy,
        rooms: Vec::new(),
        selected: None,
        timeline: Vec::new(),
        printed: HashSet::new(),
    };
    cli.run().await;

    // Ask the engine to stop and wait until it has released the store.
    let _ = cli.core.commands.send(Command::Shutdown).await;
    let drain = async { while cli.core.events.recv().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_WAIT, drain).await.is_err() {
        eprintln!("tessera-cli: engine did not stop within {SHUTDOWN_WAIT:?}");
    }
    if let Some(dir) = cleanup
        && let Err(error) = std::fs::remove_dir_all(&dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("tessera-cli: could not remove the temporary data dir: {error}");
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
        RoomSummary {
            id: RoomId::from(id),
            name: name.to_owned(),
            unread_messages: 0,
            unread_mentions: 0,
            unread_notifications: 0,
        }
    }

    #[test]
    fn parses_all_flags() {
        let parsed = parse_args(&args(&[
            "--homeserver",
            "http://h",
            "--ephemeral",
            "--password-stdin",
        ]));
        assert_eq!(
            parsed,
            Ok(Parsed::Run(Options {
                data_dir: None,
                homeserver: Some("http://h".into()),
                ephemeral: true,
                password_stdin: true,
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
                password_stdin: false
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
}
