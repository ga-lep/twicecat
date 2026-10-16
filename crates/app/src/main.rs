//! Twicecat desktop app entry point: parses the command line, starts the core
//! on its own runtime and runs the Iced application.

// Release builds on Windows are GUI programs: no console window opens beside
// the app. `--help`, `--version` and argument errors still reach a terminal
// that started it (`attach_parent_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use twicecat_app::settings::DiskSettings;
use twicecat_app::state::App;
use twicecat_app::ui;
use twicecat_app::{Bridge, Startup, TITLE};
use twicecat_core::api::AudioBackend;
use twicecat_core::logging;
use twicecat_core::paths::DataPaths;
use twicecat_core::secrets::{
    InMemorySecretStore, KEYCHAIN_SERVICE, KeyringSecretStore, SecretStore,
};
use twicecat_core::{CoreConfig, DEFAULT_ROOM_LIST_PAGE_SIZE};

/// Log filter of the app (plan Q19): the app never logs messages or secrets.
const LOG_FILTER: &str = "warn,twicecat_core=info,twicecat_app=info";

const HELP: &str = "\
twicecat: desktop Matrix client

USAGE:
    twicecat [--data-dir <path> | --ephemeral] [--homeserver <url>]

OPTIONS:
    --data-dir <path>     Store session data here (default: the OS data directory;
                          on Linux ~/.local/share/twicecat)
    --homeserver <url>    Use this homeserver (URL or server name) instead of
                          discovering it from the user ID
    --ephemeral           Keep the secret store in memory and use a temporary
                          data directory that is deleted on exit. The session
                          is not revoked on the server; revoke it from your
                          account settings.
    --version             Show the version
    --help                Show this help

The app and twicecat-cli share the same data directory and keychain entry, so a
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
    Version,
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut options = Options {
        data_dir: None,
        homeserver: None,
        ephemeral: false,
    };
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        return match args {
            [_] => Ok(Parsed::Version),
            _ => Err("--version takes no other arguments".to_owned()),
        };
    }
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
    std::env::temp_dir().join(format!("twicecat-{suffix}"))
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
    // Read before the window opens so the stored choices apply from the start.
    let ui_settings = data.read_settings();
    let settings_store = DiskSettings::new(data.clone());
    let config = CoreConfig {
        data,
        homeserver_override: options.homeserver,
        room_list_page_size: DEFAULT_ROOM_LIST_PAGE_SIZE,
        max_upload_bytes_override: None,
        audio: AudioBackend::System,
        call_service_override: None,
        video: twicecat_core::api::VideoBackend::System,
    };
    let (bridge, events) = Bridge::start(config, secrets)
        .map_err(|error| format!("could not start the core runtime: {error}"))?;
    let startup = Startup {
        sink: Box::new(bridge),
        events,
    };

    // The app (and with it the bridge and the core's runtime) is dropped when
    // `run` returns, before the temporary directory is removed.
    let application = iced::application(
        App::booter(startup, ui_settings, settings_store),
        App::update,
        App::view,
    );
    let result = ui::fonts::ALL
        .into_iter()
        .fold(application, |application, face| application.font(face))
        .default_font(ui::fonts::BODY)
        .title(TITLE)
        .subscription(App::subscription)
        .theme(App::theme)
        .scale_factor(App::scale_factor)
        .window(window_settings())
        .exit_on_close_request(false)
        .run();
    if let Some(dir) = cleanup {
        // Best effort: nothing useful to do if it fails.
        let _ = std::fs::remove_dir_all(dir);
    }
    // **Restart now** after an AppImage update: the new AppImage, now that
    // this one has let go of its stores.
    if let Some(program) = twicecat_app::updates::take_restart() {
        let _ = std::process::Command::new(program).spawn();
    }
    result.map_err(|error| format!("the window failed: {error}"))
}

/// The window's settings: its icon, and on Linux its app ID, the desktop
/// entry's name (`packaging/linux/`), so the compositor and the shortcuts
/// portal tie the window to it (Wayland compositors take the icon from the
/// entry instead).
fn window_settings() -> iced::window::Settings {
    // The width the layout assumes until the first resize event arrives.
    let size = iced::Size::new(twicecat_app::layout::DEFAULT_WINDOW_WIDTH, 800.0);
    iced::window::Settings {
        size,
        icon: ui::app_icon::window_icon(),
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: twicecat_ptt::APP_ID.to_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `twicecat <version>`, as `--version` prints it.
fn version_line() -> String {
    format!("twicecat {}", env!("CARGO_PKG_VERSION"))
}

/// Gives a Windows GUI build the console of the terminal that started it,
/// so `--help`, `--version` and argument errors are seen. Output already
/// redirected to a pipe or a file is left as it is; started from the desktop
/// there is no console, and the text goes nowhere.
#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    // SAFETY: both calls take plain values and only change this process's
    // standard handles, before anything else uses them.
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_err() {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    // SAFETY: the first thing `main` does, before any thread exists.
    unsafe {
        twicecat_core::prepare_appimage_tls();
    }
    twicecat_core::require_cpu_level();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            attach_parent_console();
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            attach_parent_console();
            println!("{}", version_line());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            attach_parent_console();
            eprintln!("twicecat: {message}\nTry --help.");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = logging::init(LOG_FILTER) {
        eprintln!("twicecat: logging not started: {error}");
    }
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("twicecat: {message}");
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
    fn window_title_is_twicecat() {
        assert_eq!(TITLE, "Twicecat");
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
        assert_eq!(parse_args(&args(&["--version"])), Ok(Parsed::Version));
        assert_eq!(parse_args(&args(&["-V"])), Ok(Parsed::Version));
        assert_eq!(parse_args(&args(&[])).map(|p| p == Parsed::Help), Ok(false));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&args(&["--data-dir"])).is_err());
        assert!(parse_args(&args(&["--homeserver"])).is_err());
        assert!(parse_args(&args(&["--ephemeral", "--data-dir", "/d"])).is_err());
        assert!(parse_args(&args(&["--nope"])).is_err());
        assert!(parse_args(&args(&["--version", "--ephemeral"])).is_err());
        assert!(parse_args(&args(&["--data-dir", "/d", "--version"])).is_err());
        assert!(parse_args(&args(&["--help", "--version"])).is_err());
    }

    #[test]
    fn version_names_the_workspace_version() {
        assert_eq!(
            version_line(),
            format!("twicecat {}", env!("CARGO_PKG_VERSION"))
        );
        assert!(HELP.contains("--version"));
    }

    #[test]
    fn help_carries_the_shared_data_dir_note() {
        assert!(HELP.contains("twicecat-cli"));
        assert!(HELP.contains("at the same time"));
    }

    /// The desktop entry is named after the app ID the shortcuts portal and
    /// the window use, starts the binary, and is valid where
    /// `desktop-file-validate` is installed.
    #[test]
    fn the_desktop_entry_carries_the_app_id() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/linux")
            .join(format!("{}.desktop", twicecat_ptt::APP_ID));
        let entry = std::fs::read_to_string(&path).expect("the desktop entry exists");
        let lines: Vec<&str> = entry.lines().collect();
        assert_eq!(lines.first(), Some(&"[Desktop Entry]"));
        for wanted in [
            "Type=Application",
            "Name=Twicecat",
            "Exec=twicecat",
            &format!("StartupWMClass={}", twicecat_ptt::APP_ID),
            &format!("Icon={}", twicecat_ptt::APP_ID),
        ] {
            assert!(lines.contains(&wanted), "missing {wanted}");
        }
        assert_eq!(
            twicecat_ptt::APP_ID,
            twicecat_core::secrets::KEYCHAIN_SERVICE,
            "one name for the app everywhere"
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            window_settings().platform_specific.application_id,
            twicecat_ptt::APP_ID
        );
        // `Exec` takes no arguments: the app refuses unknown ones.
        assert!(parse_args(&args(&["https://matrix.to/#/@a:x"])).is_err());
        // Where it is not installed, the checks above stand.
        if let Ok(out) = std::process::Command::new("desktop-file-validate")
            .arg(&path)
            .output()
        {
            assert!(
                out.status.success() && out.stdout.is_empty(),
                "desktop-file-validate: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// The icon the entry names exists in every size packages install, the
    /// window gets it, and the AppStream metadata describes the same app
    /// (validated where `appstreamcli` is installed).
    #[test]
    fn the_icon_and_the_metainfo_carry_the_app_id() {
        let packaging = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging");
        let id = twicecat_ptt::APP_ID;
        assert!(
            packaging
                .join(format!("icons/hicolor/scalable/apps/{id}.svg"))
                .is_file()
        );
        for size in [16, 24, 32, 48, 64, 128, 256, 512] {
            let path = packaging.join(format!("icons/hicolor/{size}x{size}/apps/{id}.png"));
            let image = image::open(&path).expect("the icon decodes");
            assert_eq!((image.width(), image.height()), (size, size), "{path:?}");
        }
        assert!(packaging.join(format!("icons/{id}.ico")).is_file());
        assert!(window_settings().icon.is_some());

        let path = packaging.join(format!("linux/{id}.metainfo.xml"));
        let metainfo = std::fs::read_to_string(&path).expect("the metainfo exists");
        assert!(metainfo.contains(&format!("<id>{id}</id>")));
        assert!(metainfo.contains(&format!(
            "<launchable type=\"desktop-id\">{id}.desktop</launchable>"
        )));
        assert!(metainfo.contains("<binary>twicecat</binary>"));
        assert!(
            metainfo.contains(&format!(
                "<release version=\"{}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "the metainfo lists this version"
        );
        if let Ok(out) = std::process::Command::new("appstreamcli")
            .args(["validate", "--strict", "--no-net"])
            .arg(&path)
            .output()
        {
            assert!(
                out.status.success(),
                "appstreamcli: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn ephemeral_dirs_differ_between_calls() {
        assert_ne!(ephemeral_dir(), ephemeral_dir());
    }
}
