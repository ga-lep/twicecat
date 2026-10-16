//! The AppImage updates itself (Phase 7 plan Task 9b, Q7 b) through
//! cargo-packager-updater =0.2.3. It reads the signed manifest of the latest
//! published release (`latest.json`, written by the release workflow), then
//! downloads the new AppImage, verifies its minisign signature against the
//! public key built in, and swaps it in for the running one. It runs only:
//! - in an AppImage (`APPIMAGE` is set),
//! - in a build with the public key (`TWICECAT_UPDATER_PUBKEY` at build
//!   time, which the release workflow sets only when it can sign),
//! - after [`prepare_tls`] found this distribution's certificates.
//!
//! cargo-packager-updater sets `SSL_CERT_FILE` and `SSL_CERT_DIR` itself on
//! every check when they are unset (`src/lib.rs:510-519`): a data race in a
//! process with threads, and on Fedora a file that does not exist. So the app
//! sets both, to files that exist here, before any thread starts; without
//! them the updater never runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cargo_packager_updater::semver::Version;
use cargo_packager_updater::{Config, Update, UpdaterBuilder};

/// The minisign public key (base64, as `cargo packager signer generate`
/// writes it), or `None` in a build that cannot update itself.
const PUBKEY: Option<&str> = option_env!("TWICECAT_UPDATER_PUBKEY");

/// The latest published release's manifest. Drafts are not "latest".
const MANIFEST: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/releases/latest/download/latest.json"
);

const TIMEOUT: Duration = Duration::from_secs(120);

/// Certificate bundles, in the order distributions put them.
const BUNDLES: [&str; 5] = [
    // Debian, Ubuntu, Arch, SteamOS, Gentoo.
    "/etc/ssl/certs/ca-certificates.crt",
    // Fedora, RHEL.
    "/etc/pki/tls/certs/ca-bundle.crt",
    // openSUSE.
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/tls/cacert.pem",
    // Alpine, Void.
    "/etc/ssl/cert.pem",
];

/// Certificate directories, in the same spirit.
const DIRS: [&str; 2] = ["/etc/ssl/certs", "/etc/pki/tls/certs"];

/// The bundle and the directory to point the variables at: the first of each
/// that exists (`is_file`, `is_dir`), the bundle's own folder when no known
/// directory exists. `None` without a bundle.
pub(crate) fn tls_paths(
    is_file: impl Fn(&Path) -> bool,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<(PathBuf, PathBuf)> {
    let file = BUNDLES.iter().map(Path::new).find(|p| is_file(p))?;
    let dir = DIRS
        .iter()
        .map(Path::new)
        .find(|p| is_dir(p))
        .or_else(|| file.parent())?;
    Some((file.to_path_buf(), dir.to_path_buf()))
}

/// In an AppImage of a build that can update itself, sets `SSL_CERT_FILE`
/// and `SSL_CERT_DIR` (each only when unset) to this distribution's
/// certificates, so cargo-packager-updater never sets them itself.
///
/// # Safety
///
/// Call it at the very start of `main`, before any other thread exists
/// (`std::env::set_var` races with every other access to the environment).
pub unsafe fn prepare_tls() {
    if !PUBKEY.is_some_and(|key| !key.is_empty()) || std::env::var_os("APPIMAGE").is_none() {
        return;
    }
    let Some((file, dir)) = tls_paths(Path::is_file, Path::is_dir) else {
        return;
    };
    for (name, value) in [("SSL_CERT_FILE", file), ("SSL_CERT_DIR", dir)] {
        if std::env::var_os(name).is_none() {
            // SAFETY: the caller guarantees that no other thread runs yet.
            unsafe { std::env::set_var(name, value) };
        }
    }
}

/// Whether this process can update itself: see the module docs.
pub(crate) fn available() -> bool {
    PUBKEY.is_some_and(|key| !key.is_empty())
        && std::env::var_os("APPIMAGE").is_some()
        && std::env::var_os("SSL_CERT_FILE").is_some()
        && std::env::var_os("SSL_CERT_DIR").is_some()
}

/// Why the updater gave no answer (fixed texts only; the library's errors
/// can carry URLs and paths).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fail {
    Unavailable,
    Check,
    Install,
}

fn updater() -> Result<cargo_packager_updater::Updater, Fail> {
    let key = PUBKEY
        .filter(|key| !key.is_empty())
        .ok_or(Fail::Unavailable)?;
    if !available() {
        return Err(Fail::Unavailable);
    }
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| Fail::Unavailable)?;
    let config = Config {
        endpoints: vec![MANIFEST.parse().map_err(|_| Fail::Unavailable)?],
        pubkey: key.to_owned(),
        windows: None,
    };
    UpdaterBuilder::new(version, config)
        .timeout(TIMEOUT)
        .build()
        .map_err(|_| Fail::Unavailable)
}

/// The newer release in the manifest, if any (blocking: run it on a
/// blocking thread).
pub(crate) fn check_blocking() -> Result<Option<Update>, Fail> {
    updater()?.check().map_err(|_| Fail::Check)
}

/// Checks again, then downloads, verifies and swaps in the new AppImage;
/// returns its version (blocking).
pub(crate) fn install_blocking() -> Result<String, Fail> {
    let update = check_blocking()?.ok_or(Fail::Check)?;
    update.download_and_install().map_err(|_| Fail::Install)?;
    Ok(update.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_and_folder_of_each_distribution_are_found() {
        let on = |files: &'static [&'static str], dirs: &'static [&'static str]| {
            tls_paths(
                move |p: &Path| files.iter().any(|f| Path::new(f) == p),
                move |p: &Path| dirs.iter().any(|d| Path::new(d) == p),
            )
        };
        let pair = |f: &str, d: &str| Some((PathBuf::from(f), PathBuf::from(d)));
        // Debian and its relatives.
        assert_eq!(
            on(&["/etc/ssl/certs/ca-certificates.crt"], &["/etc/ssl/certs"]),
            pair("/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/certs")
        );
        // Fedora: /etc/ssl/certs is a link to /etc/pki/tls/certs, which has
        // no ca-certificates.crt.
        assert_eq!(
            on(
                &["/etc/pki/tls/certs/ca-bundle.crt"],
                &["/etc/ssl/certs", "/etc/pki/tls/certs"]
            ),
            pair("/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/certs")
        );
        // A bundle without a known folder: the bundle's own.
        assert_eq!(
            on(&["/etc/ssl/cert.pem"], &[]),
            pair("/etc/ssl/cert.pem", "/etc/ssl")
        );
        // No bundle: nothing is set, and the updater stays off.
        assert_eq!(on(&[], &["/etc/ssl/certs"]), None);
    }

    #[test]
    fn the_manifest_is_the_latest_published_release() {
        assert_eq!(
            MANIFEST,
            "https://github.com/ga-lep/twicecat/releases/latest/download/latest.json"
        );
    }

    #[test]
    fn a_build_without_the_key_never_updates_itself() {
        if PUBKEY.is_none() {
            assert!(!available());
            assert_eq!(check_blocking().err(), Some(Fail::Unavailable));
        }
    }
}
