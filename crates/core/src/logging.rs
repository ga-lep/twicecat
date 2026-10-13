//! Tracing setup for front ends.
//!
//! Nothing in the core logs tokens, passwords, passphrases, or message bodies.
//! The SDK's own `debug` and `trace` levels can log request details, so keep
//! them off outside of local debugging. Targets that carry call keys are off
//! whatever the filter says.

use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::Directive;

use crate::call::SECRET_LOG_TARGETS;

/// Default filter: warnings from everything, info from the core.
pub const DEFAULT_FILTER: &str = "warn,twicecat_core=info";

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

/// The filter [`init`] installs: `env` (the value of `RUST_LOG`) if it is
/// set, non-empty and valid, otherwise `default_filter`; either way with the
/// targets that carry call keys turned off, even when `env` names them.
///
/// # Errors
/// Fails if `default_filter` is invalid.
pub fn filter(default_filter: &str, env: Option<&str>) -> Result<EnvFilter, LogInitError> {
    let default =
        EnvFilter::try_new(default_filter).map_err(|_| LogInitError::InvalidDefaultFilter)?;
    // An unset, empty or invalid `RUST_LOG` is ignored rather than fatal.
    let chosen = match env
        .filter(|value| !value.is_empty())
        .map(EnvFilter::try_new)
    {
        Some(Ok(from_env)) => from_env,
        _ => default,
    };
    // A directive for the same target replaces the one already there.
    Ok(SECRET_LOG_TARGETS.iter().fold(chosen, |f, target| {
        f.add_directive(
            format!("{target}=off")
                .parse::<Directive>()
                .expect("a target name is a valid directive"),
        )
    }))
}

/// Installs a `fmt` subscriber on stderr. `RUST_LOG` overrides
/// `default_filter` (use [`DEFAULT_FILTER`]).
///
/// # Errors
/// Fails if `default_filter` is invalid or a global subscriber exists.
pub fn init(default_filter: &str) -> Result<(), LogInitError> {
    let env = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    tracing_subscriber::fmt()
        .with_env_filter(filter(default_filter, env.as_deref())?)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|_| LogInitError::AlreadyInstalled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_is_used_without_env() {
        let f = filter(DEFAULT_FILTER, None).unwrap();
        let text = f.to_string();
        assert!(text.contains("twicecat_core=info"), "{text}");
        assert!(text.contains("warn"), "{text}");
    }

    #[test]
    fn env_overrides_the_default() {
        let text = filter(DEFAULT_FILTER, Some("debug")).unwrap().to_string();
        assert!(text.contains("debug"), "{text}");
        assert!(!text.contains("twicecat_core=info"), "{text}");
    }

    #[test]
    fn empty_or_invalid_env_falls_back_to_the_default() {
        for env in ["", "twicecat_core=nonsense[["] {
            let f = filter(DEFAULT_FILTER, Some(env)).unwrap();
            assert!(f.to_string().contains("twicecat_core=info"), "{env}");
        }
    }

    #[test]
    fn secret_targets_stay_off_whatever_the_filter_asks() {
        for target in SECRET_LOG_TARGETS {
            let asked = [
                None,
                Some("trace".to_owned()),
                Some(format!("{target}=trace")),
                Some(format!("warn,{target}=debug")),
            ];
            for env in asked {
                let text = filter(DEFAULT_FILTER, env.as_deref()).unwrap().to_string();
                assert!(text.contains(&format!("{target}=off")), "{env:?}: {text}");
                assert_eq!(text.matches(target).count(), 1, "{env:?}: {text}");
            }
        }
    }

    #[test]
    fn invalid_default_is_an_error() {
        assert!(matches!(
            filter("twicecat_core=nonsense[[", None),
            Err(LogInitError::InvalidDefaultFilter)
        ));
    }
}
