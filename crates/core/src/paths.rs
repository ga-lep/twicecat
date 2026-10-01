//! On-disk locations for per-session stores and the last-account marker.

use std::path::{Path, PathBuf};

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

    /// `<root>/last_account`: holds only a user ID, never a secret.
    pub fn last_account_file(&self) -> PathBuf {
        self.root.join("last_account")
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

    #[test]
    fn from_os_returns_some() {
        let p = DataPaths::from_os().expect("OS data dir available on the test machine");
        assert!(p.root().is_absolute());
        assert!(p.last_account_file().starts_with(p.root()));
    }
}
