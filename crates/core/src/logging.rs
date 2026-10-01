//! Tracing setup for front ends.
//!
//! Nothing in the core logs tokens, passwords, passphrases, or message bodies.
//! The SDK's own `debug` and `trace` levels can log request details, so keep
//! them off outside of local debugging.

use thiserror::Error;
use tracing_subscriber::EnvFilter;

/// Default filter: warnings from everything, info from the core.
pub const DEFAULT_FILTER: &str = "warn,tessera_core=info";

/// Why [`init`] failed.
#[derive(Debug, Error)]
pub enum LogInitError {
    /// `default_filter` is not a valid filter directive list.
    #[error("invalid default log filter")]
    InvalidDefaultFilter,
    /// A global tracing subscriber is already installed.
    #[error("a tracing subscriber is already installed")]
    AlreadyInstalled,
}

/// Picks the filter: `env` (the value of `RUST_LOG`) if it is set, non-empty
/// and valid, otherwise `default_filter`.
fn build_filter(default_filter: &str, env: Option<&str>) -> Result<EnvFilter, LogInitError> {
    let default =
        EnvFilter::try_new(default_filter).map_err(|_| LogInitError::InvalidDefaultFilter)?;
    // An unset, empty or invalid `RUST_LOG` is ignored rather than fatal.
    match env
        .filter(|value| !value.is_empty())
        .map(EnvFilter::try_new)
    {
        Some(Ok(from_env)) => Ok(from_env),
        _ => Ok(default),
    }
}

/// Installs a `fmt` subscriber on stderr. `RUST_LOG` overrides
/// `default_filter` (use [`DEFAULT_FILTER`]).
///
/// # Errors
/// Fails if `default_filter` is invalid or a global subscriber exists.
pub fn init(default_filter: &str) -> Result<(), LogInitError> {
    let env = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    let filter = build_filter(default_filter, env.as_deref())?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|_| LogInitError::AlreadyInstalled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_is_used_without_env() {
        let f = build_filter(DEFAULT_FILTER, None).unwrap();
        let text = f.to_string();
        assert!(text.contains("tessera_core=info"), "{text}");
        assert!(text.contains("warn"), "{text}");
    }

    #[test]
    fn env_overrides_the_default() {
        let f = build_filter(DEFAULT_FILTER, Some("debug")).unwrap();
        assert_eq!(f.to_string(), "debug");
    }

    #[test]
    fn empty_or_invalid_env_falls_back_to_the_default() {
        for env in ["", "tessera_core=nonsense[["] {
            let f = build_filter(DEFAULT_FILTER, Some(env)).unwrap();
            assert!(f.to_string().contains("tessera_core=info"), "{env}");
        }
    }

    #[test]
    fn invalid_default_is_an_error() {
        assert!(matches!(
            build_filter("tessera_core=nonsense[[", None),
            Err(LogInitError::InvalidDefaultFilter)
        ));
    }
}
