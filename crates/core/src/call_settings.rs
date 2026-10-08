//! `call_settings.json`: the persisted [`AudioSettings`], in a versioned
//! wrapper. No secrets live here.

use serde::{Deserialize, Serialize};

use crate::api::{AudioSettings, CoreError};
use crate::paths::DataPaths;

/// Current file format version.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    settings: AudioSettings,
}

/// The outcome of loading: the settings to use, and an error to report when
/// the file existed but could not be used.
pub(crate) struct Loaded {
    pub settings: AudioSettings,
    pub problem: Option<CoreError>,
}

/// Parses a settings file. Unknown fields are ignored and missing ones take
/// their defaults; the result is validated.
fn from_bytes(bytes: &[u8]) -> Result<AudioSettings, CoreError> {
    let file: SettingsFile = serde_json::from_slice(bytes)
        // serde's message can quote file content; it is not forwarded.
        .map_err(|_| CoreError::Storage("call settings file is not valid".into()))?;
    if file.version != VERSION {
        return Err(CoreError::Storage(
            "call settings file has an unknown version".into(),
        ));
    }
    Ok(file.settings.validated())
}

fn to_bytes(settings: &AudioSettings) -> Vec<u8> {
    let file = SettingsFile {
        version: VERSION,
        settings: settings.clone(),
    };
    // Plain data with string map keys: serialisation cannot fail.
    serde_json::to_vec_pretty(&file).expect("audio settings serialise")
}

/// Loads the settings (blocking file I/O). A missing file gives the
/// defaults; an unreadable or invalid one gives the defaults plus a problem
/// to report, and is left untouched until the next save.
pub(crate) fn load(paths: &DataPaths) -> Loaded {
    match paths.read_call_settings() {
        Ok(None) => Loaded {
            settings: AudioSettings::default(),
            problem: None,
        },
        Ok(Some(bytes)) => match from_bytes(&bytes) {
            Ok(settings) => Loaded {
                settings,
                problem: None,
            },
            Err(e) => Loaded {
                settings: AudioSettings::default(),
                problem: Some(e),
            },
        },
        Err(_) => Loaded {
            settings: AudioSettings::default(),
            problem: Some(CoreError::Storage(
                "call settings file could not be read".into(),
            )),
        },
    }
}

/// Saves the settings (blocking file I/O).
pub(crate) fn save(paths: &DataPaths, settings: &AudioSettings) -> Result<(), CoreError> {
    paths
        .write_call_settings(&to_bytes(settings))
        .map_err(|_| CoreError::Storage("call settings could not be saved".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{InputMode, UserId};

    fn paths() -> (tempfile::TempDir, DataPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        (dir, paths)
    }

    #[test]
    fn missing_file_gives_defaults_without_problem() {
        let (_dir, paths) = paths();
        let loaded = load(&paths);
        assert_eq!(loaded.settings, AudioSettings::default());
        assert!(loaded.problem.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_dir, paths) = paths();
        let mut settings = AudioSettings {
            input_mode: InputMode::PushToTalk {
                release_delay_ms: 250,
            },
            ..AudioSettings::default()
        };
        settings.user_volumes.insert(UserId::from("@bob:x"), 150);
        save(&paths, &settings).unwrap();
        let loaded = load(&paths);
        assert!(loaded.problem.is_none());
        assert_eq!(loaded.settings, settings);
    }

    #[test]
    fn corrupt_file_gives_defaults_and_a_problem_without_content() {
        let (_dir, paths) = paths();
        paths
            .write_call_settings(b"{ not json secret-looking-x")
            .unwrap();
        let loaded = load(&paths);
        assert_eq!(loaded.settings, AudioSettings::default());
        let problem = loaded.problem.expect("reported");
        assert!(!problem.to_string().contains("secret-looking-x"));
    }

    #[test]
    fn unknown_version_is_refused() {
        let (_dir, paths) = paths();
        paths
            .write_call_settings(br#"{"version": 2, "settings": {}}"#)
            .unwrap();
        assert!(load(&paths).problem.is_some());
    }

    #[test]
    fn loaded_values_are_validated() {
        let (_dir, paths) = paths();
        paths
            .write_call_settings(br#"{"version": 1, "settings": {"voice_bitrate_kbps": 9999}}"#)
            .unwrap();
        let loaded = load(&paths);
        assert!(loaded.problem.is_none());
        assert_eq!(loaded.settings.voice_bitrate_kbps, 510);
    }
}
