//! Secret storage behind a small trait, with an OS keychain backend and an
//! in-memory backend for tests and `--ephemeral` runs.
//!
//! All calls are blocking. The engine must call them through
//! `tokio::task::spawn_blocking`.

use std::collections::HashMap;
use std::sync::Mutex;

use thiserror::Error;
use zeroize::Zeroizing;

/// Keychain service name used in production (plan Decision 5).
pub const KEYCHAIN_SERVICE: &str = "io.github.ga_lep.Twicecat";

/// Keychain service name used only by the ignored real-keychain test, so it
/// can never collide with a production entry (plan Decision 7).
pub const TEST_KEYCHAIN_SERVICE: &str = "io.github.ga_lep.Twicecat.test";

/// A failure of the secret store. Messages never contain secret values.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The backend failed (keychain locked, no Secret Service, ...).
    #[error("secret store backend error: {0}")]
    Backend(String),
    /// The in-memory store's lock was poisoned by a panicking thread.
    #[error("secret store lock poisoned")]
    Poisoned,
}

/// A place to keep small secrets, keyed by account name. Returned secrets are
/// zeroized when dropped.
pub trait SecretStore: Send + Sync {
    /// Returns the secret for `account`, or `None` if there is none.
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;
    /// Stores `secret` for `account`, replacing any previous value.
    fn set(&self, account: &str, secret: &[u8]) -> Result<(), SecretError>;
    /// Removes the secret for `account`. A missing entry is `Ok`.
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

type Entries = HashMap<String, Zeroizing<Vec<u8>>>;

/// A [`SecretStore`] that keeps everything in process memory.
///
/// Debug output shows only the number of entries.
#[derive(Default)]
pub struct InMemorySecretStore {
    entries: Mutex<Entries>,
}

impl InMemorySecretStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for InMemorySecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.entries.lock().map(|e| e.len()).ok();
        f.debug_struct("InMemorySecretStore")
            .field("entries", &len)
            .finish()
    }
}

impl InMemorySecretStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Entries>, SecretError> {
        self.entries.lock().map_err(|_| SecretError::Poisoned)
    }
}

impl SecretStore for InMemorySecretStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        Ok(self.lock()?.get(account).cloned())
    }

    fn set(&self, account: &str, secret: &[u8]) -> Result<(), SecretError> {
        self.lock()?
            .insert(account.to_owned(), Zeroizing::new(secret.to_vec()));
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.lock()?.remove(account);
        Ok(())
    }
}

/// A [`SecretStore`] backed by the OS keychain (Keychain, Credential Manager,
/// or Secret Service) through the `keyring` crate.
#[derive(Debug, Clone)]
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    /// Creates a store that files entries under `service`. Production code
    /// passes [`KEYCHAIN_SERVICE`].
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, account).map_err(backend)
    }
}

fn backend(e: keyring::Error) -> SecretError {
    SecretError::Backend(e.to_string())
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        match self.entry(account)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    fn set(&self, account: &str, secret: &[u8]) -> Result<(), SecretError> {
        self.entry(account)?.set_secret(secret).map_err(backend)
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_round_trip() {
        let store = InMemorySecretStore::new();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", b"one").unwrap();
        assert_eq!(
            store.get("a").unwrap().as_deref().map(Vec::as_slice),
            Some(&b"one"[..])
        );
        store.set("a", b"two").unwrap();
        assert_eq!(
            store.get("a").unwrap().as_deref().map(Vec::as_slice),
            Some(&b"two"[..])
        );
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
    }

    #[test]
    fn deleting_missing_entry_is_ok() {
        let store = InMemorySecretStore::new();
        store.delete("nope").unwrap();
    }

    #[test]
    fn accounts_are_independent() {
        let store = InMemorySecretStore::new();
        store.set("a", b"1").unwrap();
        store.set("b", b"2").unwrap();
        store.delete("a").unwrap();
        assert_eq!(
            store.get("b").unwrap().as_deref().map(Vec::as_slice),
            Some(&b"2"[..])
        );
    }

    #[test]
    fn in_memory_debug_hides_contents() {
        let store = InMemorySecretStore::new();
        store.set("acct-x", b"sentinel-secret-x").unwrap();
        let dbg = format!("{store:?}");
        assert!(!dbg.contains("sentinel-secret-x"));
        assert!(!dbg.contains("acct-x"));
    }

    #[test]
    fn test_service_differs_from_production() {
        assert_ne!(TEST_KEYCHAIN_SERVICE, KEYCHAIN_SERVICE);
    }

    /// Deletes the throwaway keychain entry even if the test panics.
    struct KeyringCleanup {
        service: &'static str,
        account: String,
    }

    impl Drop for KeyringCleanup {
        fn drop(&mut self) {
            // Cleanup is best effort: ignore a missing entry or a backend error.
            if let Ok(entry) = keyring::Entry::new(self.service, &self.account) {
                let _ = entry.delete_credential();
            }
        }
    }

    // Human-only (plan Decision 7). The name contains `keyring` so that
    // `--skip keyring` excludes exactly this test.
    #[test]
    #[ignore = "touches the real OS keychain"]
    fn keyring_real_keychain_round_trip() {
        let account = format!("test-account-{}", std::process::id());
        let _guard = KeyringCleanup {
            service: TEST_KEYCHAIN_SERVICE,
            account: account.clone(),
        };
        let store = KeyringSecretStore::new(TEST_KEYCHAIN_SERVICE);
        assert_eq!(store.get(&account).unwrap(), None);
        store.set(&account, b"keyring-test-secret").unwrap();
        assert_eq!(
            store.get(&account).unwrap().as_deref().map(Vec::as_slice),
            Some(&b"keyring-test-secret"[..])
        );
        store.delete(&account).unwrap();
        assert_eq!(store.get(&account).unwrap(), None);
    }
}
