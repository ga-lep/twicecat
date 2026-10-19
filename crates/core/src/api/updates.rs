//! Update checks (Phase 7 plan Task 9, Q7): which mechanism this copy of
//! Twicecat updates through, and when a check is due.

#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::time::Duration;

/// How this copy of Twicecat gets new versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMechanism {
    /// Windows, macOS, an unpacked `.tar.zst`, a build from source, and the
    /// Linux packages nothing updates (a Flatpak bundle with no repository,
    /// a `.deb` or `.rpm` installed from a downloaded file): an opt-in
    /// check, once a day, that shows a link to the release page. Nothing is
    /// downloaded or run.
    CheckAndLink,
    /// An AppImage (`APPIMAGE` is set). Checked like `CheckAndLink` until it
    /// can update itself.
    AppImage,
    /// A Flatpak installed from a repository: Flatpak updates it.
    Flatpak,
    /// Installed under `/usr` with Twicecat's apt or dnf repository set up,
    /// or from the AUR: the package manager updates it.
    PackageManager,
}

/// Whether this Flatpak comes from a repository that Flatpak updates it
/// from: `TWICECAT_FLATPAK_REMOTE=1` at build time (`value`), set by the
/// manifest once the bundle carries `--repo-url` (plan Q15). The sandbox
/// cannot see its origin (`/.flatpak-info` has none), so the build says it.
#[cfg(any(target_os = "linux", test))]
fn flatpak_remote(value: Option<&str>) -> bool {
    value == Some("1")
}

/// What the Linux rules read about the running copy.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Install<'a> {
    /// `/.flatpak-info` exists: this is a Flatpak.
    pub flatpak: bool,
    /// The Flatpak was built to come from a repository
    /// (`TWICECAT_FLATPAK_REMOTE`).
    pub flatpak_remote: bool,
    /// `APPIMAGE` is set.
    pub appimage: bool,
    /// The running binary.
    pub exe: &'a Path,
    /// A package manager can update Twicecat: its apt or dnf repository
    /// entry exists, or pacman installed it ([`package_repository`]).
    pub repository: bool,
}

impl UpdateMechanism {
    /// The mechanism of the running copy.
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            let exe = std::env::current_exe().unwrap_or_default();
            Self::detect(Install {
                flatpak: Path::new("/.flatpak-info").exists(),
                flatpak_remote: flatpak_remote(option_env!("TWICECAT_FLATPAK_REMOTE")),
                appimage: std::env::var_os("APPIMAGE").is_some(),
                exe: &exe,
                repository: package_repository(Path::new("/")),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::CheckAndLink
        }
    }

    /// The Linux rules: a Flatpak from a repository, then an AppImage (its
    /// files are mounted under `/tmp`), then a binary under `/usr` but not
    /// `/usr/local` (where copies built by hand go) that a package manager
    /// can update; anything else checks, including a Flatpak bundle and a
    /// package installed from a downloaded file, which nothing updates.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn detect(install: Install<'_>) -> Self {
        let packaged = install.exe.starts_with("/usr") && !install.exe.starts_with("/usr/local");
        if install.flatpak {
            if install.flatpak_remote {
                Self::Flatpak
            } else {
                Self::CheckAndLink
            }
        } else if install.appimage {
            Self::AppImage
        } else if packaged && install.repository {
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

/// Whether a package manager updates Twicecat on the system under `root`:
/// the apt or dnf repository entry the install instructions (plan Task 12)
/// and the packages themselves (Q15) put in place, or pacman's record of the
/// AUR package `twicecat-bin`, which AUR helpers update.
#[cfg(any(target_os = "linux", test))]
fn package_repository(root: &Path) -> bool {
    let entries = [
        "etc/apt/sources.list.d/twicecat.sources",
        "etc/yum.repos.d/twicecat.repo",
    ];
    entries.iter().any(|entry| root.join(entry).is_file())
        || std::fs::read_dir(root.join("var/lib/pacman/local")).is_ok_and(|records| {
            records.flatten().any(|record| {
                record
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("twicecat-bin-"))
            })
        })
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

    /// A binary at `exe`, outside any Flatpak or AppImage, with no package
    /// repository set up.
    fn at(exe: &str) -> Install<'_> {
        Install {
            flatpak: false,
            flatpak_remote: false,
            appimage: false,
            exe: Path::new(exe),
            repository: false,
        }
    }

    #[test]
    fn each_install_gets_its_mechanism() {
        let detect = UpdateMechanism::detect;
        let flatpak = Install {
            flatpak: true,
            ..at("/app/bin/twicecat")
        };
        assert_eq!(
            detect(Install {
                flatpak_remote: true,
                ..flatpak
            }),
            UpdateMechanism::Flatpak
        );
        // The release's bundle has no repository behind it: nothing updates
        // it, so it checks.
        assert_eq!(detect(flatpak), UpdateMechanism::CheckAndLink);
        // An AppImage's binary runs from its mount under /tmp.
        assert_eq!(
            detect(Install {
                appimage: true,
                ..at("/tmp/.mount_TwicecXYZ/usr/bin/twicecat")
            }),
            UpdateMechanism::AppImage
        );
        let packaged = at("/usr/bin/twicecat");
        assert_eq!(
            detect(Install {
                repository: true,
                ..packaged
            }),
            UpdateMechanism::PackageManager
        );
        // A .deb or .rpm installed from a downloaded file: no repository.
        assert_eq!(detect(packaged), UpdateMechanism::CheckAndLink);
        assert_eq!(
            detect(at("/home/me/twicecat-0.1.0-linux-x86_64/bin/twicecat")),
            UpdateMechanism::CheckAndLink
        );
        // Built from source and installed by hand: no package manager, even
        // with the repository set up for another copy.
        assert_eq!(
            detect(Install {
                repository: true,
                ..at("/usr/local/bin/twicecat")
            }),
            UpdateMechanism::CheckAndLink
        );
        // Paths compare by component.
        assert_eq!(
            detect(Install {
                repository: true,
                ..at("/usrx/bin/twicecat")
            }),
            UpdateMechanism::CheckAndLink
        );
        assert!(UpdateMechanism::CheckAndLink.checks_itself());
        assert!(UpdateMechanism::AppImage.checks_itself());
        assert!(!UpdateMechanism::Flatpak.checks_itself());
        assert!(!UpdateMechanism::PackageManager.checks_itself());
    }

    #[test]
    fn only_the_value_1_marks_a_flatpak_from_a_repository() {
        assert!(flatpak_remote(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 1")] {
            assert!(!flatpak_remote(value), "{value:?}");
        }
    }

    #[test]
    fn a_package_manager_updates_only_with_a_repository_or_from_the_aur() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        assert!(!package_repository(root));
        let put = |path: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
        };
        let clear = || {
            for dir in ["etc", "var"] {
                let _ = std::fs::remove_dir_all(root.join(dir));
            }
        };
        for entry in [
            "etc/apt/sources.list.d/twicecat.sources",
            "etc/yum.repos.d/twicecat.repo",
            "var/lib/pacman/local/twicecat-bin-0.1.0-1/desc",
        ] {
            put(entry);
            assert!(package_repository(root), "{entry}");
            clear();
        }
        // Someone else's entries, or a directory where the file should be.
        put("etc/apt/sources.list.d/other.sources");
        put("etc/yum.repos.d/twicecat.repo.rpmsave");
        put("var/lib/pacman/local/twicecat-git-r1-1/desc");
        std::fs::create_dir_all(root.join("etc/apt/sources.list.d/twicecat.sources")).unwrap();
        assert!(!package_repository(root));
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
