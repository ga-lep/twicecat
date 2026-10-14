//! `video_settings.json`: the persisted [`VideoSettings`], in a versioned
//! wrapper, as `call_settings.json` holds the audio ones. No secrets live
//! here.

use serde::{Deserialize, Serialize};

use crate::api::{CoreError, VideoSettings};
use crate::paths::DataPaths;

/// Current file format version.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    settings: VideoSettings,
}

/// The outcome of loading: the settings to use, and an error to report when
/// the file existed but could not be used.
pub(crate) struct Loaded {
    pub settings: VideoSettings,
    pub problem: Option<CoreError>,
}

/// Parses a settings file. Unknown fields are ignored and missing ones take
/// their defaults.
fn from_bytes(bytes: &[u8]) -> Result<VideoSettings, CoreError> {
    let file: SettingsFile = serde_json::from_slice(bytes)
        // serde's message can quote file content; it is not forwarded.
        .map_err(|_| CoreError::Storage("video settings file is not valid".into()))?;
    if file.version != VERSION {
        return Err(CoreError::Storage(
            "video settings file has an unknown version".into(),
        ));
    }
    Ok(file.settings)
}

fn to_bytes(settings: &VideoSettings) -> Vec<u8> {
    let file = SettingsFile {
        version: VERSION,
        settings: settings.clone(),
    };
    // Plain data: serialisation cannot fail.
    serde_json::to_vec_pretty(&file).expect("video settings serialise")
}

/// Loads the settings (blocking file I/O). A missing file gives the
/// defaults; an unreadable or invalid one gives the defaults plus a problem
/// to report, and is left untouched until the next save.
pub(crate) fn load(paths: &DataPaths) -> Loaded {
    let (settings, problem) = match paths.read_video_settings() {
        Ok(None) => (VideoSettings::default(), None),
        Ok(Some(bytes)) => match from_bytes(&bytes) {
            Ok(settings) => (settings, None),
            Err(e) => (VideoSettings::default(), Some(e)),
        },
        Err(_) => (
            VideoSettings::default(),
            Some(CoreError::Storage(
                "video settings file could not be read".into(),
            )),
        ),
    };
    Loaded { settings, problem }
}

/// Saves the settings (blocking file I/O).
pub(crate) fn save(paths: &DataPaths, settings: &VideoSettings) -> Result<(), CoreError> {
    paths
        .write_video_settings(&to_bytes(settings))
        .map_err(|_| CoreError::Storage("video settings could not be saved".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{EncoderBackend, EncoderChoice, VideoQuality};

    fn paths() -> (tempfile::TempDir, DataPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        (dir, paths)
    }

    #[test]
    fn missing_file_gives_defaults_without_problem() {
        let (_dir, paths) = paths();
        let loaded = load(&paths);
        assert_eq!(loaded.settings, VideoSettings::default());
        assert!(loaded.problem.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_dir, paths) = paths();
        let settings = VideoSettings {
            encoder: EncoderChoice::Prefer(EncoderBackend::Software),
            screen_quality: VideoQuality::Uhd2160p30,
            ..VideoSettings::default()
        };
        save(&paths, &settings).unwrap();
        let loaded = load(&paths);
        assert!(loaded.problem.is_none());
        assert_eq!(loaded.settings, settings);
    }

    #[test]
    fn corrupt_file_gives_defaults_and_a_problem_without_content() {
        let (_dir, paths) = paths();
        paths
            .write_video_settings(b"{ not json secret-looking-x")
            .unwrap();
        let loaded = load(&paths);
        assert_eq!(loaded.settings, VideoSettings::default());
        let problem = loaded.problem.expect("reported");
        assert!(!problem.to_string().contains("secret-looking-x"));
    }

    #[test]
    fn unknown_version_is_refused_and_missing_fields_default() {
        let (_dir, paths) = paths();
        paths
            .write_video_settings(br#"{"version": 2, "settings": {}}"#)
            .unwrap();
        assert!(load(&paths).problem.is_some());
        paths
            .write_video_settings(
                br#"{"version": 1, "settings": {"encoder": {"prefer": "software"}}}"#,
            )
            .unwrap();
        let loaded = load(&paths);
        assert!(loaded.problem.is_none(), "{:?}", loaded.problem);
        assert_eq!(
            loaded.settings.encoder,
            EncoderChoice::Prefer(EncoderBackend::Software)
        );
        assert_eq!(loaded.settings.camera_quality, VideoQuality::Auto);
    }
}
