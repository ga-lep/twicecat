//! On-disk locations for per-session stores, the last-account marker and the
//! local settings file.

use std::path::{Path, PathBuf};

use crate::api::{ThemeChoice, UiSettings};

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

/// Root of all Tessera data on disk.
#[derive(Debug, Clone)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    /// Uses the OS data directory; `None` if it cannot be determined.
    pub fn from_os() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("io.github", "ga_lep", "Tessera")?;
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
    pub fn remove_session_store_dir(&self, session_id: &str) -> Result<(), PathsError> {
        let dir = self.session_store_dir(session_id)?;
        match std::fs::remove_dir_all(dir) {
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
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => return UiSettings::default(),
        };
        match serde_json::from_slice(&bytes) {
            Ok(settings) => settings,
            Err(_) => {
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
        let json = serde_json::json!({
            "theme": theme,
            "desktop_notifications": settings.desktop_notifications,
            "show_message_content": settings.show_message_content,
            "tray_badge": settings.tray_badge,
        })
        .to_string();
        let tmp = self.root.join("settings.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&tmp)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
