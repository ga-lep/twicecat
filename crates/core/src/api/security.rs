//! Types of the Phase 4 encryption API: recovery, cross-signing, verification,
//! devices and trust. None of them names an SDK type.

use std::fmt;

use zeroize::Zeroizing;

use super::{DeviceId, UserId};

/// A recovery key (or recovery passphrase) as typed or shown to the user.
///
/// `Debug` never prints it, each clone is zeroized separately on drop, and
/// two keys compare by value.
///
/// ```
/// use twicecat_core::api::RecoveryKey;
/// let k = RecoveryKey::from("EsTc secret");
/// assert_eq!(format!("{k:?}"), "RecoveryKey(<redacted>)");
/// assert_eq!(k.expose(), "EsTc secret");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryKey(Zeroizing<String>);

impl RecoveryKey {
    /// Returns the secret. Call only where the key is actually needed.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The zeroizing wrapper itself; lets tests check the wrapper type.
    #[cfg(test)]
    pub(crate) fn zeroizing(&self) -> &Zeroizing<String> {
        &self.0
    }
}

impl From<String> for RecoveryKey {
    fn from(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
}

impl From<&str> for RecoveryKey {
    fn from(value: &str) -> Self {
        Self(Zeroizing::new(value.to_owned()))
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey(<redacted>)")
    }
}

/// State of the account's recovery (key backup plus secret storage).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecoveryStatus {
    /// Not known yet.
    #[default]
    Unknown,
    /// Set up, and this device has the secrets.
    Enabled,
    /// Not set up.
    Disabled,
    /// Set up, but this device lacks the secrets.
    Incomplete,
}

/// Progress of setting recovery up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryProgress {
    /// Started.
    Starting,
    /// Creating the key backup.
    CreatingBackup,
    /// Creating the recovery key.
    CreatingKey,
    /// Uploading room keys to the backup.
    BackingUp {
        /// Keys uploaded so far.
        done: u64,
        /// Keys to upload in all.
        total: u64,
    },
    /// An upload failed.
    UploadFailed,
}

/// Whether this device is verified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceVerification {
    /// Not known yet.
    #[default]
    Unknown,
    /// Signed by the account's identity.
    Verified,
    /// Not signed by the account's identity.
    Unverified,
}

/// State of the account's cross-signing keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrossSigning {
    /// Not known yet.
    #[default]
    Unknown,
    /// The account has no cross-signing identity.
    NotSetUp,
    /// The identity exists and this device holds its private keys.
    Complete,
    /// The identity exists but this device lacks its private keys.
    MissingKeys,
}

/// The security state of the logged-in account on this device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecurityStatus {
    /// Recovery state.
    pub recovery: RecoveryStatus,
    /// Whether this device is verified.
    pub device: DeviceVerification,
    /// Cross-signing state.
    pub cross_signing: CrossSigning,
    /// Another device of this account could verify this one.
    pub has_devices_to_verify_against: bool,
    /// The account has a cross-signing identity.
    pub identity_exists: bool,
}

/// Identifies one verification flow. Not secret.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VerificationId(String);

impl VerificationId {
    /// Borrows the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for VerificationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for VerificationId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for VerificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who a verification is with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationPeer {
    /// The user.
    pub user_id: UserId,
    /// The device, when the request came from one.
    pub device_id: Option<DeviceId>,
    /// The device's display name, when known. User-chosen text.
    pub device_name: Option<String>,
    /// The peer is another device of the logged-in account.
    pub own_account: bool,
}

/// One of the seven emoji of a short-authentication-string comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SasEmoji {
    /// The emoji.
    pub symbol: String,
    /// Its English name.
    pub description: String,
}

/// The module grid of a verification QR code, without the quiet zone.
///
/// The grid encodes the flow's one-time shared secret, so `Debug` prints the
/// size only and the grid must never be logged.
#[derive(Clone, PartialEq, Eq)]
pub struct QrImage {
    /// Modules per side.
    pub size: u16,
    /// Row-major modules, `true` for dark; `size * size` of them.
    pub dark: Vec<bool>,
}

impl fmt::Debug for QrImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QrImage")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Where a verification flow stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationPhase {
    /// A request exists.
    Requested {
        /// The other side asked.
        incoming: bool,
    },
    /// Both sides agreed to verify.
    Ready,
    /// Waiting for the other side.
    Waiting,
    /// Show this QR code for the other device to scan.
    ShowQr(QrImage),
    /// The other device scanned the code; the user must confirm.
    QrScanned,
    /// Compare these seven emoji. Boxed to keep `VerificationPhase` and
    /// `CoreEvent` small (clippy `large_enum_variant`).
    Emojis(Box<[SasEmoji; 7]>),
    /// Compare these three numbers (when emoji are unsupported).
    Decimals([u16; 3]),
    /// The user confirmed; waiting for the other side.
    Confirmed,
    /// Verified.
    Done,
    /// The flow ended without success.
    Cancelled {
        /// Why.
        reason: CancelReason,
    },
}

/// Why a verification ended without success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// The user cancelled.
    ByUs,
    /// The other side cancelled.
    ByThem,
    /// The codes did not match.
    Mismatch,
    /// The flow timed out.
    Timeout,
    /// Another device accepted the request.
    AcceptedElsewhere,
    /// Anything else.
    Other,
}

/// Trust in one device of the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceTrust {
    /// Verified.
    Verified,
    /// Not verified.
    Unverified,
    /// The device has no encryption keys.
    NoKeys,
}

/// One device of the logged-in account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSummary {
    /// Device ID.
    pub device_id: DeviceId,
    /// Display name, when set. User-chosen text.
    pub display_name: Option<String>,
    /// Last activity, milliseconds since the Unix epoch, when known.
    pub last_seen_ms: Option<u64>,
    /// This is the device the core runs on.
    pub current: bool,
    /// Trust in the device.
    pub trust: DeviceTrust,
}

/// Trust in another user's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserTrust {
    /// Verified by the logged-in user.
    Verified,
    /// Not verified.
    Unverified,
    /// The user has no cross-signing identity.
    NoIdentity,
    /// A verified user's identity changed.
    IdentityChanged,
}

/// Why a message's authenticity is in doubt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShieldCode {
    /// Cannot be guaranteed on this device.
    AuthenticityNotGuaranteed,
    /// Sent from an unknown or deleted device.
    UnknownDevice,
    /// Sent from a device its owner has not verified.
    UnsignedDevice,
    /// The sender's identity is not verified.
    UnverifiedIdentity,
    /// The sender's verified identity was reset.
    VerificationViolation,
    /// The key's owner is not the event's sender.
    MismatchedSender,
    /// Sent unencrypted in an encrypted room.
    SentInClear,
}

/// The authenticity shield of an encrypted message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shield {
    /// A warning.
    Red(ShieldCode),
    /// A caution.
    Grey(ShieldCode),
}

/// Why a message could not be decrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtdReason {
    /// No known cause (also used for non-Megolm messages).
    Unknown,
    /// Sent before this device's user joined the room.
    SentBeforeWeJoined,
    /// The sender's verified identity was reset.
    VerificationViolation,
    /// The sending device is not signed by its owner.
    UnsignedDevice,
    /// The sending device is unknown.
    UnknownDevice,
    /// Historical message and the key backup is disabled.
    HistoricalMessageAndBackupIsDisabled,
    /// The key was withheld because this device is unverified or insecure.
    WithheldForUnverifiedOrInsecureDevice,
    /// The sender withheld the key.
    WithheldBySender,
    /// Historical message and this device is unverified.
    HistoricalMessageAndDeviceIsUnverified,
}

/// An action that needs the account password (or an OAuth approval).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReauthAction {
    /// Sign out other devices.
    DeleteDevices,
    /// Reset the cross-signing identity.
    ResetIdentity,
    /// Create the cross-signing identity.
    BootstrapCrossSigning,
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use crate::api::{
        ActionKind, Command, CoreError, CoreEvent, DeviceId, MessageDetails, Password, QrImage,
        RecoveryKey, RoomId, RoomSummary, TimelineItemKind, UserId, VerificationId,
        VerificationPhase,
    };

    #[test]
    fn recovery_key_debug_is_redacted_and_compares_by_value() {
        let key = RecoveryKey::from("secret-key-x");
        assert_eq!(format!("{key:?}"), "RecoveryKey(<redacted>)");
        assert_eq!(key.expose(), "secret-key-x");
        assert_eq!(key, RecoveryKey::from(String::from("secret-key-x")));
        assert_ne!(key, RecoveryKey::from("other"));
    }

    #[test]
    fn recovery_key_clones_are_separately_zeroizing() {
        let key = RecoveryKey::from("secret-key-x");
        let copy = key.clone();
        let wrapper: &Zeroizing<String> = key.zeroizing();
        assert_eq!(wrapper.as_str(), "secret-key-x");
        let wrapper: &Zeroizing<String> = copy.zeroizing();
        assert_eq!(wrapper.as_str(), "secret-key-x");
        drop(key);
        drop(copy);
    }

    #[test]
    fn commands_and_events_hide_secrets() {
        let dbgs = [
            format!(
                "{:?}",
                Command::Recover {
                    key: RecoveryKey::from("secret-key-x")
                }
            ),
            format!(
                "{:?}",
                CoreEvent::RecoveryKeyReady(RecoveryKey::from("secret-key-x"))
            ),
            format!(
                "{:?}",
                Command::SignOutDevices {
                    device_ids: vec![DeviceId::from("DEV")],
                    password: Some(Password::from("secret-pw-x")),
                }
            ),
            format!(
                "{:?}",
                Command::ResetIdentity {
                    password: Some(Password::from("secret-pw-x"))
                }
            ),
            format!(
                "{:?}",
                Command::BootstrapCrossSigning {
                    password: Some(Password::from("secret-pw-x"))
                }
            ),
        ];
        for dbg in dbgs {
            assert!(!dbg.contains("secret-key-x"), "{dbg}");
            assert!(!dbg.contains("secret-pw-x"), "{dbg}");
        }
        let dbg = format!(
            "{:?}",
            Command::SignOutDevices {
                device_ids: vec![DeviceId::from("DEV")],
                password: None,
            }
        );
        assert!(dbg.contains("DEV"), "{dbg}");
    }

    #[test]
    fn qr_image_debug_prints_the_size_only() {
        let dark: Vec<bool> = (0..45 * 45).map(|i| i % 3 == 0).collect();
        let img = QrImage { size: 45, dark };
        assert_eq!(format!("{img:?}"), "QrImage { size: 45, .. }");
        let ev = CoreEvent::Verification {
            id: VerificationId::from("flow"),
            phase: VerificationPhase::ShowQr(img.clone()),
        };
        let dbg = format!("{ev:?}");
        assert!(!dbg.contains("true") && !dbg.contains("false"), "{dbg}");
        assert_eq!(img, img.clone());
    }

    #[test]
    fn existing_types_gain_defaults() {
        let r = RoomSummary::new(RoomId::from("!r:x"), "n".into());
        assert!(!r.is_encrypted);
        assert!(MessageDetails::default().shield.is_none());
        let p = TimelineItemKind::Placeholder {
            sender: UserId::from("@a:x"),
            sender_name: None,
            is_own: false,
            timestamp_ms: 1,
            what: crate::api::PlaceholderKind::UnableToDecrypt,
            utd: Some(crate::api::UtdReason::Unknown),
        };
        assert_eq!(p.clone(), p);
        assert_ne!(CoreError::ReauthFailed, CoreError::Busy);
        let _ = [
            ActionKind::Recovery,
            ActionKind::CrossSigning,
            ActionKind::Verification,
            ActionKind::Devices,
            ActionKind::SignOutDevices,
        ];
    }
}
