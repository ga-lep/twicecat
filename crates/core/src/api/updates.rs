//! Update checks (Phase 7 plan Task 9, Q7): which mechanism this copy of
//! Twicecat updates through, and when a check is due.

use std::path::Path;
use std::time::Duration;

/// How this copy of Twicecat gets new versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMechanism {
    /// Windows, macOS, an unpacked `.tar.zst` or a build from source: an
    /// opt-in check, once a day, that shows a link to the release page.
    /// Nothing is downloaded or run.
    CheckAndLink,
    /// An AppImage (`APPIMAGE` is set). Checked like `CheckAndLink` until it
    /// can update itself.
    AppImage,
    /// The Flatpak: Flatpak updates it.
    Flatpak,
    /// Installed under `/usr` (`.deb`, `.rpm`, AUR): the package manager
    /// updates it.
    PackageManager,
}

impl UpdateMechanism {
    /// The mechanism of the running copy.
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            let exe = std::env::current_exe().unwrap_or_default();
            Self::detect(
                Path::new("/.flatpak-info").exists(),
                std::env::var_os("APPIMAGE").is_some(),
                &exe,
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::CheckAndLink
        }
    }

    /// The Linux rules: Flatpak, then AppImage (its files are mounted under
    /// `/tmp`), then a binary under `/usr` but not `/usr/local` (where
    /// copies built by hand go), else a check.
    pub fn detect(flatpak_info: bool, appimage: bool, exe: &Path) -> Self {
        if flatpak_info {
            Self::Flatpak
        } else if appimage {
            Self::AppImage
        } else if exe.starts_with("/usr") && !exe.starts_with("/usr/local") {
            Self::PackageManager
        } else {
            Self::CheckAndLink
        }
    }

    /// Whether Twicecat checks for updates itself (when the user lets it).
    pub fn checks_itself(self) -> bool {
        matches!(self, Self::CheckAndLink | Self::AppImage)
    }
}

/// How often a check is due.
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether a check is due at `now` (seconds since the Unix epoch) after one
/// at `last` (0: never). A clock set back by more than an hour also counts,
/// so a wrong date never stops the checks for long.
///
/// ```
/// use twicecat_core::api::update_check_due;
/// assert!(update_check_due(0, 1_000_000));
/// assert!(!update_check_due(1_000_000, 1_000_000 + 3_600));
/// assert!(update_check_due(1_000_000, 1_000_000 + 86_400));
/// ```
pub fn update_check_due(last: u64, now: u64) -> bool {
    if last == 0 {
        return true;
    }
    match now.checked_sub(last) {
        Some(elapsed) => elapsed >= UPDATE_CHECK_INTERVAL.as_secs(),
        None => last - now > 3_600,
    }
}

/// Whether `latest` (a release tag such as `v1.2.0`) is newer than
/// `current` (`CARGO_PKG_VERSION`). Both are `MAJOR.MINOR.PATCH`, with an
/// optional `v` and pre-release suffix; a pre-release is older than its
/// release. Anything else is never newer.
///
/// ```
/// use twicecat_core::api::is_newer_version;
/// assert!(is_newer_version("v0.2.0", "0.1.9"));
/// assert!(!is_newer_version("v0.1.0", "0.1.0"));
/// assert!(is_newer_version("v1.0.0", "1.0.0-rc.1"));
/// assert!(!is_newer_version("nightly", "0.1.0"));
/// ```
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// `(major, minor, patch, is_release)`: a release sorts after its
/// pre-releases. Pre-releases of one version are not ordered among
/// themselves (they compare equal).
fn parse_version(text: &str) -> Option<(u64, u64, u64, bool)> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, true),
        Some(_) => return None,
        None => (text, false),
    };
    let mut parts = core.split('.');
    let mut number = || -> Option<u64> {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let version = (number()?, number()?, number()?, !pre);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_install_gets_its_mechanism() {
        let detect = UpdateMechanism::detect;
        assert_eq!(
            detect(true, false, Path::new("/app/bin/twicecat")),
            UpdateMechanism::Flatpak
        );
        // An AppImage's binary runs from its mount under /tmp.
        assert_eq!(
            detect(
                false,
                true,
                Path::new("/tmp/.mount_TwicecXYZ/usr/bin/twicecat")
            ),
            UpdateMechanism::AppImage
        );
        assert_eq!(
            detect(false, false, Path::new("/usr/bin/twicecat")),
            UpdateMechanism::PackageManager
        );
        assert_eq!(
            detect(
                false,
                false,
                Path::new("/home/me/twicecat-0.1.0-linux-x86_64/bin/twicecat")
            ),
            UpdateMechanism::CheckAndLink
        );
        // Built from source and installed by hand: no package manager.
        assert_eq!(
            detect(false, false, Path::new("/usr/local/bin/twicecat")),
            UpdateMechanism::CheckAndLink
        );
        // Paths compare by component.
        assert_eq!(
            detect(false, false, Path::new("/usrx/bin/twicecat")),
            UpdateMechanism::CheckAndLink
        );
        assert!(UpdateMechanism::CheckAndLink.checks_itself());
        assert!(UpdateMechanism::AppImage.checks_itself());
        assert!(!UpdateMechanism::Flatpak.checks_itself());
        assert!(!UpdateMechanism::PackageManager.checks_itself());
    }

    #[test]
    fn a_check_is_due_once_a_day() {
        let day = UPDATE_CHECK_INTERVAL.as_secs();
        let t = 1_800_000_000;
        assert!(update_check_due(0, t));
        assert!(!update_check_due(t, t));
        assert!(!update_check_due(t, t + day - 1));
        assert!(update_check_due(t, t + day));
        // The clock went back a little (a correction): wait.
        assert!(!update_check_due(t, t - 60));
        // The clock went back a lot (a wrong date was fixed): check.
        assert!(update_check_due(t, t - 7_200));
    }

    #[test]
    fn versions_compare_by_number_then_release() {
        assert!(is_newer_version("v0.1.1", "0.1.0"));
        assert!(is_newer_version("v0.10.0", "0.9.9"));
        assert!(is_newer_version("1.0.0", "0.99.99"));
        assert!(!is_newer_version("v0.1.0", "0.1.1"));
        assert!(!is_newer_version("v0.1.0", "0.1.0"));
        assert!(is_newer_version("v0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer_version("v0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer_version("v0.2.0-beta.2", "0.2.0-beta.1"));
        for bad in [
            "", "v", "1", "1.2", "1.2.3.4", "1.2.x", "1..3", "+1.2.3", "1.2.3-", "v 1.2.3",
        ] {
            assert!(!is_newer_version(bad, "0.0.1"), "{bad:?}");
            assert!(!is_newer_version("9.9.9", bad), "{bad:?}");
        }
    }
}
