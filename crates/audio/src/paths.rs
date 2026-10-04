//! Where the fetch scripts (`scripts/fetch-*.sh`, `scripts/export-*.sh`) put
//! models and runtimes: under `$XDG_CACHE_HOME/tessera`, else
//! `~/.cache/tessera`. Each location also has its own variable, which wins.

use std::path::PathBuf;

/// `$XDG_CACHE_HOME/tessera`, else `~/.cache/tessera`.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .map(|cache| cache.join("tessera"))
}

/// The path in `variable` when set, else `relative` under [`cache_dir`].
pub(crate) fn from_env_or_cache(variable: &str, relative: &str) -> Option<PathBuf> {
    match std::env::var_os(variable) {
        Some(path) => Some(PathBuf::from(path)),
        None => Some(cache_dir()?.join(relative)),
    }
}
