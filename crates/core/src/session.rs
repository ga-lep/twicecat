//! The persisted session record: everything needed to restore a login.
//!
//! The record holds secrets (access and refresh tokens, the store
//! passphrase). It lives only in the secret store, and its `Debug` output is
//! redacted. This module is private, so it may name SDK types.

use std::fmt;

use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::authentication::oauth::UserSession;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::api::CoreError;
use crate::paths::validate_session_id;

/// Current record format version.
pub(crate) const RECORD_VERSION: u32 = 1;

/// Windows Credential Manager limits a credential blob to 2560 bytes.
pub(crate) const MAX_RECORD_BYTES: usize = 2560;

/// How the session was authenticated.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum SessionAuth {
    /// Classic Matrix password login.
    Matrix(MatrixSession),
    /// OAuth 2.0 / MAS login.
    OAuth {
        /// The client ID obtained at registration.
        client_id: String,
        /// The user session with its tokens.
        user: UserSession,
    },
}

impl fmt::Debug for SessionAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matrix(_) => f.write_str("SessionAuth::Matrix(<redacted>)"),
            Self::OAuth { .. } => f.write_str("SessionAuth::OAuth(<redacted>)"),
        }
    }
}

impl SessionAuth {
    /// The SDK session to hand to `Client::restore_session`.
    pub(crate) fn to_auth_session(&self) -> matrix_sdk::AuthSession {
        match self {
            Self::Matrix(session) => matrix_sdk::AuthSession::Matrix(session.clone()),
            Self::OAuth { client_id, user } => matrix_sdk::AuthSession::OAuth(Box::new(
                matrix_sdk::authentication::oauth::OAuthSession {
                    client_id: matrix_sdk::authentication::oauth::ClientId::new(client_id.clone()),
                    user: user.clone(),
                },
            )),
        }
    }

    /// The record form of a live client session; `None` for a future session
    /// kind this code does not know.
    pub(crate) fn from_auth_session(session: matrix_sdk::AuthSession) -> Option<Self> {
        match session {
            matrix_sdk::AuthSession::Matrix(s) => Some(Self::Matrix(s)),
            matrix_sdk::AuthSession::OAuth(s) => Some(Self::OAuth {
                client_id: s.client_id.as_str().to_owned(),
                user: s.user,
            }),
            _ => None,
        }
    }
}

/// A restorable session, stored as JSON in the secret store.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecord {
    /// Format version, always [`RECORD_VERSION`] when written.
    pub version: u32,
    /// The account's user ID.
    pub user_id: String,
    /// The homeserver URL the session was created against.
    pub homeserver_url: String,
    /// The store directory name under `<data>/sessions/`.
    pub session_id: String,
    /// Passphrase of the encrypted SQLite store.
    pub store_passphrase: Zeroizing<String>,
    /// Tokens.
    pub auth: SessionAuth,
}

impl fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionRecord")
            .field("version", &self.version)
            .field("user_id", &self.user_id)
            .field("homeserver_url", &self.homeserver_url)
            .field("session_id", &self.session_id)
            .field("store_passphrase", &"<redacted>")
            .field("auth", &self.auth)
            .finish()
    }
}

impl SessionRecord {
    /// Serializes the record, refusing to exceed [`MAX_RECORD_BYTES`].
    ///
    /// Errors carry no record content: serde messages can quote values.
    pub(crate) fn to_bytes(&self) -> Result<Zeroizing<Vec<u8>>, CoreError> {
        let bytes = serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| CoreError::Storage("could not serialize session record".into()))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(CoreError::Storage(format!(
                "session record is {} bytes, over the {MAX_RECORD_BYTES} byte keychain limit",
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    /// Parses a record written by [`SessionRecord::to_bytes`].
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, CoreError> {
        let record: Self = serde_json::from_slice(bytes)
            .map_err(|_| CoreError::Storage("stored session record is unreadable".into()))?;
        if record.version != RECORD_VERSION {
            return Err(CoreError::Storage(format!(
                "unsupported session record version {}",
                record.version
            )));
        }
        validate_session_id(&record.session_id)
            .map_err(|_| CoreError::Storage("stored session record is invalid".into()))?;
        validate_homeserver_url(&record.homeserver_url)?;
        let auth_user = match &record.auth {
            SessionAuth::Matrix(s) => &s.meta.user_id,
            SessionAuth::OAuth { user, .. } => &user.meta.user_id,
        };
        if auth_user.as_str() != record.user_id {
            return Err(CoreError::Storage(
                "stored session record is inconsistent".into(),
            ));
        }
        Ok(record)
    }
}

/// Accepts `https` URLs, and `http` only for loopback hosts (the local test
/// homeserver). Errors carry no part of the URL.
fn validate_homeserver_url(raw: &str) -> Result<(), CoreError> {
    if is_allowed_homeserver_url(raw) {
        Ok(())
    } else {
        Err(CoreError::Storage(
            "stored session record has an invalid homeserver".into(),
        ))
    }
}

/// The homeserver transport policy, shared by restore and login: `https`, or
/// `http` only for `localhost` and loopback addresses.
pub(crate) fn is_allowed_homeserver_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => true,
        ("http", Some(url::Host::Domain(d))) => d.eq_ignore_ascii_case("localhost"),
        ("http", Some(url::Host::Ipv4(ip))) => ip.is_loopback(),
        ("http", Some(url::Host::Ipv6(ip))) => ip.is_loopback(),
        _ => false,
    }
}

/// Generates a random store directory name: 16 random bytes, hex encoded
/// (32 characters, valid per [`validate_session_id`]). Not secret.
pub(crate) fn generate_session_id() -> String {
    use std::fmt::Write as _;

    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes[..]);
    bytes
        .iter()
        .fold(String::with_capacity(32), |mut out, byte| {
            // Writing to a String cannot fail.
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Generates a store passphrase: 32 random bytes from the OS-seeded CSPRNG,
/// hex encoded (64 characters).
pub(crate) fn generate_store_passphrase() -> Zeroizing<String> {
    use std::fmt::Write as _;

    let mut bytes = Zeroizing::new([0u8; 32]);
    // `rand::fill` uses the thread-local ChaCha CSPRNG, seeded from the OS.
    rand::fill(&mut bytes[..]);
    let mut out = Zeroizing::new(String::with_capacity(64));
    for byte in bytes.iter() {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    // `bytes` is zeroized on drop here.
    out
}

#[cfg(test)]
mod tests {
    use matrix_sdk::SessionTokens;
    use matrix_sdk::ruma::{owned_device_id, owned_user_id};

    use super::*;

    const ACCESS: &str = "sentinel-access-token-x";
    const REFRESH: &str = "sentinel-refresh-token-x";
    const PASS: &str = "sentinel-passphrase-x";

    fn meta() -> matrix_sdk::SessionMeta {
        matrix_sdk::SessionMeta {
            user_id: owned_user_id!("@alice:example.org"),
            device_id: owned_device_id!("DEVICEID"),
        }
    }

    fn tokens(access: &str, refresh: Option<&str>) -> SessionTokens {
        SessionTokens {
            access_token: access.to_owned(),
            refresh_token: refresh.map(str::to_owned),
        }
    }

    fn matrix_record() -> SessionRecord {
        SessionRecord {
            version: RECORD_VERSION,
            user_id: "@alice:example.org".into(),
            homeserver_url: "https://matrix.example.org/".into(),
            session_id: "session-1".into(),
            store_passphrase: PASS.to_owned().into(),
            auth: SessionAuth::Matrix(MatrixSession {
                meta: meta(),
                tokens: tokens(ACCESS, Some(REFRESH)),
            }),
        }
    }

    fn oauth_record() -> SessionRecord {
        SessionRecord {
            auth: SessionAuth::OAuth {
                client_id: "sentinel-client-id-x".into(),
                user: UserSession {
                    meta: meta(),
                    tokens: tokens(ACCESS, Some(REFRESH)),
                },
            },
            ..matrix_record()
        }
    }

    #[test]
    fn homeserver_policy_on_resolved_urls() {
        for ok in [
            "https://matrix.example.org/",
            "https://127.0.0.1:8448/",
            "http://localhost:8008/",
            "http://LocalHost/",
            "http://127.0.0.1:8008/",
            "http://127.5.5.5/",
            "http://[::1]:8008/",
        ] {
            assert!(is_allowed_homeserver_url(ok), "{ok} should be allowed");
        }
        for bad in [
            "http://matrix.example.invalid/",
            "http://192.168.1.5:8008/",
            "http://localhost.example.org/",
            "http://[2001:db8::1]/",
            "ftp://localhost/",
            "not a url",
            "",
        ] {
            assert!(!is_allowed_homeserver_url(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn debug_hides_every_secret() {
        for rec in [matrix_record(), oauth_record()] {
            let dbg = format!("{rec:?}");
            for secret in [ACCESS, REFRESH, PASS, "sentinel-client-id-x"] {
                assert!(!dbg.contains(secret), "{secret} leaked in {dbg}");
            }
            assert!(dbg.contains("@alice:example.org"));
        }
    }

    /// Q16 (c): `to_bytes` is the only serializer of the tokens and the
    /// passphrase, and nothing it or its errors print carries them.
    #[test]
    fn to_bytes_is_the_only_serializer_and_its_errors_carry_no_secret() {
        // The oversize error quotes sizes, never content.
        let mut big = matrix_record();
        big.homeserver_url = format!(
            "https://matrix.example.org/{}",
            "a".repeat(MAX_RECORD_BYTES)
        );
        let err = big.to_bytes().unwrap_err();
        for text in [err.to_string(), format!("{err:?}")] {
            for secret in [ACCESS, REFRESH, PASS] {
                assert!(!text.contains(secret), "a secret leaked in an error");
            }
        }
        // The sentinels reach bytes only through `to_bytes`.
        let bytes = matrix_record().to_bytes().unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains(ACCESS) && text.contains(PASS));
        // No other module serializes anything: `Serialize` is derived only
        // here, and `serde_json::to_*` is called only here (and in its tests).
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|n| n != "session.rs")
                    // call_settings.rs, call.rs and types.rs serialize non-secret
                    // audio settings, room IDs and similar public data.
                    && path.file_name().is_some_and(|n| n != "call_settings.rs")
                    && path.file_name().is_some_and(|n| n != "call.rs")
                    && path.file_name().is_some_and(|n| n != "types.rs")
                {
                    let code = std::fs::read_to_string(&path).unwrap();
                    if code.contains("serde_json::to_") || code.contains("Serialize") {
                        offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "serializers outside session.rs: {offenders:?}"
        );
    }

    #[test]
    fn json_round_trip_matrix() {
        let rec = matrix_record();
        let back = SessionRecord::from_bytes(&rec.to_bytes().unwrap()).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(back.user_id, rec.user_id);
        assert_eq!(back.homeserver_url, rec.homeserver_url);
        assert_eq!(back.session_id, rec.session_id);
        assert_eq!(back.store_passphrase.as_str(), PASS);
        match back.auth {
            SessionAuth::Matrix(s) => {
                assert_eq!(s.tokens.access_token, ACCESS);
                assert_eq!(s.tokens.refresh_token.as_deref(), Some(REFRESH));
                assert_eq!(s.meta.device_id, "DEVICEID");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn oauth_record_is_selected_for_the_oauth_restore_path() {
        use matrix_sdk::AuthSession;
        let rec = oauth_record();
        let SessionAuth::OAuth { client_id, .. } = &rec.auth else {
            panic!("not an oauth record")
        };
        match rec.auth.to_auth_session() {
            AuthSession::OAuth(s) => {
                assert_eq!(s.client_id.as_str(), client_id);
                assert_eq!(s.user.meta.user_id.as_str(), rec.user_id);
            }
            _ => panic!("wrong restore path"),
        }
        assert!(matches!(
            matrix_record().auth.to_auth_session(),
            AuthSession::Matrix(_)
        ));
    }

    #[test]
    fn json_round_trip_oauth() {
        let rec = oauth_record();
        let back = SessionRecord::from_bytes(&rec.to_bytes().unwrap()).unwrap();
        match back.auth {
            SessionAuth::OAuth { client_id, user } => {
                assert_eq!(client_id, "sentinel-client-id-x");
                assert_eq!(user.tokens.access_token, ACCESS);
                assert_eq!(user.meta.user_id, "@alice:example.org");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn from_bytes_rejects_garbage_and_unknown_version() {
        assert!(SessionRecord::from_bytes(b"not json").is_err());
        let mut rec = matrix_record();
        rec.version = 2;
        let bytes = serde_json::to_vec(&rec).unwrap();
        assert!(SessionRecord::from_bytes(&bytes).is_err());
    }

    fn rejected(rec: &SessionRecord) {
        let bytes = serde_json::to_vec(rec).unwrap();
        let err = SessionRecord::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, CoreError::Storage(_)), "{err:?}");
        let msg = err.to_string();
        assert!(!msg.contains(PASS) && !msg.contains(ACCESS), "{msg}");
    }

    #[test]
    fn from_bytes_validates_session_id() {
        for bad in ["", "../x", "a/b", "a\\b", "CON", "x.", "C:x", "a b"] {
            let mut rec = matrix_record();
            rec.session_id = bad.into();
            rejected(&rec);
        }
    }

    #[test]
    fn from_bytes_validates_homeserver_url() {
        for bad in [
            "",
            "not a url",
            "http://matrix.example.org/",
            "ftp://localhost/",
            "file:///etc/passwd",
            "https://",
            "http://localhost.evil.example/",
            "http://10.0.0.1:8008/",
        ] {
            let mut rec = matrix_record();
            rec.homeserver_url = bad.into();
            rejected(&rec);
        }
        for good in [
            "https://matrix.example.org/",
            "http://localhost:8008/",
            "http://127.0.0.1:8008/",
            "http://[::1]:8008/",
        ] {
            let mut rec = matrix_record();
            rec.homeserver_url = good.into();
            let bytes = serde_json::to_vec(&rec).unwrap();
            assert!(SessionRecord::from_bytes(&bytes).is_ok(), "{good}");
        }
    }

    #[test]
    fn from_bytes_requires_matching_user_id() {
        for mut rec in [matrix_record(), oauth_record()] {
            rec.user_id = "@mallory:example.org".into();
            rejected(&rec);
        }
    }

    #[test]
    fn error_messages_do_not_contain_secrets() {
        let err =
            SessionRecord::from_bytes(format!("{{\"x\":\"{ACCESS}\"").as_bytes()).unwrap_err();
        assert!(!err.to_string().contains(ACCESS));
    }

    #[test]
    fn realistic_records_fit_the_windows_blob_limit() {
        // Synapse-style: short opaque tokens, a refresh token.
        let synapse_access = format!("syt_{}", "a".repeat(56));
        let synapse_refresh = format!("syr_{}", "b".repeat(56));
        let mut synapse = matrix_record();
        synapse.store_passphrase = generate_store_passphrase();
        synapse.auth = SessionAuth::Matrix(MatrixSession {
            meta: meta(),
            tokens: tokens(&synapse_access, Some(&synapse_refresh)),
        });
        // MAS-style: compact tokens plus a client ID, on a long homeserver URL.
        let mut mas = oauth_record();
        mas.homeserver_url = "https://matrix-client.matrix.example.org/".into();
        mas.store_passphrase = generate_store_passphrase();
        mas.auth = SessionAuth::OAuth {
            client_id: "0".repeat(26),
            user: UserSession {
                meta: meta(),
                tokens: tokens(
                    &format!("mat_{}", "c".repeat(40)),
                    Some(&format!("mar_{}", "d".repeat(40))),
                ),
            },
        };
        for rec in [synapse, mas] {
            let len = rec.to_bytes().unwrap().len();
            assert!(len < MAX_RECORD_BYTES, "{len} bytes");
        }
    }

    #[test]
    fn oversize_record_is_a_core_error_not_a_blob() {
        let mut rec = matrix_record();
        rec.auth = SessionAuth::Matrix(MatrixSession {
            meta: meta(),
            tokens: tokens(&"x".repeat(MAX_RECORD_BYTES), None),
        });
        let err = rec.to_bytes().unwrap_err();
        assert!(matches!(err, CoreError::Storage(_)), "{err:?}");
        assert!(!err.to_string().contains(&"x".repeat(50)));
    }

    #[test]
    fn session_ids_are_valid_and_unique() {
        let a = generate_session_id();
        assert_eq!(a.len(), 32);
        assert!(validate_session_id(&a).is_ok());
        assert_ne!(a, generate_session_id());
    }

    #[test]
    fn passphrase_is_64_hex_chars_and_unique() {
        let a = generate_store_passphrase();
        let b = generate_store_passphrase();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
