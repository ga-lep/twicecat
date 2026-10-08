//! Tessera desktop app entry point: parses the command line, starts the core
//! on its own runtime and runs the Iced application.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use tessera_app::state::App;
use tessera_app::{Bridge, Startup, TITLE};
use tessera_core::api::AudioBackend;
use tessera_core::logging;
use tessera_core::paths::DataPaths;
use tessera_core::secrets::{
    InMemorySecretStore, KEYCHAIN_SERVICE, KeyringSecretStore, SecretStore,
};
use tessera_core::{CoreConfig, DEFAULT_ROOM_LIST_PAGE_SIZE};

/// Log filter of the app (plan Q19): the app never logs messages or secrets.
const LOG_FILTER: &str = "warn,tessera_core=info,tessera_app=info";

const HELP: &str = "\
tessera: desktop Matrix client

USAGE:
    tessera [--data-dir <path> | --ephemeral] [--homeserver <url>]

OPTIONS:
    --data-dir <path>     Store session data here (default: the OS data directory;
                          on Linux ~/.local/share/tessera)
    --homeserver <url>    Use this homeserver (URL or server name) instead of
                          discovering it from the user ID
    --ephemeral           Keep the secret store in memory and use a temporary
                          data directory that is deleted on exit. The session
                          is not revoked on the server; revoke it from your
                          account settings.
    --help                Show this help

The app and tessera-cli share the same data directory and keychain entry, so a
session made in one is restored in the other. Do not run both at the same time
on one data directory: the session store is locked by SQLite.
";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    data_dir: Option<String>,
    homeserver: Option<String>,
    ephemeral: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Run(Options),
    Help,
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut options = Options {
        data_dir: None,
        homeserver: None,
        ephemeral: false,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Parsed::Help),
            "--ephemeral" => options.ephemeral = true,
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

/// A fresh, unpredictable path under the system temp dir; the core creates it
/// with owner-only permissions when it first needs it. The randomness is the
/// standard library's per-process hash seed.
fn ephemeral_dir() -> PathBuf {
    let seed = |_| RandomState::new().build_hasher().finish();
    let suffix: String = (0..2).map(|i| format!("{:016x}", seed(i))).collect();
    std::env::temp_dir().join(format!("tessera-{suffix}"))
}

fn run(options: Options) -> Result<(), String> {
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
                None => DataPaths::from_os().ok_or("no OS data directory; pass --data-dir")?,
            };
            (
                data,
                Arc::new(KeyringSecretStore::new(KEYCHAIN_SERVICE)),
                None,
            )
        };
    let config = CoreConfig {
        data,
        homeserver_override: options.homeserver,
        room_list_page_size: DEFAULT_ROOM_LIST_PAGE_SIZE,
        max_upload_bytes_override: None,
        audio: AudioBackend::System,
        call_service_override: None,
    };
    let (bridge, events) = Bridge::start(config, secrets)
        .map_err(|error| format!("could not start the core runtime: {error}"))?;
    let startup = Startup {
        sink: Box::new(bridge),
        events,
    };

    // The app (and with it the bridge and the core's runtime) is dropped when
    // `run` returns, before the temporary directory is removed.
    let result = iced::application(App::booter(startup), App::update, App::view)
        .title(TITLE)
        .subscription(App::subscription)
        .theme(App::theme)
        .exit_on_close_request(false)
        .run();
    if let Some(dir) = cleanup {
        // Best effort: nothing useful to do if it fails.
        let _ = std::fs::remove_dir_all(dir);
    }
    result.map_err(|error| format!("the window failed: {error}"))
}

fn main() -> ExitCode {
    tessera_core::require_cpu_level();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("tessera: {message}\nTry --help.");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = logging::init(LOG_FILTER) {
        eprintln!("tessera: logging not started: {error}");
    }
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tessera: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn window_title_is_tessera() {
        assert_eq!(TITLE, "Tessera");
    }

    #[test]
    fn parses_every_option() {
        assert_eq!(
            parse_args(&args(&["--ephemeral", "--homeserver", "http://h"])),
            Ok(Parsed::Run(Options {
                data_dir: None,
                homeserver: Some("http://h".into()),
                ephemeral: true,
            }))
        );
        assert_eq!(
            parse_args(&args(&["--data-dir", "/d"])),
            Ok(Parsed::Run(Options {
                data_dir: Some("/d".into()),
                homeserver: None,
                ephemeral: false,
            }))
        );
        assert_eq!(parse_args(&args(&["--help"])), Ok(Parsed::Help));
        assert_eq!(parse_args(&args(&[])).map(|p| p == Parsed::Help), Ok(false));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&args(&["--data-dir"])).is_err());
        assert!(parse_args(&args(&["--homeserver"])).is_err());
        assert!(parse_args(&args(&["--ephemeral", "--data-dir", "/d"])).is_err());
        assert!(parse_args(&args(&["--nope"])).is_err());
    }

    #[test]
    fn help_carries_the_shared_data_dir_note() {
        assert!(HELP.contains("tessera-cli"));
        assert!(HELP.contains("at the same time"));
    }

    #[test]
    fn ephemeral_dirs_differ_between_calls() {
        assert_ne!(ephemeral_dir(), ephemeral_dir());
    }
}
