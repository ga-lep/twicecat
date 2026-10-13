//! On-disk locations for per-session stores, the last-account marker and the
//! local settings file.

use std::path::{Path, PathBuf};

use crate::api::{DensityChoice, PaletteChoice, RoomListChoice, ThemeChoice, UiSettings};

/// Largest settings file read (64 KiB); anything bigger is treated as corrupt.
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

/// Builds the settings from a parsed file, field by field: a missing field or
/// one with an unknown value takes that field's default.
fn settings_from_map(map: &serde_json::Map<String, serde_json::Value>) -> UiSettings {
    let text = |key: &str| map.get(key).and_then(serde_json::Value::as_str);
    let flag = |key: &str, default: bool| {
        map.get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    };
    let defaults = UiSettings::default();
    UiSettings {
        theme: match text("theme") {
            Some("System") => ThemeChoice::System,
            Some("Light") => ThemeChoice::Light,
            Some("Dark") => ThemeChoice::Dark,
            _ => defaults.theme,
        },
        desktop_notifications: flag("desktop_notifications", defaults.desktop_notifications),
        show_message_content: flag("show_message_content", defaults.show_message_content),
        tray_badge: flag("tray_badge", defaults.tray_badge),
        palette: match text("palette") {
            Some("Twicecat") => Some(PaletteChoice::Twicecat),
            Some("Frost") => Some(PaletteChoice::Frost),
            Some("Ember") => Some(PaletteChoice::Ember),
            Some("HighContrast") => Some(PaletteChoice::HighContrast),
            _ => None,
        },
        density: match text("density") {
            Some("Compact") => DensityChoice::Compact,
            Some("Comfortable") => DensityChoice::Comfortable,
            _ => defaults.density,
        },
        room_list: match text("room_list") {
            Some("Rail") => RoomListChoice::Rail,
            Some("Expanded") => RoomListChoice::Expanded,
            _ => defaults.room_list,
        },
        text_scale_percent: map
            .get("text_scale_percent")
            .and_then(serde_json::Value::as_u64)
            .map_or(defaults.text_scale_percent, clamp_text_scale),
    }
}

/// Clamps a text scale to 80-200 % and rounds it to a step of 10.
fn clamp_text_scale(raw: u64) -> u16 {
    let stepped = raw.saturating_add(5) / 10 * 10;
    u16::try_from(stepped.clamp(80, 200)).expect("clamped to at most 200")
}

/// Errors from building paths.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    /// The session id is empty or could escape the sessions directory.
    #[error("invalid session id")]
    InvalidSessionId,
    /// A directory could not be created or removed.
    #[error("could not access the data directory")]
    Io,
}

/// Checks a session id against the allowlist: 1..=128 characters of
/// `[A-Za-z0-9_-]`, not a Windows reserved device name.
pub(crate) fn validate_session_id(id: &str) -> Result<(), PathsError> {
    let chars_ok = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if chars_ok && !is_windows_reserved(id) {
        Ok(())
    } else {
        Err(PathsError::InvalidSessionId)
    }
}

fn is_windows_reserved(id: &str) -> bool {
    let upper = id.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            upper
                .strip_prefix(p)
                .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
        })
}

/// `create_dir_all` with mode 0o700 on Unix.
fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Reads a regular file of at most [`MAX_SETTINGS_BYTES`]. `None` for anything
/// else (missing, a directory or device, too big, unreadable).
fn read_capped_regular_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_SETTINGS_BYTES).then_some(bytes)
}

/// Root of all Twicecat data on disk.
#[derive(Debug, Clone)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    /// Uses the OS data directory; `None` if it cannot be determined.
    pub fn from_os() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("io.github", "ga_lep", "Twicecat")?;
        Some(Self::at(dirs.data_dir()))
    }

    /// Uses `root` as the data directory (tests, `--data-dir`).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/sessions/<session_id>/`.
    ///
    /// The id must be 1..=128 characters of `[A-Za-z0-9_-]` and must not be a
    /// Windows reserved device name (`CON`, `NUL`, `COM1`, ...).
    pub fn session_store_dir(&self, session_id: &str) -> Result<PathBuf, PathsError> {
        validate_session_id(session_id)?;
        Ok(self.root.join("sessions").join(session_id))
    }

    /// Creates `<root>/sessions/<session_id>/` (and the root) and returns it.
    /// On Unix every directory created gets mode 0o700.
    pub fn create_session_store_dir(&self, session_id: &str) -> Result<PathBuf, PathsError> {
        let dir = self.session_store_dir(session_id)?;
        create_private_dir_all(&dir).map_err(|_| PathsError::Io)?;
        Ok(dir)
    }

    /// Reads the last-account marker. `Ok(None)` if there is none or it is
    /// empty.
    pub fn read_last_account(&self) -> std::io::Result<Option<String>> {
        match std::fs::read_to_string(self.last_account_file()) {
            Ok(text) => {
                let text = text.trim();
                Ok((!text.is_empty()).then(|| text.to_owned()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes the last-account marker (a user ID, never a secret). On Unix the
    /// file gets mode 0o600 and the root 0o700.
    pub fn write_last_account(&self, user_id: &str) -> std::io::Result<()> {
        use std::io::Write as _;

        create_private_dir_all(&self.root)?;
        let path = self.last_account_file();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        // `mode` applies only when the file is created; fix an older file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(user_id.as_bytes())
    }

    /// Removes the last-account marker. A missing file is `Ok`.
    pub fn remove_last_account(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.last_account_file()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Removes a session store directory. A missing directory is `Ok`.
    /// Blocking: on Windows it waits (at most [`STORE_RELEASE_WAIT`]) for
    /// files still in use, see [`remove_dir_all_when_released`].
    pub fn remove_session_store_dir(&self, session_id: &str) -> Result<(), PathsError> {
        let dir = self.session_store_dir(session_id)?;
        match remove_dir_all_when_released(&dir, STORE_RELEASE_WAIT) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(PathsError::Io),
            _ => Ok(()),
        }
    }

    /// `<root>/settings.json`: the typed [`UiSettings`], no secrets, no content.
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Reads the local settings. A missing file gives the defaults. A file that
    /// cannot be read or parsed also gives the defaults, and a corrupt one is
    /// renamed `settings.json.bad` so the next write does not hide it. Unknown
    /// fields are ignored and missing ones take their default.
    pub fn read_settings(&self) -> UiSettings {
        let path = self.settings_file();
        let Some(bytes) = read_capped_regular_file(&path) else {
            return UiSettings::default();
        };
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(serde_json::Value::Object(map)) => settings_from_map(&map),
            _ => {
                // Best effort: if the rename fails the defaults still apply.
                let _ = std::fs::rename(&path, self.root.join("settings.json.bad"));
                UiSettings::default()
            }
        }
    }

    /// Writes the local settings atomically: a temporary file in the data root
    /// (mode 0o600 on Unix), then a rename over `settings.json`.
    pub fn write_settings(&self, settings: &UiSettings) -> std::io::Result<()> {
        use std::io::Write as _;

        create_private_dir_all(&self.root)?;
        // Built by hand: the serializer guard in `session::tests` allows no
        // serializing outside `session.rs`.
        let theme = match settings.theme {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        };
        let mut json = serde_json::json!({
            "theme": theme,
            "desktop_notifications": settings.desktop_notifications,
            "show_message_content": settings.show_message_content,
            "tray_badge": settings.tray_badge,
            "density": match settings.density {
                DensityChoice::Comfortable => "Comfortable",
                DensityChoice::Compact => "Compact",
            },
            "room_list": match settings.room_list {
                RoomListChoice::Expanded => "Expanded",
                RoomListChoice::Rail => "Rail",
            },
            "text_scale_percent": settings.text_scale_percent,
        });
        // No key while `None`, so a later change of the app's default reaches
        // this user.
        if let (Some(palette), Some(map)) = (settings.palette, json.as_object_mut()) {
            let name = match palette {
                PaletteChoice::Twicecat => "Twicecat",
                PaletteChoice::Frost => "Frost",
                PaletteChoice::Ember => "Ember",
                PaletteChoice::HighContrast => "HighContrast",
            };
            map.insert("palette".into(), name.into());
        }
        let json = json.to_string();
        let tmp = self.root.join("settings.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let result = (|| {
            // Never write through a leftover file or symlink at the temp path.
            match std::fs::remove_file(&tmp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let mut file = options.open(&tmp)?;
            // `mode` applies only when the file is created; fix a stale one.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, self.settings_file())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// `<root>/last_account`: holds only a user ID, never a secret.
    pub fn last_account_file(&self) -> PathBuf {
        self.root.join("last_account")
    }

    /// `<root>/call_settings.json`: audio devices, processing, input mode and
    /// per-user volumes. No secrets.
    pub fn call_settings_file(&self) -> PathBuf {
        self.root.join("call_settings.json")
    }

    /// Reads the call settings file; a missing file is `Ok(None)`.
    pub fn read_call_settings(&self) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.call_settings_file()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replaces the call settings file atomically: writes a temporary file
    /// next to it, then renames it over the old one, so a crash leaves either
    /// the old or the new file. On Unix the file gets mode 0o600 and the
    /// root 0o700.
    pub fn write_call_settings(&self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write as _;

        create_private_dir_all(&self.root)?;
        let path = self.call_settings_file();
        let tmp = self.root.join("call_settings.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    }
}

/// How long removing a store waits for its files to be released.
const STORE_RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// `remove_dir_all`, retried while a file in `dir` is still open. Windows
/// cannot delete an open file: the SDK's SQLite stores close their
/// connections shortly after the client is dropped (25 to 50 ms measured),
/// so a removal right after a failed login or a logout found them in use
/// and left the session's store behind. Elsewhere open files are unlinked
/// and the first attempt is the only one.
fn remove_dir_all_when_released(dir: &Path, wait: std::time::Duration) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match std::fs::remove_dir_all(dir) {
            Err(e) if in_use(&e) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

/// Whether `e` says a file is open elsewhere: Windows' sharing violation,
/// or access denied for a file already marked for deletion.
fn in_use(e: &std::io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    cfg!(windows)
        && matches!(
            e.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file opened as SQLite opens its database on Windows: shared for
    /// reading and writing, not for deletion (std's `File` allows deletion).
    #[cfg(windows)]
    fn open_like_sqlite(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_WRITE: u32 = 2;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(path)
            .unwrap()
    }

    #[test]
    #[cfg(windows)]
    fn a_store_still_open_is_removed_once_released() {
        let root = tempfile::tempdir().unwrap();
        let p = DataPaths::at(root.path().to_path_buf());
        let dir = p.create_session_store_dir("abc").unwrap();
        let file = open_like_sqlite(&dir.join("store.sqlite3"));
        // Closed 100 ms from now, as the SDK's stores are after a drop.
        let closer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(file);
        });
        p.remove_session_store_dir("abc").unwrap();
        closer.join().unwrap();
        assert!(!dir.exists());
    }

    #[test]
    #[cfg(windows)]
    fn a_store_never_released_is_an_error_after_the_wait() {
        let root = tempfile::tempdir().unwrap();
        let p = DataPaths::at(root.path().to_path_buf());
        let dir = p.create_session_store_dir("abc").unwrap();
        let file = open_like_sqlite(&dir.join("store.sqlite3"));
        let started = std::time::Instant::now();
        let result = remove_dir_all_when_released(&dir, std::time::Duration::from_millis(200));
        assert!(result.is_err());
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
        drop(file);
    }

    #[test]
    fn paths_are_under_root() {
        let p = DataPaths::at("/data/root");
        assert_eq!(p.root(), Path::new("/data/root"));
        let dir = p.session_store_dir("abc123").unwrap();
        assert_eq!(dir, Path::new("/data/root/sessions/abc123"));
        assert!(dir.starts_with(p.root()));
        assert_eq!(p.last_account_file(), Path::new("/data/root/last_account"));
    }

    #[test]
    fn bad_session_ids_are_rejected() {
        let p = DataPaths::at("/data/root");
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "../x",
            "a\\b",
            "..\\x",
            "/abs",
            "a\0b",
            "C:x",
            "C:",
            "a:b",
            "CON",
            "x.",
            "a b",
            "é",
            "a.b",
            &"a".repeat(129),
        ] {
            assert_eq!(
                p.session_store_dir(bad),
                Err(PathsError::InvalidSessionId),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn good_session_ids_are_accepted() {
        let p = DataPaths::at("/data/root");
        for good in [
            "a",
            "abc_DEF-123",
            "0123456789abcdef",
            &"a".repeat(128),
            "CONX",
            "COM10",
        ] {
            assert!(p.session_store_dir(good).is_ok(), "{good:?}");
        }
        for reserved in ["con", "NUL", "Com1", "lpt9", "aux", "PRN"] {
            assert!(p.session_store_dir(reserved).is_err(), "{reserved:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_modes_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let dir = p.create_session_store_dir("abc").unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(p.root()), 0o700);
        assert_eq!(mode(&p.root().join("sessions")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        p.write_last_account("@a:x").unwrap();
        assert_eq!(mode(&p.last_account_file()), 0o600);
        // An older, world-readable file is tightened.
        std::fs::set_permissions(
            p.last_account_file(),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        p.write_last_account("@b:x").unwrap();
        assert_eq!(mode(&p.last_account_file()), 0o600);
        assert_eq!(p.read_last_account().unwrap().as_deref(), Some("@b:x"));
        p.remove_last_account().unwrap();
        p.remove_last_account().unwrap();
        assert_eq!(p.read_last_account().unwrap(), None);
        p.remove_session_store_dir("abc").unwrap();
        p.remove_session_store_dir("abc").unwrap();
        assert!(!dir.exists());
    }

    /// Q16 (c): after a session starts, the data root holds only the
    /// last-account marker and the store directory (which the SDK fills with
    /// its encrypted store). No token or passphrase file lives next to them.
    #[test]
    fn the_data_root_holds_only_the_marker_and_the_sessions_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.create_session_store_dir("abc").unwrap();
        p.write_last_account("@a:x").unwrap();
        p.write_settings(&UiSettings::default()).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(p.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["last_account", "sessions", "settings.json"]);
        let sessions: Vec<String> = std::fs::read_dir(p.root().join("sessions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(sessions, ["abc"]);
        // The store directory is empty until the SDK writes its database.
        assert_eq!(
            std::fs::read_dir(p.root().join("sessions/abc"))
                .unwrap()
                .count(),
            0
        );
        // The marker holds the user ID and nothing else.
        assert_eq!(p.read_last_account().unwrap().as_deref(), Some("@a:x"));
    }

    #[test]
    fn settings_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        assert_eq!(p.settings_file(), tmp.path().join("root/settings.json"));
        assert_eq!(p.read_settings(), UiSettings::default());
    }

    #[test]
    fn settings_round_trip_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let s = UiSettings {
            theme: ThemeChoice::Dark,
            desktop_notifications: false,
            show_message_content: false,
            tray_badge: false,
            ..UiSettings::default()
        };
        p.write_settings(&s).unwrap();
        assert_eq!(p.read_settings(), s);
        // Overwriting works and leaves no temp file behind.
        p.write_settings(&UiSettings::default()).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        let names: Vec<_> = std::fs::read_dir(p.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[test]
    fn corrupt_settings_give_defaults_and_are_set_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(p.settings_file(), b"{ not json").unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        assert!(!p.settings_file().exists());
        assert_eq!(
            std::fs::read(p.root().join("settings.json.bad")).unwrap(),
            b"{ not json"
        );
    }

    #[test]
    fn settings_ignore_unknown_fields_and_fill_missing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(
            p.settings_file(),
            br#"{"theme":"Light","tray_badge":false,"from_the_future":{"x":1}}"#,
        )
        .unwrap();
        let s = p.read_settings();
        assert_eq!(s.theme, ThemeChoice::Light);
        assert!(!s.tray_badge);
        assert!(s.desktop_notifications && s.show_message_content);
        assert!(p.settings_file().exists());
    }

    fn read_text(p: &DataPaths, text: &str) -> UiSettings {
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(p.settings_file(), text).unwrap();
        p.read_settings()
    }

    #[test]
    fn an_old_file_reads_with_the_appearance_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let s = read_text(&p, r#"{"theme":"Dark"}"#);
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.palette, None);
        assert_eq!(s.density, DensityChoice::Comfortable);
        assert_eq!(s.room_list, RoomListChoice::Expanded);
        assert_eq!(s.text_scale_percent, 100);
    }

    #[test]
    fn an_unknown_appearance_value_falls_back_per_field() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let s = read_text(
            &p,
            r#"{"palette":"Nope","theme":"Dark","density":"Huge","room_list":7,"text_scale_percent":"x","tray_badge":false}"#,
        );
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.palette, None);
        assert_eq!(s.density, DensityChoice::Comfortable);
        assert_eq!(s.room_list, RoomListChoice::Expanded);
        assert_eq!(s.text_scale_percent, 100);
        assert!(!s.tray_badge);
        assert!(p.settings_file().exists());
        assert!(!p.root().join("settings.json.bad").exists());
    }

    #[test]
    fn text_scale_is_clamped_and_stepped_when_read() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for (raw, want) in [
            (0u32, 80u16),
            (79, 80),
            (85, 90),
            (200, 200),
            (250, 200),
            (u32::from(u16::MAX), 200),
        ] {
            let s = read_text(&p, &format!(r#"{{"text_scale_percent":{raw}}}"#));
            assert_eq!(s.text_scale_percent, want, "{raw}");
        }
    }

    #[test]
    fn a_palette_of_none_leaves_no_key_in_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.write_settings(&UiSettings::default()).unwrap();
        let text = std::fs::read_to_string(p.settings_file()).unwrap();
        assert!(!text.contains("palette"), "{text}");
    }

    #[test]
    fn every_appearance_value_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let palettes = [
            None,
            Some(PaletteChoice::Twicecat),
            Some(PaletteChoice::Frost),
            Some(PaletteChoice::Ember),
            Some(PaletteChoice::HighContrast),
        ];
        for palette in palettes {
            for density in [DensityChoice::Comfortable, DensityChoice::Compact] {
                for room_list in [RoomListChoice::Expanded, RoomListChoice::Rail] {
                    for text_scale_percent in [80, 100, 130, 200] {
                        let s = UiSettings {
                            palette,
                            density,
                            room_list,
                            text_scale_percent,
                            ..UiSettings::default()
                        };
                        p.write_settings(&s).unwrap();
                        assert_eq!(p.read_settings(), s);
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn settings_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.write_settings(&UiSettings::default()).unwrap();
        let mode = std::fs::metadata(p.settings_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn from_os_returns_some() {
        let p = DataPaths::from_os().expect("OS data dir available on the test machine");
        assert!(p.root().is_absolute());
        assert!(p.last_account_file().starts_with(p.root()));
    }

    #[test]
    fn every_theme_and_bool_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for theme in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            for bits in 0..8u8 {
                let s = UiSettings {
                    theme,
                    desktop_notifications: bits & 1 != 0,
                    show_message_content: bits & 2 != 0,
                    tray_badge: bits & 4 != 0,
                    ..UiSettings::default()
                };
                p.write_settings(&s).unwrap();
                assert_eq!(p.read_settings(), s);
            }
        }
    }

    #[test]
    fn call_settings_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        assert_eq!(paths.read_call_settings().unwrap(), None);
        paths.write_call_settings(b"{\"a\":1}").unwrap();
        paths.write_call_settings(b"{\"a\":2}").unwrap();
        assert_eq!(
            paths.read_call_settings().unwrap().as_deref(),
            Some(&b"{\"a\":2}"[..])
        );
        assert!(!dir.path().join("root/call_settings.json.tmp").exists());
        assert!(paths.call_settings_file().starts_with(paths.root()));
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_loose_temp_file_is_tightened_before_the_write() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        let stale = p.root().join("settings.json.tmp");
        std::fs::write(&stale, b"old").unwrap();
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o666)).unwrap();
        p.write_settings(&UiSettings::default()).unwrap();
        let mode = std::fs::metadata(p.settings_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn an_oversized_settings_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        let mut big = br#"{"theme":"Dark","pad":""#.to_vec();
        big.resize(MAX_SETTINGS_BYTES as usize + 10, b'a');
        big.extend_from_slice(b"\"}");
        std::fs::write(p.settings_file(), &big).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
    }

    #[test]
    fn a_settings_path_that_is_not_a_regular_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.settings_file()).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        assert!(p.settings_file().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn call_settings_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        paths.write_call_settings(b"{}").unwrap();
        let mode = std::fs::metadata(paths.call_settings_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
