//! The local test stack's settings, as `scripts/stack-env.sh` reads them:
//! `.env.test` (the committed defaults), overridden by `.env.local` (this
//! machine's, not committed), overridden by the environment. The URLs the
//! clients use are derived from the address and ports unless set. Shared by
//! twicecat-core's and twicecat-app's tests (`#[path]`).

use std::collections::HashMap;
use std::path::Path;

/// The derived URLs: key, scheme, and the port setting they use.
const URLS: [(&str, &str, &str); 5] = [
    ("HOMESERVER_URL", "http", "TWICECAT_SYNAPSE_PORT"),
    ("LK_JWT_URL", "http", "TWICECAT_LK_JWT_PORT"),
    ("LIVEKIT_URL", "ws", "TWICECAT_LIVEKIT_PORT"),
    ("LK_JWT_URL_B", "http", "TWICECAT_LK_JWT_PORT_B"),
    ("LIVEKIT_URL_B", "ws", "TWICECAT_LIVEKIT_PORT_B"),
];

/// Loads the settings from the workspace root.
pub fn load() -> HashMap<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let defaults = std::fs::read_to_string(root.join(".env.test"))
        .expect("read .env.test at the workspace root");
    let local = std::fs::read_to_string(root.join(".env.local")).ok();
    resolve(&defaults, local.as_deref(), |key| std::env::var(key).ok())
}

/// `defaults`, then `local`, then `env` per key; then the derived URLs.
pub fn resolve(
    defaults: &str,
    local: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> HashMap<String, String> {
    let mut values = parse_env(defaults);
    values.extend(local.map(parse_env).unwrap_or_default());
    let keys: Vec<String> = values
        .keys()
        .cloned()
        .chain(URLS.iter().map(|(key, ..)| (*key).to_owned()))
        .collect();
    for key in keys {
        if let Some(value) = env(&key) {
            values.insert(key, value);
        }
    }
    let address = values.get("TWICECAT_ADDRESS").cloned().unwrap_or_default();
    // An IPv6 address goes in brackets.
    let host = if address.contains(':') {
        format!("[{address}]")
    } else {
        address
    };
    for (key, scheme, port) in URLS {
        if !values.contains_key(key)
            && let Some(port) = values.get(port)
        {
            let url = format!("{scheme}://{host}:{port}");
            values.insert(key.to_owned(), url);
        }
    }
    values
}

/// Parses `KEY=VALUE` lines; `#` comments and blank lines are skipped.
pub fn parse_env(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}
