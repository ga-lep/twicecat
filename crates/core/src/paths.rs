//! On-disk locations for per-session stores and the last-account marker.

use std::path::{Path, PathBuf};

/// Errors from building paths.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    /// The session id is empty or could escape the sessions directory.
    #[error("invalid session id")]
    InvalidSessionId,
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
    pub fn session_store_dir(&self, session_id: &str) -> Result<PathBuf, PathsError> {
        let bad = session_id.is_empty()
            || session_id == "."
            || session_id.contains("..")
            || session_id.contains(['/', '\\', '\0']);
        if bad {
            return Err(PathsError::InvalidSessionId);
        }
        Ok(self.root.join("sessions").join(session_id))
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
            "", ".", "..", "a/b", "../x", "a\\b", "..\\x", "/abs", "a\0b",
        ] {
            assert_eq!(
                p.session_store_dir(bad),
                Err(PathsError::InvalidSessionId),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn from_os_returns_some() {
        let p = DataPaths::from_os().expect("OS data dir available on the test machine");
        assert!(p.root().is_absolute());
        assert!(p.last_account_file().starts_with(p.root()));
    }
}
