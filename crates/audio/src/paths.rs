//! Where models and runtimes are: in the user cache, where the fetch scripts
//! (`scripts/fetch-*.sh`, `scripts/export-*.sh`) put them, or in the package
//! that installed Twicecat, which carries the models and, on Linux, the
//! "any GPU" pack (Phase 7 plan Q3, Q10). Both use the same layout under
//! their root. Each location also has its own variable, which wins.
//!
//! The cache comes first, so a runtime the user fetched (CUDA) is used over
//! the bundled CPU build.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The user cache: `$XDG_CACHE_HOME/twicecat`, else `~/.cache/twicecat`. On
/// Windows, without `HOME` (it is set in Git Bash, where the scripts run),
/// `%USERPROFILE%\.cache\twicecat`, the same folder.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        std::env::consts::OS,
    )
}

fn cache_dir_from(
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
    user_profile: Option<OsString>,
    os: &str,
) -> Option<PathBuf> {
    let home = home.or_else(|| user_profile.filter(|_| os == "windows"));
    xdg_cache_home
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".cache")))
        .map(|cache| cache.join("twicecat"))
}

/// The package's resource directory, from where the executable is: on Linux
/// `<prefix>/bin/twicecat` gives `<prefix>/lib/twicecat` (a `.deb` or
/// `.rpm` in `/usr`, the Flatpak in `/app`, the AppImage in `$APPDIR/usr`,
/// the `.tar.zst` wherever it is unpacked). Not `<prefix>/share/twicecat`:
/// with the `.tar.zst` unpacked into `~/.local`, that is the app's data
/// directory, and the package would share it with the session store. On
/// macOS `Contents/MacOS/` gives
/// `Contents/Resources/`; on Windows, the executable's own folder. `None`
/// for a development build, whose folder has no such directory.
pub(crate) fn resource_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    resource_dir_from(&exe, std::env::consts::OS).filter(|dir| dir.is_dir())
}

fn resource_dir_from(exe: &Path, os: &str) -> Option<PathBuf> {
    let folder = exe.parent()?;
    match os {
        "windows" => Some(folder.to_path_buf()),
        "macos" => Some(folder.parent()?.join("Resources")),
        _ => Some(folder.parent()?.join("lib").join("twicecat")),
    }
}

/// The roots searched, in order: the user cache, then the package.
pub(crate) fn roots() -> Vec<PathBuf> {
    cache_dir().into_iter().chain(resource_dir()).collect()
}

/// The path in `variable` when set; else `relative` under the first root
/// where it exists; else under the cache, so that a caller that reports a
/// missing file names where the scripts would put it.
pub(crate) fn from_env_or_cache(variable: &str, relative: &str) -> Option<PathBuf> {
    locate(std::env::var_os(variable), relative, &roots(), |path| {
        path.exists()
    })
}

fn locate(
    variable: Option<OsString>,
    relative: &str,
    roots: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(path) = variable {
        return Some(PathBuf::from(path));
    }
    let candidates = roots.iter().map(|root| root.join(relative));
    candidates
        .clone()
        .find(|path| exists(path))
        .or_else(|| candidates.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn the_cache_follows_xdg_then_home_then_the_windows_profile() {
        assert_eq!(
            cache_dir_from(os("/x"), os("/h"), os("C:/u"), "linux"),
            Some(PathBuf::from("/x/twicecat"))
        );
        assert_eq!(
            cache_dir_from(None, os("/h"), None, "linux"),
            Some(PathBuf::from("/h/.cache/twicecat"))
        );
        // Git Bash sets HOME; it wins over the profile.
        assert_eq!(
            cache_dir_from(None, os("/h"), os("C:/u"), "windows"),
            Some(PathBuf::from("/h/.cache/twicecat"))
        );
        assert_eq!(
            cache_dir_from(None, None, os("C:/u"), "windows"),
            Some(PathBuf::from("C:/u/.cache/twicecat"))
        );
        // USERPROFILE means nothing elsewhere.
        assert_eq!(cache_dir_from(None, None, os("/u"), "linux"), None);
        assert_eq!(cache_dir_from(None, None, None, "windows"), None);
    }

    #[test]
    fn the_resource_dir_follows_each_package_layout() {
        let at = |exe: &str, os: &str| resource_dir_from(Path::new(exe), os);
        assert_eq!(
            at("/usr/bin/twicecat", "linux"),
            Some(PathBuf::from("/usr/lib/twicecat"))
        );
        assert_eq!(
            at("/app/bin/twicecat", "linux"),
            Some(PathBuf::from("/app/lib/twicecat"))
        );
        assert_eq!(
            at("/tmp/.mount_twXYZ/usr/bin/twicecat", "linux"),
            Some(PathBuf::from("/tmp/.mount_twXYZ/usr/lib/twicecat"))
        );
        // The .tar.zst unpacked into ~/.local: beside the data directory
        // (~/.local/share/twicecat), not inside it.
        assert_eq!(
            at("/home/u/.local/bin/twicecat", "linux"),
            Some(PathBuf::from("/home/u/.local/lib/twicecat"))
        );
        assert_eq!(
            at(
                "/Applications/Twicecat.app/Contents/MacOS/twicecat",
                "macos"
            ),
            Some(PathBuf::from(
                "/Applications/Twicecat.app/Contents/Resources"
            ))
        );
        assert_eq!(
            at("C:/Program Files/Twicecat/twicecat.exe", "windows"),
            Some(PathBuf::from("C:/Program Files/Twicecat"))
        );
        // A bare file name has no folder to look beside.
        assert_eq!(at("twicecat", "linux"), None);
    }

    #[test]
    fn the_variable_wins_then_the_first_root_that_has_it_then_the_cache() {
        let roots = [PathBuf::from("/cache"), PathBuf::from("/pkg")];
        let only = |present: &'static str| move |path: &Path| path == Path::new(present);

        assert_eq!(
            locate(os("/mine"), "models/m", &roots, only("/pkg/models/m")),
            Some(PathBuf::from("/mine"))
        );
        assert_eq!(
            locate(None, "models/m", &roots, |_| true),
            Some(PathBuf::from("/cache/models/m"))
        );
        assert_eq!(
            locate(None, "models/m", &roots, only("/pkg/models/m")),
            Some(PathBuf::from("/pkg/models/m"))
        );
        assert_eq!(
            locate(None, "models/m", &roots, |_| false),
            Some(PathBuf::from("/cache/models/m"))
        );
        assert_eq!(locate(None, "models/m", &[], |_| true), None);
    }
}
