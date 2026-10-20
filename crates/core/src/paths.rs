//! On-disk locations for per-session stores, the last-account marker and the
//! local settings file.

use std::path::{Path, PathBuf};

use crate::api::sidebar::is_valid_user_id;
use crate::api::sidebar::limits::MAX_SIDEBAR_BYTES;
use crate::api::{
    CustomSection, DensityChoice, FolderLayout, RailEntry, RoomId, RoomListChoice, SectionKey,
    SidebarLabel, SidebarLayout, SidebarRead, SpaceLayout, ThemeChoice, ThemeName, UiSettings,
    UpdateCheckChoice, UserId,
};

/// Largest settings file read (64 KiB); anything bigger is treated as corrupt.
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

/// Builds the settings from a parsed file, field by field: a missing field or
/// one with an unknown value takes that field's default.
fn settings_from_map(map: &serde_json::Map<String, serde_json::Value>) -> UiSettings {
    let text = |key: &str| map.get(key).and_then(serde_json::Value::as_str);
    let flag = |key: &str, default: bool| {
        map.get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    };
    let defaults = UiSettings::default();
    UiSettings {
        theme: match text("theme") {
            Some("System") => ThemeChoice::System,
            Some("Light") => ThemeChoice::Light,
            Some("Dark") => ThemeChoice::Dark,
            _ => defaults.theme,
        },
        desktop_notifications: flag("desktop_notifications", defaults.desktop_notifications),
        show_message_content: flag("show_message_content", defaults.show_message_content),
        tray_badge: flag("tray_badge", defaults.tray_badge),
        // The older `palette` key is ignored; a name that fails the rules is
        // the built-in theme.
        theme_name: text("theme_name").and_then(|name| ThemeName::try_from(name).ok()),
        density: match text("density") {
            Some("Compact") => DensityChoice::Compact,
            Some("Comfortable") => DensityChoice::Comfortable,
            _ => defaults.density,
        },
        room_list: match text("room_list") {
            Some("Rail") => RoomListChoice::Rail,
            Some("Expanded") => RoomListChoice::Expanded,
            _ => defaults.room_list,
        },
        text_scale_percent: map
            .get("text_scale_percent")
            .and_then(serde_json::Value::as_u64)
            .map_or(defaults.text_scale_percent, clamp_text_scale),
        advanced_settings: flag("advanced_settings", defaults.advanced_settings),
        update_check: match text("update_check") {
            Some("Off") => UpdateCheckChoice::Off,
            Some("On") => UpdateCheckChoice::On,
            _ => defaults.update_check,
        },
        update_checked_at: map
            .get("update_checked_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(defaults.update_checked_at),
    }
}

/// Clamps a text scale to 80-200 % and rounds it to a step of 10.
fn clamp_text_scale(raw: u64) -> u16 {
    let stepped = raw.saturating_add(5) / 10 * 10;
    u16::try_from(stepped.clamp(80, 200)).expect("clamped to at most 200")
}

/// Errors from building paths.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    /// The session id is empty or could escape the sessions directory.
    #[error("invalid session id")]
    InvalidSessionId,
    /// A directory could not be created or removed.
    #[error("could not access the data directory")]
    Io,
}

/// Checks a session id against the allowlist: 1..=128 characters of
/// `[A-Za-z0-9_-]`, not a Windows reserved device name.
pub(crate) fn validate_session_id(id: &str) -> Result<(), PathsError> {
    let chars_ok = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if chars_ok && !is_windows_reserved(id) {
        Ok(())
    } else {
        Err(PathsError::InvalidSessionId)
    }
}

fn is_windows_reserved(id: &str) -> bool {
    let upper = id.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            upper
                .strip_prefix(p)
                .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
        })
}

/// `create_dir_all` with mode 0o700 on Unix.
fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Reads a regular file of at most `cap` bytes. `None` for anything else
/// (missing, a directory, device or FIFO, too big, unreadable). A symlink is
/// followed, and is accepted only if its target is a regular file.
fn read_capped_regular_file(path: &Path, cap: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;

    // Checked before opening: opening a FIFO for reading would block until a
    // writer shows up.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= cap).then_some(bytes)
}

/// Why a sidebar file was not written or removed.
#[derive(Debug, thiserror::Error)]
pub enum SidebarWriteError {
    /// The file at the path was saved by a newer Twicecat or belongs to
    /// another account. It is left as it is.
    #[error("the sidebar file is protected")]
    Protected,
    /// The layout is invalid or over a limit (see [`SidebarLayout::fits`]).
    #[error("the sidebar layout is invalid or too big")]
    Invalid,
    /// The file could not be written or removed.
    #[error("could not write the sidebar file")]
    Io(#[source] std::io::Error),
}

/// 64-bit FNV-1a, a fixed public algorithm (unlike `std`'s `DefaultHasher`,
/// whose output may change between Rust releases).
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

/// The strings of `value` when it is an array; anything else gives none.
fn strings_of(value: Option<&serde_json::Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
}

fn room_ids_of(value: Option<&serde_json::Value>) -> Vec<RoomId> {
    strings_of(value).map(RoomId::from).collect()
}

fn section_keys_of(value: Option<&serde_json::Value>) -> Vec<SectionKey> {
    strings_of(value).filter_map(SectionKey::from_key).collect()
}

fn u16_of(value: Option<&serde_json::Value>) -> Option<u16> {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u16::try_from(n).ok())
}

fn rail_entry_from(value: &serde_json::Value) -> Option<RailEntry> {
    let map = value.as_object()?;
    if let Some(id) = map.get("space").and_then(serde_json::Value::as_str) {
        return Some(RailEntry::Space(RoomId::from(id)));
    }
    let folder = map.get("folder")?.as_object()?;
    Some(RailEntry::Folder(FolderLayout {
        id: u16_of(folder.get("id"))?,
        name: folder
            .get("name")
            .and_then(serde_json::Value::as_str)
            .and_then(|name| SidebarLabel::try_from(name).ok()),
        open: folder
            .get("open")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        spaces: room_ids_of(folder.get("spaces")),
    }))
}

fn space_layout_from(value: &serde_json::Value) -> Option<SpaceLayout> {
    let map = value.as_object()?;
    let mut layout = SpaceLayout::new(RoomId::from(
        map.get("space").and_then(serde_json::Value::as_str)?,
    ));
    layout.order = section_keys_of(map.get("order"));
    layout.collapsed = section_keys_of(map.get("collapsed"));
    layout.starred = room_ids_of(map.get("starred"));
    layout.custom = map
        .get("custom")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|section| {
            let section = section.as_object()?;
            Some(CustomSection {
                id: u16_of(section.get("id"))?,
                name: SidebarLabel::try_from(
                    section.get("name").and_then(serde_json::Value::as_str)?,
                )
                .ok()?,
                rooms: room_ids_of(section.get("rooms")),
            })
        })
        .collect();
    Some(layout)
}

/// Builds the layout from a parsed sidebar file, field by field, for
/// `account`. The version comes first, then the account; the rest is checked
/// entry by entry and what is invalid is dropped
/// ([`SidebarLayout::normalized`]).
fn sidebar_from_value(value: &serde_json::Value, account: &UserId) -> SidebarRead {
    let Some(map) = value.as_object() else {
        return SidebarRead::Corrupt;
    };
    match map.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => {}
        Some(newer) if newer > 1 => return SidebarRead::Newer(newer),
        _ => return SidebarRead::Corrupt,
    }
    let stored = map.get("account").and_then(serde_json::Value::as_str);
    if stored != Some(account.as_str()) || !is_valid_user_id(account.as_str()) {
        return SidebarRead::OtherAccount;
    }
    let list = |key: &str| {
        map.get(key)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
    };
    SidebarRead::Layout(
        SidebarLayout {
            account: account.clone(),
            rail: list("rail").filter_map(rail_entry_from).collect(),
            spaces: list("spaces").filter_map(space_layout_from).collect(),
        }
        .normalized(),
    )
}

fn id_strs(ids: &[RoomId]) -> Vec<&str> {
    ids.iter().map(RoomId::as_str).collect()
}

fn key_strs(keys: &[SectionKey]) -> Vec<String> {
    keys.iter().map(SectionKey::to_key).collect()
}

/// The JSON of a layout, built by hand: the serializer guard in
/// `session::tests` allows no serializing outside `session.rs`.
fn sidebar_json(layout: &SidebarLayout) -> serde_json::Value {
    use serde_json::{Value, json};

    let rail: Vec<Value> = layout
        .rail
        .iter()
        .map(|entry| match entry {
            RailEntry::Space(id) => json!({ "space": id.as_str() }),
            RailEntry::Folder(folder) => {
                let mut inner = json!({
                    "id": folder.id,
                    "open": folder.open,
                    "spaces": id_strs(&folder.spaces),
                });
                if let (Some(name), Some(map)) = (folder.name, inner.as_object_mut()) {
                    map.insert("name".into(), name.as_str().into());
                }
                json!({ "folder": inner })
            }
        })
        .collect();
    let spaces: Vec<Value> = layout
        .spaces
        .iter()
        .map(|space| {
            let custom: Vec<Value> = space
                .custom
                .iter()
                .map(|c| json!({ "id": c.id, "name": c.name.as_str(), "rooms": id_strs(&c.rooms) }))
                .collect();
            json!({
                "space": space.space.as_str(),
                "order": key_strs(&space.order),
                "collapsed": key_strs(&space.collapsed),
                "starred": id_strs(&space.starred),
                "custom": custom,
            })
        })
        .collect();
    json!({
        "version": 1,
        "account": layout.account.as_str(),
        "rail": rail,
        "spaces": spaces,
    })
}

/// Root of all Twicecat data on disk.
#[derive(Debug, Clone)]
pub struct DataPaths {
    root: PathBuf,
    themes: PathBuf,
}

impl DataPaths {
    /// Uses the OS data directory; `None` if it cannot be determined.
    pub fn from_os() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("io.github", "ga_lep", "Twicecat")?;
        Some(Self {
            root: dirs.data_dir().to_path_buf(),
            themes: dirs.config_dir().join("themes"),
        })
    }

    /// Uses `root` as the data directory (tests, `--data-dir`).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let themes = root.join("themes");
        Self { root, themes }
    }

    /// The data root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where custom theme files are read from: `<root>/themes` for
    /// [`DataPaths::at`], and `themes` in the OS config directory for
    /// [`DataPaths::from_os`]. Nothing is created by asking.
    pub fn themes_dir(&self) -> &Path {
        &self.themes
    }

    /// Creates the themes folder (and its parents) and returns it. On Unix
    /// every directory created gets mode 0o700.
    pub fn create_themes_dir(&self) -> std::io::Result<PathBuf> {
        create_private_dir_all(&self.themes)?;
        Ok(self.themes.clone())
    }

    /// `<root>/sessions/<session_id>/`.
    ///
    /// The id must be 1..=128 characters of `[A-Za-z0-9_-]` and must not be a
    /// Windows reserved device name (`CON`, `NUL`, `COM1`, ...).
    pub fn session_store_dir(&self, session_id: &str) -> Result<PathBuf, PathsError> {
        validate_session_id(session_id)?;
        Ok(self.root.join("sessions").join(session_id))
    }

    /// Creates `<root>/sessions/<session_id>/` (and the root) and returns it.
    /// On Unix every directory created gets mode 0o700.
    pub fn create_session_store_dir(&self, session_id: &str) -> Result<PathBuf, PathsError> {
        let dir = self.session_store_dir(session_id)?;
        create_private_dir_all(&dir).map_err(|_| PathsError::Io)?;
        Ok(dir)
    }

    /// Reads the last-account marker. `Ok(None)` if there is none or it is
    /// empty.
    pub fn read_last_account(&self) -> std::io::Result<Option<String>> {
        match std::fs::read_to_string(self.last_account_file()) {
            Ok(text) => {
                let text = text.trim();
                Ok((!text.is_empty()).then(|| text.to_owned()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes the last-account marker (a user ID, never a secret). On Unix the
    /// file gets mode 0o600 and the root 0o700.
    pub fn write_last_account(&self, user_id: &str) -> std::io::Result<()> {
        use std::io::Write as _;

        create_private_dir_all(&self.root)?;
        let path = self.last_account_file();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        // `mode` applies only when the file is created; fix an older file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(user_id.as_bytes())
    }

    /// Removes the last-account marker. A missing file is `Ok`.
    pub fn remove_last_account(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.last_account_file()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Removes a session store directory. A missing directory is `Ok`.
    /// Blocking: on Windows it waits (at most [`STORE_RELEASE_WAIT`]) for
    /// files still in use, see [`remove_dir_all_when_released`].
    pub fn remove_session_store_dir(&self, session_id: &str) -> Result<(), PathsError> {
        let dir = self.session_store_dir(session_id)?;
        match remove_dir_all_when_released(&dir, STORE_RELEASE_WAIT) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(PathsError::Io),
            _ => Ok(()),
        }
    }

    /// `<root>/settings.json`: the typed [`UiSettings`], no secrets, no content.
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Reads the local settings. A missing file gives the defaults. A file that
    /// cannot be read or parsed also gives the defaults, and a corrupt one is
    /// renamed `settings.json.bad` so the next write does not hide it. Unknown
    /// fields are ignored and missing ones take their default.
    pub fn read_settings(&self) -> UiSettings {
        let path = self.settings_file();
        let Some(bytes) = read_capped_regular_file(&path, MAX_SETTINGS_BYTES) else {
            return UiSettings::default();
        };
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(serde_json::Value::Object(map)) => settings_from_map(&map),
            _ => {
                // Best effort: if the rename fails the defaults still apply.
                let _ = std::fs::rename(&path, self.root.join("settings.json.bad"));
                UiSettings::default()
            }
        }
    }

    /// Writes the local settings atomically: a temporary file in the data root
    /// (mode 0o600 on Unix), then a rename over `settings.json`.
    pub fn write_settings(&self, settings: &UiSettings) -> std::io::Result<()> {
        use std::io::Write as _;

        create_private_dir_all(&self.root)?;
        // Built by hand: the serializer guard in `session::tests` allows no
        // serializing outside `session.rs`.
        let theme = match settings.theme {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        };
        let mut json = serde_json::json!({
            "theme": theme,
            "desktop_notifications": settings.desktop_notifications,
            "show_message_content": settings.show_message_content,
            "tray_badge": settings.tray_badge,
            "density": match settings.density {
                DensityChoice::Comfortable => "Comfortable",
                DensityChoice::Compact => "Compact",
            },
            "room_list": match settings.room_list {
                RoomListChoice::Expanded => "Expanded",
                RoomListChoice::Rail => "Rail",
            },
            "text_scale_percent": settings.text_scale_percent,
            "advanced_settings": settings.advanced_settings,
            "update_check": match settings.update_check {
                UpdateCheckChoice::Unasked => "Unasked",
                UpdateCheckChoice::Off => "Off",
                UpdateCheckChoice::On => "On",
            },
            "update_checked_at": settings.update_checked_at,
        });
        // No key while `None` (the built-in theme), so a later change of the
        // app's default reaches this user. `palette` is never written again.
        if let (Some(name), Some(map)) = (settings.theme_name, json.as_object_mut()) {
            map.insert("theme_name".into(), name.as_str().into());
        }
        let json = json.to_string();
        let tmp = self.root.join("settings.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let result = (|| {
            // Never write through a leftover file or symlink at the temp path.
            match std::fs::remove_file(&tmp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let mut file = options.open(&tmp)?;
            // `mode` applies only when the file is created; fix a stale one.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, self.settings_file())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// `<root>/last_account`: holds only a user ID, never a secret.
    pub fn last_account_file(&self) -> PathBuf {
        self.root.join("last_account")
    }

    /// `<root>/call_settings.json`: audio devices, processing, input mode and
    /// per-user volumes. No secrets.
    pub fn call_settings_file(&self) -> PathBuf {
        self.root.join("call_settings.json")
    }

    /// Reads the call settings file; a missing file is `Ok(None)`.
    pub fn read_call_settings(&self) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.call_settings_file()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replaces the call settings file atomically: writes a temporary file
    /// next to it, then renames it over the old one, so a crash leaves either
    /// the old or the new file. On Unix the file gets mode 0o600 and the
    /// root 0o700.
    pub fn write_call_settings(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_private("call_settings.json", bytes)
    }

    /// `<root>/video_settings.json`: camera, qualities, codec and encoder
    /// choices (plan Phase 9). No secrets.
    pub fn video_settings_file(&self) -> PathBuf {
        self.root.join("video_settings.json")
    }

    /// Reads the video settings file; a missing file is `Ok(None)`.
    pub fn read_video_settings(&self) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.video_settings_file()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replaces the video settings file atomically, as
    /// [`Self::write_call_settings`] does.
    pub fn write_video_settings(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_private("video_settings.json", bytes)
    }

    /// `<root>/screen_restore_token`: the ScreenCast portal's token for the
    /// screen or window picked last time, kept only while "remember my
    /// choice" is on (plan Phase 9 Task 8). It lets us share that choice
    /// again without the picker, so it is private (0o600) and never logged.
    pub fn screen_restore_token_file(&self) -> PathBuf {
        self.root.join("screen_restore_token")
    }

    /// The remembered restore token; `None` when there is none (or the file
    /// is not one we wrote: not a small regular file of text).
    pub fn read_screen_restore_token(&self) -> Option<String> {
        let bytes =
            read_capped_regular_file(&self.screen_restore_token_file(), MAX_SETTINGS_BYTES)?;
        let token = String::from_utf8(bytes).ok()?.trim().to_owned();
        (!token.is_empty()).then_some(token)
    }

    /// Replaces the remembered restore token atomically.
    pub fn write_screen_restore_token(&self, token: &str) -> std::io::Result<()> {
        self.write_private("screen_restore_token", token.as_bytes())
    }

    /// Forgets the restore token; a missing file is no error.
    pub fn remove_screen_restore_token(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.screen_restore_token_file()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// `<root>/sidebar-<digest>.json`: the sidebar layout of one account, see
    /// [`Self::read_sidebar`]. `<digest>` is the FNV-1a hash of the user ID as
    /// 16 lowercase hex digits, so a user ID never appears in a file name.
    pub fn sidebar_file(&self, account: &UserId) -> PathBuf {
        let digest = fnv1a_64(account.as_str().as_bytes());
        self.root.join(format!("sidebar-{digest:016x}.json"))
    }

    /// Reads `account`'s sidebar layout, and no other account's.
    ///
    /// - No file, or one that is not a regular file of at most
    ///   [`MAX_SIDEBAR_BYTES`]: [`SidebarRead::Missing`].
    /// - Not JSON, not an object, or no integer `version`: the file is renamed
    ///   `sidebar-<digest>.json.bad` and the answer is [`SidebarRead::Corrupt`].
    /// - A `version` above 1: [`SidebarRead::Newer`]; the file is untouched.
    /// - An `account` that is not `account`: [`SidebarRead::OtherAccount`];
    ///   the file is untouched.
    /// - Otherwise the layout, with every invalid entry dropped.
    ///
    /// Reading never creates a file.
    pub fn read_sidebar(&self, account: &UserId) -> SidebarRead {
        let read = self.read_sidebar_file(account);
        if read == SidebarRead::Corrupt {
            // Best effort: if the rename fails the default layout still applies.
            let path = self.sidebar_file(account);
            let mut bad = path.clone().into_os_string();
            bad.push(".bad");
            let _ = std::fs::rename(&path, bad);
        }
        read
    }

    /// [`Self::read_sidebar`] without setting a corrupt file aside.
    fn read_sidebar_file(&self, account: &UserId) -> SidebarRead {
        if !is_valid_user_id(account.as_str()) {
            return SidebarRead::Missing;
        }
        let Some(bytes) = read_capped_regular_file(&self.sidebar_file(account), MAX_SIDEBAR_BYTES)
        else {
            return SidebarRead::Missing;
        };
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => sidebar_from_value(&value, account),
            Err(_) => SidebarRead::Corrupt,
        }
    }

    /// Writes `layout` to the file of `layout.account`: atomically (a
    /// temporary file with mode 0o600 on Unix, then a rename), and only if the
    /// layout [fits](SidebarLayout::fits).
    ///
    /// The file already there is read first. If it was saved by a newer
    /// Twicecat or names another account, nothing is written
    /// ([`SidebarWriteError::Protected`]) and it stays byte for byte the same.
    /// A corrupt one is set aside as `.bad` first.
    pub fn write_sidebar(&self, layout: &SidebarLayout) -> Result<(), SidebarWriteError> {
        use std::io::Write as _;

        if !layout.fits() {
            return Err(SidebarWriteError::Invalid);
        }
        if matches!(
            self.read_sidebar(&layout.account),
            SidebarRead::Newer(_) | SidebarRead::OtherAccount
        ) {
            return Err(SidebarWriteError::Protected);
        }
        let json = sidebar_json(layout).to_string();
        let path = self.sidebar_file(&layout.account);
        let mut tmp_name = path.clone().into_os_string();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let result = (|| {
            create_private_dir_all(&self.root)?;
            // Never write through a leftover file or symlink at the temp path.
            match std::fs::remove_file(&tmp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let mut file = options.open(&tmp)?;
            // `mode` applies only when the file is created; fix a stale one.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(SidebarWriteError::Io(e));
        }
        Ok(())
    }

    /// Removes `account`'s sidebar file (Reset). A missing file is `Ok`. A
    /// file saved by a newer Twicecat, or naming another account, is not
    /// removed ([`SidebarWriteError::Protected`]).
    pub fn remove_sidebar(&self, account: &UserId) -> Result<(), SidebarWriteError> {
        if matches!(
            self.read_sidebar_file(account),
            SidebarRead::Newer(_) | SidebarRead::OtherAccount
        ) {
            return Err(SidebarWriteError::Protected);
        }
        match std::fs::remove_file(self.sidebar_file(account)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(SidebarWriteError::Io(e)),
            _ => Ok(()),
        }
    }

    fn write_private(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write as _;

        create_private_dir_all(&self.root)?;
        let path = self.root.join(name);
        let tmp = self.root.join(format!("{name}.tmp"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    }
}

/// How long removing a store waits for its files to be released.
const STORE_RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// `remove_dir_all`, retried while a file in `dir` is still open. Windows
/// cannot delete an open file: the SDK's SQLite stores close their
/// connections shortly after the client is dropped (25 to 50 ms measured),
/// so a removal right after a failed login or a logout found them in use
/// and left the session's store behind. Elsewhere open files are unlinked
/// and the first attempt is the only one.
fn remove_dir_all_when_released(dir: &Path, wait: std::time::Duration) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match std::fs::remove_dir_all(dir) {
            Err(e) if in_use(&e) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

/// Whether `e` says a file is open elsewhere: Windows' sharing violation,
/// or access denied for a file already marked for deletion.
fn in_use(e: &std::io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    cfg!(windows)
        && matches!(
            e.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file opened as SQLite opens its database on Windows: shared for
    /// reading and writing, not for deletion (std's `File` allows deletion).
    #[cfg(windows)]
    fn open_like_sqlite(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_WRITE: u32 = 2;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(path)
            .unwrap()
    }

    #[test]
    #[cfg(windows)]
    fn a_store_still_open_is_removed_once_released() {
        let root = tempfile::tempdir().unwrap();
        let p = DataPaths::at(root.path().to_path_buf());
        let dir = p.create_session_store_dir("abc").unwrap();
        let file = open_like_sqlite(&dir.join("store.sqlite3"));
        // Closed 100 ms from now, as the SDK's stores are after a drop.
        let closer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(file);
        });
        p.remove_session_store_dir("abc").unwrap();
        closer.join().unwrap();
        assert!(!dir.exists());
    }

    #[test]
    #[cfg(windows)]
    fn a_store_never_released_is_an_error_after_the_wait() {
        let root = tempfile::tempdir().unwrap();
        let p = DataPaths::at(root.path().to_path_buf());
        let dir = p.create_session_store_dir("abc").unwrap();
        let file = open_like_sqlite(&dir.join("store.sqlite3"));
        let started = std::time::Instant::now();
        let result = remove_dir_all_when_released(&dir, std::time::Duration::from_millis(200));
        assert!(result.is_err());
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
        drop(file);
    }

    #[test]
    fn paths_are_under_root() {
        let p = DataPaths::at("/data/root");
        assert_eq!(p.root(), Path::new("/data/root"));
        let dir = p.session_store_dir("abc123").unwrap();
        assert_eq!(dir, Path::new("/data/root/sessions/abc123"));
        assert!(dir.starts_with(p.root()));
        assert_eq!(p.last_account_file(), Path::new("/data/root/last_account"));
    }

    #[test]
    fn bad_session_ids_are_rejected() {
        let p = DataPaths::at("/data/root");
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "../x",
            "a\\b",
            "..\\x",
            "/abs",
            "a\0b",
            "C:x",
            "C:",
            "a:b",
            "CON",
            "x.",
            "a b",
            "é",
            "a.b",
            &"a".repeat(129),
        ] {
            assert_eq!(
                p.session_store_dir(bad),
                Err(PathsError::InvalidSessionId),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn good_session_ids_are_accepted() {
        let p = DataPaths::at("/data/root");
        for good in [
            "a",
            "abc_DEF-123",
            "0123456789abcdef",
            &"a".repeat(128),
            "CONX",
            "COM10",
        ] {
            assert!(p.session_store_dir(good).is_ok(), "{good:?}");
        }
        for reserved in ["con", "NUL", "Com1", "lpt9", "aux", "PRN"] {
            assert!(p.session_store_dir(reserved).is_err(), "{reserved:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_modes_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let dir = p.create_session_store_dir("abc").unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(p.root()), 0o700);
        assert_eq!(mode(&p.root().join("sessions")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        p.write_last_account("@a:x").unwrap();
        assert_eq!(mode(&p.last_account_file()), 0o600);
        // An older, world-readable file is tightened.
        std::fs::set_permissions(
            p.last_account_file(),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        p.write_last_account("@b:x").unwrap();
        assert_eq!(mode(&p.last_account_file()), 0o600);
        assert_eq!(p.read_last_account().unwrap().as_deref(), Some("@b:x"));
        p.remove_last_account().unwrap();
        p.remove_last_account().unwrap();
        assert_eq!(p.read_last_account().unwrap(), None);
        p.remove_session_store_dir("abc").unwrap();
        p.remove_session_store_dir("abc").unwrap();
        assert!(!dir.exists());
    }

    /// Q16 (c): after a session starts, the data root holds only the
    /// last-account marker and the store directory (which the SDK fills with
    /// its encrypted store). No token or passphrase file lives next to them.
    #[test]
    fn the_data_root_holds_only_the_marker_and_the_sessions_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.create_session_store_dir("abc").unwrap();
        p.write_last_account("@a:x").unwrap();
        p.write_settings(&UiSettings::default()).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(p.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["last_account", "sessions", "settings.json"]);
        let sessions: Vec<String> = std::fs::read_dir(p.root().join("sessions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(sessions, ["abc"]);
        // The store directory is empty until the SDK writes its database.
        assert_eq!(
            std::fs::read_dir(p.root().join("sessions/abc"))
                .unwrap()
                .count(),
            0
        );
        // The marker holds the user ID and nothing else.
        assert_eq!(p.read_last_account().unwrap().as_deref(), Some("@a:x"));
    }

    #[test]
    fn settings_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        assert_eq!(p.settings_file(), tmp.path().join("root/settings.json"));
        assert_eq!(p.read_settings(), UiSettings::default());
    }

    #[test]
    fn settings_round_trip_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let s = UiSettings {
            theme: ThemeChoice::Dark,
            desktop_notifications: false,
            show_message_content: false,
            tray_badge: false,
            ..UiSettings::default()
        };
        p.write_settings(&s).unwrap();
        assert_eq!(p.read_settings(), s);
        // Overwriting works and leaves no temp file behind.
        p.write_settings(&UiSettings::default()).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        let names: Vec<_> = std::fs::read_dir(p.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[test]
    fn corrupt_settings_give_defaults_and_are_set_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(p.settings_file(), b"{ not json").unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        assert!(!p.settings_file().exists());
        assert_eq!(
            std::fs::read(p.root().join("settings.json.bad")).unwrap(),
            b"{ not json"
        );
    }

    #[test]
    fn settings_ignore_unknown_fields_and_fill_missing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(
            p.settings_file(),
            br#"{"theme":"Light","tray_badge":false,"from_the_future":{"x":1}}"#,
        )
        .unwrap();
        let s = p.read_settings();
        assert_eq!(s.theme, ThemeChoice::Light);
        assert!(!s.tray_badge);
        assert!(s.desktop_notifications && s.show_message_content);
        assert!(p.settings_file().exists());
    }

    fn read_text(p: &DataPaths, text: &str) -> UiSettings {
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(p.settings_file(), text).unwrap();
        p.read_settings()
    }

    #[test]
    fn an_old_file_reads_with_the_appearance_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let s = read_text(&p, r#"{"theme":"Dark"}"#);
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.theme_name, None);
        assert_eq!(s.density, DensityChoice::Comfortable);
        assert_eq!(s.room_list, RoomListChoice::Expanded);
        assert_eq!(s.text_scale_percent, 100);
        // Settings are simple until the person asks for the rest.
        assert!(!s.advanced_settings);
    }

    #[test]
    fn the_advanced_switch_is_kept_and_a_wrong_value_is_off() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.write_settings(&UiSettings {
            advanced_settings: true,
            ..UiSettings::default()
        })
        .unwrap();
        assert!(p.read_settings().advanced_settings);
        assert!(!read_text(&p, r#"{"advanced_settings":"yes"}"#).advanced_settings);
        assert!(read_text(&p, r#"{"advanced_settings":true}"#).advanced_settings);
    }

    #[test]
    fn the_update_choice_and_time_are_kept_and_an_old_file_asks() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for choice in [
            UpdateCheckChoice::Unasked,
            UpdateCheckChoice::Off,
            UpdateCheckChoice::On,
        ] {
            let s = UiSettings {
                update_check: choice,
                update_checked_at: 1_800_000_000,
                ..UiSettings::default()
            };
            p.write_settings(&s).unwrap();
            assert_eq!(p.read_settings(), s);
        }
        // A file from before the check: not asked, never checked.
        let s = read_text(&p, r#"{"theme":"Dark"}"#);
        assert_eq!(s.update_check, UpdateCheckChoice::Unasked);
        assert_eq!(s.update_checked_at, 0);
        // Wrong values fall back per field.
        let s = read_text(&p, r#"{"update_check":"Sometimes","update_checked_at":-5}"#);
        assert_eq!(s.update_check, UpdateCheckChoice::Unasked);
        assert_eq!(s.update_checked_at, 0);
    }

    #[test]
    fn an_unknown_appearance_value_falls_back_per_field() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let s = read_text(
            &p,
            r#"{"theme_name":"Nope/","theme":"Dark","density":"Huge","room_list":7,"text_scale_percent":"x","tray_badge":false}"#,
        );
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.theme_name, None);
        assert_eq!(s.density, DensityChoice::Comfortable);
        assert_eq!(s.room_list, RoomListChoice::Expanded);
        assert_eq!(s.text_scale_percent, 100);
        assert!(!s.tray_badge);
        assert!(p.settings_file().exists());
        assert!(!p.root().join("settings.json.bad").exists());
    }

    #[test]
    fn text_scale_is_clamped_and_stepped_when_read() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for (raw, want) in [
            (0u32, 80u16),
            (79, 80),
            (85, 90),
            (200, 200),
            (250, 200),
            (u32::from(u16::MAX), 200),
        ] {
            let s = read_text(&p, &format!(r#"{{"text_scale_percent":{raw}}}"#));
            assert_eq!(s.text_scale_percent, want, "{raw}");
        }
    }

    fn name(text: &str) -> ThemeName {
        ThemeName::try_from(text).expect("a valid name")
    }

    #[test]
    fn a_theme_name_of_none_leaves_no_key_in_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.write_settings(&UiSettings::default()).unwrap();
        let text = std::fs::read_to_string(p.settings_file()).unwrap();
        assert!(!text.contains("theme_name"), "{text}");
        assert!(!text.contains("palette"), "{text}");
    }

    #[test]
    fn every_appearance_value_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for theme_name in [None, Some(name("Daylight"))] {
            for theme in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                for density in [DensityChoice::Comfortable, DensityChoice::Compact] {
                    for room_list in [RoomListChoice::Expanded, RoomListChoice::Rail] {
                        for text_scale_percent in [80, 100, 130, 200] {
                            let s = UiSettings {
                                theme_name,
                                theme,
                                density,
                                room_list,
                                text_scale_percent,
                                ..UiSettings::default()
                            };
                            p.write_settings(&s).unwrap();
                            assert_eq!(p.read_settings(), s);
                            let text = std::fs::read_to_string(p.settings_file()).unwrap();
                            assert_eq!(text.contains("theme_name"), theme_name.is_some());
                            assert!(!text.contains("palette"), "{text}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn an_older_palette_reads_as_the_default_and_is_dropped_on_the_next_write() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for old in ["Frost", "Ember", "HighContrast", "Twicecat", "Bogus"] {
            let s = read_text(&p, &format!(r#"{{"palette":"{old}","theme":"Light"}}"#));
            assert_eq!(s.theme_name, None, "{old}");
            assert_eq!(s.theme, ThemeChoice::Light, "{old}");
            assert!(p.settings_file().exists());
            assert!(!p.root().join("settings.json.bad").exists());
            p.write_settings(&s).unwrap();
            let text = std::fs::read_to_string(p.settings_file()).unwrap();
            assert!(!text.contains("palette"), "{text}");
            assert!(text.contains(r#""theme":"Light""#), "{text}");
        }
        let s = read_text(&p, r#"{"palette":"Frost","theme_name":"Daylight"}"#);
        assert_eq!(s.theme_name, Some(name("Daylight")));
    }

    #[test]
    fn a_bad_theme_name_reads_as_none_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        let long = "a".repeat(33);
        let bad = [
            r#""../x""#,
            r#""a/b""#,
            r#""C:\\x""#,
            r#""""#,
            r#"" lead""#,
            &format!("\"{long}\""),
            r#""\u202e""#,
            r#""TwiceCat""#,
            r#""DEFAULT""#,
            r#""Day.light""#,
            r#""CON""#,
            r#""nul""#,
            r#""Com1""#,
            r#""LPT9""#,
            r#""aux""#,
            r#""PRN""#,
            "5",
            "null",
        ];
        for value in bad {
            let s = read_text(
                &p,
                &format!(r#"{{"theme_name":{value},"theme":"Dark","tray_badge":false}}"#),
            );
            assert_eq!(s.theme_name, None, "{value}");
            assert_eq!(s.theme, ThemeChoice::Dark, "{value}");
            assert!(!s.tray_badge, "{value}");
            assert!(!p.root().join("settings.json.bad").exists());
        }
    }

    #[test]
    fn names_that_only_look_like_device_names_are_fine() {
        for ok in ["COM0", "COM10", "LPT", "Console", "Nul 1", "com", "AUX2"] {
            assert!(ThemeName::try_from(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn the_themes_folder_is_under_the_root_for_a_given_root() {
        let p = DataPaths::at("/some/root");
        assert_eq!(p.themes_dir(), Path::new("/some/root/themes"));
    }

    #[test]
    fn the_os_themes_folder_is_next_to_the_config_dir() {
        let p = DataPaths::from_os().expect("OS dirs available on the test machine");
        let dirs = directories::ProjectDirs::from("io.github", "ga_lep", "Twicecat").unwrap();
        assert!(p.themes_dir().ends_with("themes"));
        assert_eq!(p.themes_dir().parent(), Some(dirs.config_dir()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_themes_folder_follows_xdg_config_home() {
        // `directories` reads the environment, so check in a child process
        // (this test binary re-run with a marker) and leave ours untouched.
        const MARK: &str = "TWICECAT_TEST_XDG_CHILD";
        if let Ok(want) = std::env::var(MARK) {
            let p = DataPaths::from_os().expect("OS dirs");
            assert_eq!(p.themes_dir(), Path::new(&want).join("twicecat/themes"));
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let xdg = tmp.path().join("cfg");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "paths::tests::the_themes_folder_follows_xdg_config_home",
                "--nocapture",
            ])
            .env("XDG_CONFIG_HOME", &xdg)
            .env(MARK, &xdg)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[cfg(unix)]
    #[test]
    fn creating_the_themes_folder_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        assert!(!p.themes_dir().exists());
        let dir = p.create_themes_dir().unwrap();
        assert_eq!(dir, p.themes_dir());
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // Again: fine.
        p.create_themes_dir().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn settings_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        p.write_settings(&UiSettings::default()).unwrap();
        let mode = std::fs::metadata(p.settings_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn from_os_returns_some() {
        let p = DataPaths::from_os().expect("OS data dir available on the test machine");
        assert!(p.root().is_absolute());
        assert!(p.last_account_file().starts_with(p.root()));
    }

    #[test]
    fn every_theme_and_bool_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        for theme in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            for bits in 0..8u8 {
                let s = UiSettings {
                    theme,
                    desktop_notifications: bits & 1 != 0,
                    show_message_content: bits & 2 != 0,
                    tray_badge: bits & 4 != 0,
                    ..UiSettings::default()
                };
                p.write_settings(&s).unwrap();
                assert_eq!(p.read_settings(), s);
            }
        }
    }

    #[test]
    fn call_settings_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        assert_eq!(paths.read_call_settings().unwrap(), None);
        paths.write_call_settings(b"{\"a\":1}").unwrap();
        paths.write_call_settings(b"{\"a\":2}").unwrap();
        assert_eq!(
            paths.read_call_settings().unwrap().as_deref(),
            Some(&b"{\"a\":2}"[..])
        );
        assert!(!dir.path().join("root/call_settings.json.tmp").exists());
        assert!(paths.call_settings_file().starts_with(paths.root()));
    }

    #[test]
    fn the_screen_restore_token_is_kept_privately_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        assert_eq!(paths.read_screen_restore_token(), None);
        paths.remove_screen_restore_token().unwrap();
        paths.write_screen_restore_token("token-1").unwrap();
        paths.write_screen_restore_token("token-2").unwrap();
        assert_eq!(
            paths.read_screen_restore_token().as_deref(),
            Some("token-2")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(paths.screen_restore_token_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        paths.remove_screen_restore_token().unwrap();
        assert_eq!(paths.read_screen_restore_token(), None);
    }

    #[test]
    fn video_settings_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        assert_eq!(paths.read_video_settings().unwrap(), None);
        paths.write_video_settings(b"{\"a\":1}").unwrap();
        paths.write_video_settings(b"{\"a\":2}").unwrap();
        assert_eq!(
            paths.read_video_settings().unwrap().as_deref(),
            Some(&b"{\"a\":2}"[..])
        );
        assert!(!dir.path().join("root/video_settings.json.tmp").exists());
        assert!(paths.read_call_settings().unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_loose_temp_file_is_tightened_before_the_write() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        let stale = p.root().join("settings.json.tmp");
        std::fs::write(&stale, b"old").unwrap();
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o666)).unwrap();
        p.write_settings(&UiSettings::default()).unwrap();
        let mode = std::fs::metadata(p.settings_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn an_oversized_settings_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.root()).unwrap();
        let mut big = br#"{"theme":"Dark","pad":""#.to_vec();
        big.resize(MAX_SETTINGS_BYTES as usize + 10, b'a');
        big.extend_from_slice(b"\"}");
        std::fs::write(p.settings_file(), &big).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
    }

    #[test]
    fn a_settings_path_that_is_not_a_regular_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        std::fs::create_dir_all(p.settings_file()).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        assert!(p.settings_file().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn call_settings_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        paths.write_call_settings(b"{}").unwrap();
        let mode = std::fs::metadata(paths.call_settings_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // ---- Sidebar layout ---------------------------------------------------

    use crate::api::sidebar::limits::*;
    use crate::api::{
        CustomSection, FolderLayout, RailEntry, RoomId, SectionKey, SidebarLabel, SidebarRead,
        SpaceLayout,
    };

    fn user(id: &str) -> UserId {
        UserId::from(id)
    }

    fn rid(n: usize) -> RoomId {
        RoomId::from(format!("!r{n}:x"))
    }

    fn lbl(text: &str) -> SidebarLabel {
        SidebarLabel::try_from(text).unwrap()
    }

    fn sidebar_paths() -> (tempfile::TempDir, DataPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths::at(tmp.path().join("root"));
        (tmp, p)
    }

    /// A layout with every variant in it.
    fn full_layout(account: &str) -> SidebarLayout {
        let mut l = SidebarLayout::new(user(account));
        l.rail = vec![
            RailEntry::Space(rid(1)),
            RailEntry::Folder(FolderLayout {
                id: 1,
                name: Some(lbl("Work")),
                open: false,
                spaces: vec![rid(2), rid(3)],
            }),
            RailEntry::Folder(FolderLayout {
                id: 9,
                name: None,
                open: true,
                spaces: vec![rid(4), rid(5)],
            }),
        ];
        let mut a = SpaceLayout::new(rid(1));
        a.order = vec![
            SectionKey::Starred,
            SectionKey::Channels,
            SectionKey::SubSpace(rid(20)),
            SectionKey::Custom(2),
            SectionKey::Direct,
        ];
        a.collapsed = vec![SectionKey::SubSpace(rid(20)), SectionKey::Direct];
        a.starred = vec![rid(30)];
        a.custom = vec![CustomSection {
            id: 2,
            name: lbl("Ops"),
            rooms: vec![rid(31), rid(32)],
        }];
        l.spaces = vec![a, SpaceLayout::new(rid(2))];
        l
    }

    fn put(p: &DataPaths, account: &str, text: &str) {
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(p.sidebar_file(&user(account)), text).unwrap();
    }

    fn read_bytes(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_sidebar_file_name_is_stable_and_per_account() {
        let (_t, p) = sidebar_paths();
        let name = |id: &str| {
            p.sidebar_file(&user(id))
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(name("@alice:example.org"), "sidebar-0862de498e9b8025.json");
        assert_eq!(name("@a:x"), "sidebar-82c93996fd659248.json");
        assert_ne!(name("@a:x"), name("@b:x"));
        assert_eq!(p.sidebar_file(&user("@a:x")).parent().unwrap(), p.root());
        let long = format!("@{}", "a".repeat(254));
        let n = name(&long);
        assert_eq!(n.len(), "sidebar-".len() + 16 + ".json".len());
        assert!(n.starts_with("sidebar-") && n.ends_with(".json"));
        assert!(
            n["sidebar-".len()..n.len() - 5]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn a_layout_round_trips_and_the_file_is_private_and_versioned() {
        let (_t, p) = sidebar_paths();
        let l = full_layout("@a:x");
        assert!(l.fits());
        p.write_sidebar(&l).unwrap();
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Layout(l));
        let text = std::fs::read_to_string(p.sidebar_file(&user("@a:x"))).unwrap();
        assert!(text.contains("\"version\":1"), "{text}");
        let leftovers: Vec<_> = std::fs::read_dir(p.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(p.sidebar_file(&user("@a:x")))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn writing_replaces_the_layout_and_tightens_a_stale_temp_file() {
        let (_t, p) = sidebar_paths();
        let mut l = full_layout("@a:x");
        p.write_sidebar(&l).unwrap();
        l.rail.truncate(1);
        let tmp = p.root().join(format!(
            "{}.tmp",
            p.sidebar_file(&user("@a:x"))
                .file_name()
                .unwrap()
                .to_string_lossy()
        ));
        std::fs::write(&tmp, "stale").unwrap();
        p.write_sidebar(&l).unwrap();
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Layout(l));
        assert!(!tmp.exists());
    }

    #[test]
    fn a_missing_file_reads_as_missing_and_reading_creates_nothing() {
        let (_t, p) = sidebar_paths();
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Missing);
        assert!(!p.root().exists());
        std::fs::create_dir_all(p.root()).unwrap();
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Missing);
        assert_eq!(std::fs::read_dir(p.root()).unwrap().count(), 0);
    }

    #[test]
    fn the_data_root_has_no_sidebar_file_until_one_is_written() {
        let (_t, p) = sidebar_paths();
        p.create_session_store_dir("abc").unwrap();
        p.write_settings(&UiSettings::default()).unwrap();
        let _ = p.read_sidebar(&user("@a:x"));
        let mut names: Vec<String> = std::fs::read_dir(p.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["sessions", "settings.json"]);
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_settings_are_untouched() {
        for bad in [
            "not json",
            "[1,2]",
            "{}",
            r#"{"version":"1","account":"@a:x"}"#,
            r#"{"version":1.5}"#,
            r#"{"version":-1}"#,
            r#"{"version":0,"account":"@a:x"}"#,
            "",
        ] {
            let (_t, p) = sidebar_paths();
            p.write_settings(&UiSettings::default()).unwrap();
            let settings = read_bytes(&p.settings_file());
            put(&p, "@a:x", bad);
            assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Corrupt, "{bad}");
            let file = p.sidebar_file(&user("@a:x"));
            assert!(!file.exists(), "{bad}");
            let bad_file = PathBuf::from(format!("{}.bad", file.display()));
            assert_eq!(read_bytes(&bad_file), bad.as_bytes(), "{bad}");
            assert_eq!(read_bytes(&p.settings_file()), settings);
            assert!(!p.root().join("settings.json.bad").exists());
            // The next read sees no file.
            assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Missing);
        }
    }

    #[test]
    fn a_newer_version_is_reported_and_left_alone() {
        let (_t, p) = sidebar_paths();
        let text = r#"{"version":2,"account":"@a:x","future":[1,2,3]}"#;
        put(&p, "@a:x", text);
        let file = p.sidebar_file(&user("@a:x"));
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Newer(2));
        assert_eq!(read_bytes(&file), text.as_bytes());
        assert!(!PathBuf::from(format!("{}.bad", file.display())).exists());
        // Even for a huge version number.
        put(&p, "@a:x", r#"{"version":18446744073709551615}"#);
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Newer(u64::MAX));
    }

    #[test]
    fn another_accounts_file_is_ignored_and_never_overwritten() {
        let (_t, p) = sidebar_paths();
        p.write_sidebar(&full_layout("@a:x")).unwrap();
        let a_file = p.sidebar_file(&user("@a:x"));
        let before = read_bytes(&a_file);
        let mtime = std::fs::metadata(&a_file).unwrap().modified().unwrap();
        assert_eq!(p.read_sidebar(&user("@b:x")), SidebarRead::Missing);
        assert_eq!(read_bytes(&a_file), before);
        assert_eq!(
            std::fs::metadata(&a_file).unwrap().modified().unwrap(),
            mtime
        );

        let mut b = full_layout("@b:x");
        b.rail.truncate(1);
        p.write_sidebar(&b).unwrap();
        assert_eq!(read_bytes(&a_file), before);
        assert_eq!(
            std::fs::metadata(&a_file).unwrap().modified().unwrap(),
            mtime
        );
        assert_eq!(p.read_sidebar(&user("@b:x")), SidebarRead::Layout(b));
        assert_eq!(
            p.read_sidebar(&user("@a:x")),
            SidebarRead::Layout(full_layout("@a:x"))
        );
    }

    #[test]
    fn a_file_for_another_account_at_this_path_is_protected() {
        for text in [
            r#"{"version":1,"account":"@a:x","rail":[]}"#,
            r#"{"version":1,"rail":[]}"#,
            r#"{"version":1,"account":"bad id","rail":[]}"#,
            r#"{"version":1,"account":5}"#,
        ] {
            let (_t, p) = sidebar_paths();
            // The collision or planted case: `@a:x`'s content at `@b:x`'s path.
            put(&p, "@b:x", text);
            let file = p.sidebar_file(&user("@b:x"));
            assert_eq!(
                p.read_sidebar(&user("@b:x")),
                SidebarRead::OtherAccount,
                "{text}"
            );
            assert_eq!(read_bytes(&file), text.as_bytes());
            assert!(!PathBuf::from(format!("{}.bad", file.display())).exists());
            let mut mine = full_layout("@b:x");
            mine.rail.truncate(1);
            assert!(
                matches!(p.write_sidebar(&mine), Err(SidebarWriteError::Protected)),
                "{text}"
            );
            assert!(
                matches!(
                    p.remove_sidebar(&user("@b:x")),
                    Err(SidebarWriteError::Protected)
                ),
                "{text}"
            );
            assert_eq!(read_bytes(&file), text.as_bytes());
        }
    }

    #[test]
    fn a_newer_file_is_protected_from_write_and_remove() {
        let (_t, p) = sidebar_paths();
        let text = r#"{"version":2,"account":"@a:x"}"#;
        put(&p, "@a:x", text);
        let file = p.sidebar_file(&user("@a:x"));
        assert!(matches!(
            p.write_sidebar(&full_layout("@a:x")),
            Err(SidebarWriteError::Protected)
        ));
        assert!(matches!(
            p.remove_sidebar(&user("@a:x")),
            Err(SidebarWriteError::Protected)
        ));
        assert_eq!(read_bytes(&file), text.as_bytes());
        assert_eq!(std::fs::read_dir(p.root()).unwrap().count(), 1);
    }

    #[test]
    fn writing_over_a_corrupt_file_keeps_it_as_bad() {
        let (_t, p) = sidebar_paths();
        put(&p, "@a:x", "garbage");
        p.write_sidebar(&full_layout("@a:x")).unwrap();
        let file = p.sidebar_file(&user("@a:x"));
        assert_eq!(
            read_bytes(&PathBuf::from(format!("{}.bad", file.display()))),
            b"garbage"
        );
        assert_eq!(
            p.read_sidebar(&user("@a:x")),
            SidebarRead::Layout(full_layout("@a:x"))
        );
    }

    #[test]
    fn remove_deletes_only_this_accounts_file() {
        let (_t, p) = sidebar_paths();
        p.write_sidebar(&full_layout("@a:x")).unwrap();
        p.write_sidebar(&full_layout("@b:x")).unwrap();
        p.remove_sidebar(&user("@a:x")).unwrap();
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Missing);
        assert!(matches!(
            p.read_sidebar(&user("@b:x")),
            SidebarRead::Layout(_)
        ));
        // A missing file is fine.
        p.remove_sidebar(&user("@a:x")).unwrap();
        // A corrupt file can be reset too.
        put(&p, "@a:x", "garbage");
        p.remove_sidebar(&user("@a:x")).unwrap();
        assert!(!p.sidebar_file(&user("@a:x")).exists());
    }

    #[test]
    fn an_invalid_or_oversized_layout_is_refused() {
        let (_t, p) = sidebar_paths();
        assert!(matches!(
            p.write_sidebar(&SidebarLayout::new(user("nobody"))),
            Err(SidebarWriteError::Invalid)
        ));
        let mut l = SidebarLayout::new(user("@a:x"));
        l.rail = (0..300).map(|n| RailEntry::Space(rid(n))).collect();
        assert!(matches!(
            p.write_sidebar(&l),
            Err(SidebarWriteError::Invalid)
        ));
        assert!(!p.root().exists() || std::fs::read_dir(p.root()).unwrap().count() == 0);
        assert_eq!(p.read_sidebar(&user("nobody")), SidebarRead::Missing);
    }

    fn timed(p: &DataPaths, account: &str) -> SidebarRead {
        let start = std::time::Instant::now();
        let read = p.read_sidebar(&user(account));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "slow read"
        );
        read
    }

    fn layout_of(read: SidebarRead) -> SidebarLayout {
        match read {
            SidebarRead::Layout(l) => l,
            other => panic!("expected a layout, got {other:?}"),
        }
    }

    #[test]
    fn hostile_ids_and_labels_are_dropped_and_the_rest_kept() {
        let (_t, p) = sidebar_paths();
        let long = format!("!{}", "a".repeat(255));
        let text = serde_json::json!({
            "version": 1,
            "account": "@a:x",
            "rail": [
                {"space": "nobang"}, {"space": "!has space"}, {"space": "!caf\u{e9}"},
                {"space": long}, {"space": "!"}, {"space": 5}, 7, "x", {"other": 1},
                {"space": "!good1:x"},
                {"folder": {"id": 1, "name": "x".repeat(33), "open": true, "spaces": ["!f1", "!f2"]}},
                {"folder": {"id": 2, "name": "", "spaces": ["!f3", "!f4"]}},
                {"folder": {"id": 3, "name": "\u{202E}evil", "spaces": ["!f5", "!f6"]}},
                {"folder": {"id": 4, "name": "line\nbreak", "spaces": ["!f7", "!f8"]}},
                {"folder": {"id": 5, "name": "zw\u{200B}sp", "spaces": ["!f9", "!f10"]}},
                {"folder": {"id": 6, "name": "   ", "spaces": ["!f11", "!f12"]}},
                {"folder": {"id": 70000, "spaces": ["!g1", "!g2"]}},
                {"folder": {"id": -1, "spaces": ["!g3", "!g4"]}},
                {"folder": {"id": "7", "spaces": ["!g5", "!g6"]}},
                {"folder": {"id": 8, "name": "Fine", "open": "yes", "spaces": ["!h1", 4, "!h2"]}}
            ],
            "spaces": [
                {"space": "!good1:x", "order": ["starred", "bogus", "sub:bad", "custom:99999", "custom:3", "dms"],
                 "collapsed": ["channels", 5], "starred": ["!s1", "bad", "!s1"],
                 "custom": [{"id": 3, "name": "Ops", "rooms": ["!s1", "!c1", "?"]},
                            {"id": 4, "name": "\u{202E}", "rooms": ["!c2"]},
                            {"id": 5, "rooms": ["!c3"]}, {"name": "No id", "rooms": []}]},
                {"space": "bad id"}, {"nospace": 1}, 5
            ]
        })
        .to_string();
        put(&p, "@a:x", &text);
        let l = layout_of(timed(&p, "@a:x"));
        let spaces: Vec<_> = l
            .rail
            .iter()
            .filter_map(|e| match e {
                RailEntry::Space(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(spaces, ["!good1:x"]);
        let folders: Vec<_> = l
            .rail
            .iter()
            .filter_map(|e| match e {
                RailEntry::Folder(f) => Some(f),
                _ => None,
            })
            .collect();
        // Folders 1 to 6 keep their spaces but lose the bad name; the ones
        // with a bad id are dropped.
        let ids: Vec<_> = folders.iter().map(|f| f.id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 8]);
        assert!(folders[..6].iter().all(|f| f.name.is_none()));
        assert!(folders[0].open && folders[0].spaces.len() == 2);
        assert_eq!(folders[6].name, Some(lbl("Fine")));
        assert!(!folders[6].open);
        assert_eq!(
            folders[6].spaces,
            vec![RoomId::from("!h1"), RoomId::from("!h2")]
        );
        assert_eq!(l.spaces.len(), 1);
        let sp = &l.spaces[0];
        assert_eq!(
            sp.order,
            vec![
                SectionKey::Starred,
                SectionKey::Custom(3),
                SectionKey::Direct
            ]
        );
        assert_eq!(sp.collapsed, vec![SectionKey::Channels]);
        assert_eq!(sp.starred, vec![RoomId::from("!s1")]);
        assert_eq!(sp.custom.len(), 1);
        assert_eq!(sp.custom[0].rooms, vec![RoomId::from("!c1")]);
        assert!(l.fits());
    }

    #[test]
    fn duplicates_keep_the_first_across_the_rail_and_folders() {
        let (_t, p) = sidebar_paths();
        let text = serde_json::json!({
            "version": 1, "account": "@a:x",
            "rail": [
                {"space": "!a"}, {"space": "!a"},
                {"folder": {"id": 1, "spaces": ["!a", "!b", "!b", "!c"]}},
                {"space": "!c"}, {"space": "!d"},
                {"folder": {"id": 1, "spaces": ["!e", "!f"]}}
            ],
            "spaces": [
                {"space": "!a", "starred": ["!r1"]},
                {"space": "!a", "starred": ["!r2"]}
            ]
        })
        .to_string();
        put(&p, "@a:x", &text);
        let l = layout_of(timed(&p, "@a:x"));
        assert_eq!(
            l.rail,
            vec![
                RailEntry::Space(RoomId::from("!a")),
                RailEntry::Folder(FolderLayout {
                    id: 1,
                    name: None,
                    open: false,
                    spaces: vec![RoomId::from("!b"), RoomId::from("!c")]
                }),
                RailEntry::Space(RoomId::from("!d")),
            ]
        );
        assert_eq!(l.spaces.len(), 1);
        assert_eq!(l.spaces[0].starred, vec![RoomId::from("!r1")]);
    }

    #[test]
    fn a_hostile_file_of_too_many_entries_is_cut_to_the_limits() {
        let (_t, p) = sidebar_paths();
        let rail: Vec<_> = (0..300)
            .map(|n| serde_json::json!({"space": format!("!s{n}")}))
            .collect();
        let folders: Vec<_> = (0..60)
            .map(|n| serde_json::json!({"folder": {"id": n, "spaces": [format!("!fa{n}"), format!("!fb{n}")]}}))
            .collect();
        let all_rail = [folders, rail].concat();
        let sections: Vec<_> = (0..30)
            .map(|n| serde_json::json!({"id": n, "name": "S", "rooms": [format!("!c{n}")]}))
            .collect();
        let starred: Vec<_> = (0..5000).map(|n| format!("!x{n}")).collect();
        let text = serde_json::json!({
            "version": 1, "account": "@a:x",
            "rail": all_rail,
            "spaces": [{"space": "!s0", "custom": sections, "starred": starred}]
        })
        .to_string();
        assert!(text.len() < MAX_SIDEBAR_BYTES as usize);
        put(&p, "@a:x", &text);
        let l = layout_of(timed(&p, "@a:x"));
        assert!(l.rail.len() <= MAX_RAIL_ENTRIES);
        assert!(
            l.rail
                .iter()
                .filter(|e| matches!(e, RailEntry::Folder(_)))
                .count()
                <= MAX_FOLDERS
        );
        assert_eq!(l.spaces[0].custom.len(), MAX_CUSTOM_SECTIONS);
        assert_eq!(l.spaces[0].starred.len(), MAX_PLACED_ROOMS);
        assert!(l.fits());
    }

    #[test]
    fn deeply_nested_json_is_corrupt_not_a_crash() {
        let (_t, p) = sidebar_paths();
        let text = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        put(&p, "@a:x", &text);
        assert_eq!(timed(&p, "@a:x"), SidebarRead::Corrupt);
        let text = format!(
            r#"{{"version":1,"account":"@a:x","rail":{}1{}}}"#,
            "[".repeat(200),
            "]".repeat(200)
        );
        put(&p, "@a:x", &text);
        assert_eq!(timed(&p, "@a:x"), SidebarRead::Corrupt);
    }

    #[test]
    fn an_oversized_sidebar_file_reads_as_missing() {
        let (_t, p) = sidebar_paths();
        let mut big = br#"{"version":1,"account":"@a:x","pad":""#.to_vec();
        big.resize(MAX_SIDEBAR_BYTES as usize + 10, b'a');
        big.extend_from_slice(b"\"}");
        std::fs::create_dir_all(p.root()).unwrap();
        std::fs::write(p.sidebar_file(&user("@a:x")), &big).unwrap();
        assert_eq!(timed(&p, "@a:x"), SidebarRead::Missing);
        assert_eq!(read_bytes(&p.sidebar_file(&user("@a:x"))), big);
        // Just under the cap parses.
        let mut ok = br#"{"version":1,"account":"@a:x","pad":""#.to_vec();
        ok.resize(MAX_SIDEBAR_BYTES as usize - 2, b'a');
        ok.extend_from_slice(b"\"}");
        assert_eq!(ok.len() as u64, MAX_SIDEBAR_BYTES);
        std::fs::write(p.sidebar_file(&user("@a:x")), &ok).unwrap();
        assert!(matches!(timed(&p, "@a:x"), SidebarRead::Layout(_)));
    }

    #[test]
    fn a_sidebar_path_that_is_not_a_regular_file_reads_as_missing() {
        let (_t, p) = sidebar_paths();
        std::fs::create_dir_all(p.sidebar_file(&user("@a:x"))).unwrap();
        assert_eq!(timed(&p, "@a:x"), SidebarRead::Missing);
        assert!(p.sidebar_file(&user("@a:x")).is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_or_a_symlink_to_one_reads_as_missing_without_blocking() {
        let (_t, p) = sidebar_paths();
        std::fs::create_dir_all(p.root()).unwrap();
        let fifo = p.root().join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        std::os::unix::fs::symlink(&fifo, p.sidebar_file(&user("@a:x"))).unwrap();
        assert_eq!(timed(&p, "@a:x"), SidebarRead::Missing);
        std::fs::remove_file(p.sidebar_file(&user("@a:x"))).unwrap();
        let direct = p.sidebar_file(&user("@b:x"));
        let made = std::process::Command::new("mkfifo")
            .arg(&direct)
            .status()
            .unwrap();
        assert!(made.success());
        assert_eq!(timed(&p, "@b:x"), SidebarRead::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_regular_file_is_read() {
        let (_t, p) = sidebar_paths();
        p.write_sidebar(&full_layout("@a:x")).unwrap();
        let real = p.sidebar_file(&user("@a:x"));
        let moved = p.root().join("elsewhere.json");
        std::fs::rename(&real, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &real).unwrap();
        assert_eq!(
            p.read_sidebar(&user("@a:x")),
            SidebarRead::Layout(full_layout("@a:x"))
        );
    }

    /// The largest layout the budget accepts fits in the file cap, in the
    /// worst case for size: IDs full of characters that JSON escapes.
    #[test]
    fn the_largest_layout_that_fits_is_written_under_the_file_cap() {
        let (_t, p) = sidebar_paths();
        let worst = |tag: char, n: usize| {
            // 255 bytes, every byte but the first and the counter a quote or a backslash.
            let counter = format!("{n:0>6}");
            let filler: String = std::iter::repeat_n(
                if n.is_multiple_of(2) { '"' } else { '\\' },
                255 - 1 - counter.len() - 1,
            )
            .collect();
            RoomId::from(format!("!{tag}{counter}{filler}"))
        };
        let name = lbl(&"\"".repeat(32));
        let mut l = SidebarLayout::new(user(&format!("@{}", "\"".repeat(254))));
        l.rail = (0..200).map(|n| RailEntry::Space(worst('r', n))).collect();
        for s in 0..200 {
            let mut sp = SpaceLayout::new(worst('s', s));
            sp.order = (0..20)
                .map(|n| SectionKey::SubSpace(worst('o', s * 20 + n)))
                .collect();
            sp.collapsed = sp.order.clone();
            sp.starred = (0..20).map(|n| worst('t', s * 20 + n)).collect();
            sp.custom = (0..20)
                .map(|n| CustomSection {
                    id: n,
                    name,
                    rooms: vec![worst('c', s * 20 + usize::from(n))],
                })
                .collect();
            l.spaces.push(sp);
        }
        let n = l.normalized();
        assert!(n.fits());
        // It really is big: the budget, not the input, ended it.
        let refs: usize = n.rail.len()
            + n.spaces
                .iter()
                .map(|s| {
                    1 + s.order.len()
                        + s.collapsed.len()
                        + s.starred.len()
                        + s.custom.iter().map(|c| c.rooms.len()).sum::<usize>()
                })
                .sum::<usize>();
        assert!(refs > 300, "{refs}");
        p.write_sidebar(&n).unwrap();
        let size = std::fs::metadata(p.sidebar_file(&n.account)).unwrap().len();
        assert!(size <= MAX_SIDEBAR_BYTES, "{size}");
        assert!(
            size > MAX_SIDEBAR_BYTES / 2,
            "{size}: the estimate is not far too careful"
        );
        assert_eq!(p.read_sidebar(&n.account), SidebarRead::Layout(n));
    }

    #[test]
    fn a_typical_big_layout_round_trips() {
        let (_t, p) = sidebar_paths();
        let mut l = SidebarLayout::new(user("@a:x"));
        l.rail = (0..150).map(|n| RailEntry::Space(rid(n))).collect();
        for s in 0..20 {
            let mut sp = SpaceLayout::new(rid(s));
            sp.starred = (0..50).map(|n| rid(1000 + s * 100 + n)).collect();
            l.spaces.push(sp);
        }
        assert!(l.fits());
        p.write_sidebar(&l).unwrap();
        assert_eq!(p.read_sidebar(&user("@a:x")), SidebarRead::Layout(l));
    }

    #[test]
    fn settings_json_keeps_its_own_64_kib_cap_next_to_a_sidebar_file() {
        let (_t, p) = sidebar_paths();
        p.write_sidebar(&full_layout("@a:x")).unwrap();
        let mut big = br#"{"theme":"Dark","pad":""#.to_vec();
        big.resize(MAX_SETTINGS_BYTES as usize + 10, b'a');
        big.extend_from_slice(b"\"}");
        std::fs::write(p.settings_file(), &big).unwrap();
        assert_eq!(p.read_settings(), UiSettings::default());
        assert!(matches!(
            p.read_sidebar(&user("@a:x")),
            SidebarRead::Layout(_)
        ));
    }
}
