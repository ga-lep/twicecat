//! The Settings screen's state and handlers (plan Q17, Task 15): the profile,
//! the notification defaults and keywords, the three local toggles, the
//! theme and the shortcut list.
//!
//! Account settings go to the core as commands. The local settings
//! ([`UiSettings`]) live in this device's `settings.json`: every change is
//! applied at once (notifications, previews, the tray badge and the theme read
//! them live) and written by a task through a [`SettingsStore`], so `update`
//! never touches the disk. Display names and keywords are user content: they
//! are kept as [`MessageText`] and never reach `Debug` or a log.

use std::fmt;
use std::sync::{Arc, Mutex};

use iced::Task;
use twicecat_core::api::{
    Command, DensityChoice, MediaBytes, MessageText, NotificationSettingsView, OwnProfileView,
    PaletteChoice, RoomListChoice, RoomNotificationModeView, ThemeChoice, UiSettings,
};
use twicecat_core::paths::DataPaths;

use crate::blocking;
use crate::media::SavePath;
use crate::rooms::{AvatarRefusal, AvatarUpload, has_unsafe_char, read_avatar_file};
use crate::sessions::SessionsMessage;
use crate::state::{App, Message, Screen};
use crate::text_safety::one_line_capped;

/// Widget ID of the display name field.
pub const NAME_INPUT_ID: &str = "settings-name-input";
/// Widget ID of the keyword field.
pub const KEYWORD_INPUT_ID: &str = "settings-keyword-input";

/// Longest display name, in characters (the core's limit too).
pub const MAX_NAME_CHARS: usize = 255;
/// Longest keyword, in characters (the core's limit too).
pub const KEYWORD_MAX_CHARS: usize = 64;
/// Most keywords an account may have (the core's limit too).
pub const MAX_KEYWORDS: usize = 50;
/// How many events that disagree with a mode the user just picked are taken
/// for late echoes of an earlier setting before the UI believes them.
pub const MAX_STALE_EVENTS: u8 = 3;

/// The Voice & audio section of Settings.
pub const VOICE_AUDIO_LINE: &str = "Devices, voice activity or push-to-talk (a key or a gamepad button), microphone processing, volume, the mic test, the ringtone, and cleaning other voices.";
/// The note under the shortcut list.
pub const DRAG_AND_DROP_NOTE: &str = "Dragging files into the window does not work on a native Wayland window (an upstream limit). Use Attach (Ctrl+U) or paste instead.";

/// Why a field is not sent, or a change did not happen. The words are fixed;
/// they never repeat the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    /// The display name is empty.
    NameEmpty,
    /// The display name is over 255 characters.
    NameTooLong,
    /// The display name has control or direction characters.
    NameUnsafe,
    /// The keyword is over 64 characters.
    KeywordTooLong,
    /// The keyword has control or direction characters.
    KeywordUnsafe,
    /// The account has 50 keywords already.
    TooManyKeywords,
    /// The picture was refused.
    Picture(AvatarRefusal),
    /// The core could not change the profile.
    ProfileFailed,
    /// The core could not change a notification setting.
    NotificationFailed,
    /// The settings file could not be written.
    SaveFailed,
}

impl SettingsError {
    /// The line under the control.
    pub fn text(self) -> &'static str {
        match self {
            Self::NameEmpty => "Enter a display name.",
            Self::NameTooLong => "A display name has at most 255 characters.",
            Self::NameUnsafe => "A display name cannot contain control characters.",
            Self::KeywordTooLong => "A keyword has at most 64 characters.",
            Self::KeywordUnsafe => "A keyword cannot contain control characters.",
            Self::TooManyKeywords => "You have the most keywords the account can hold (50).",
            Self::Picture(refusal) => refusal.text(),
            Self::ProfileFailed => "Your profile could not be changed.",
            Self::NotificationFailed => "The notification setting could not be changed.",
            Self::SaveFailed => "The settings could not be saved on this device.",
        }
    }
}

/// A display name: trimmed, 1 to 255 characters, no control or direction
/// characters.
pub fn check_display_name(raw: &str) -> Result<String, SettingsError> {
    let name = raw.trim();
    let count = name.chars().count();
    if count == 0 {
        return Err(SettingsError::NameEmpty);
    }
    if count > MAX_NAME_CHARS {
        return Err(SettingsError::NameTooLong);
    }
    if has_unsafe_char(name, false) {
        return Err(SettingsError::NameUnsafe);
    }
    Ok(name.to_owned())
}

/// A keyword typed in the field. `Ok(None)` for a blank field (nothing to do),
/// `Ok(Some(keyword))` for one to send. `existing` is how many keywords the
/// account has and `already_there` whether this one is among them (adding it
/// again changes nothing, so it is allowed at the limit).
pub fn check_keyword(
    raw: &str,
    existing: usize,
    already_there: bool,
) -> Result<Option<String>, SettingsError> {
    let keyword = raw.trim();
    if keyword.is_empty() {
        return Ok(None);
    }
    if keyword.chars().count() > KEYWORD_MAX_CHARS {
        return Err(SettingsError::KeywordTooLong);
    }
    if has_unsafe_char(keyword, false) {
        return Err(SettingsError::KeywordUnsafe);
    }
    if !already_there && existing >= MAX_KEYWORDS {
        return Err(SettingsError::TooManyKeywords);
    }
    Ok(Some(keyword.to_owned()))
}

/// A choice in a default-mode menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultChoice {
    /// Every message notifies.
    AllMessages,
    /// Only mentions and keywords notify.
    MentionsAndKeywords,
    /// Nothing notifies.
    Mute,
}

impl DefaultChoice {
    /// The choices, in menu order.
    pub const ALL: [Self; 3] = [Self::AllMessages, Self::MentionsAndKeywords, Self::Mute];

    /// The choice for a mode.
    pub fn from_mode(mode: RoomNotificationModeView) -> Self {
        match mode {
            RoomNotificationModeView::AllMessages => Self::AllMessages,
            RoomNotificationModeView::MentionsAndKeywords => Self::MentionsAndKeywords,
            RoomNotificationModeView::Mute => Self::Mute,
        }
    }

    /// The mode the choice stands for.
    pub fn mode(self) -> RoomNotificationModeView {
        match self {
            Self::AllMessages => RoomNotificationModeView::AllMessages,
            Self::MentionsAndKeywords => RoomNotificationModeView::MentionsAndKeywords,
            Self::Mute => RoomNotificationModeView::Mute,
        }
    }
}

impl fmt::Display for DefaultChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AllMessages => "All messages",
            Self::MentionsAndKeywords => "Mentions and keywords",
            Self::Mute => "Nothing",
        })
    }
}

/// A default mode the user picked that the core's events have not confirmed.
///
/// The SDK replaces its whole push-rule cache on every push-rules echo from
/// sync, so a late echo of an earlier setting can follow the event of the
/// latest one. The first event after a change is therefore not final: the
/// picked mode is shown until an event agrees with it, and a disagreeing
/// event counts as a late echo for up to [`MAX_STALE_EVENTS`] events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingMode {
    /// What the user picked.
    pub mode: RoomNotificationModeView,
    /// Events seen that carried another mode.
    pub stale: u8,
}

impl PendingMode {
    fn new(mode: RoomNotificationModeView) -> Self {
        Self { mode, stale: 0 }
    }

    /// Takes one event's mode into account; `None` once the pick is settled.
    fn observe(self, reported: RoomNotificationModeView) -> Option<Self> {
        if reported == self.mode {
            return None;
        }
        let stale = self.stale + 1;
        (stale < MAX_STALE_EVENTS).then_some(Self { stale, ..self })
    }
}

/// What the app keeps of the user's own profile.
#[derive(Clone, PartialEq, Eq)]
pub struct ProfileSummary {
    /// The display name, when set.
    pub display_name: Option<String>,
    /// Whether a picture is set.
    pub has_avatar: bool,
}

// Manual impl: the display name is user content.
impl fmt::Debug for ProfileSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileSummary")
            .field("has_avatar", &self.has_avatar)
            .finish_non_exhaustive()
    }
}

/// The Settings screen's state.
pub struct SettingsModel {
    /// The screen is open.
    pub open: bool,
    /// The display name field.
    pub name: MessageText,
    /// The field was edited since the profile last filled it.
    pub name_edited: bool,
    /// The profile the core last reported.
    pub profile: Option<ProfileSummary>,
    /// The picture is being chosen or read.
    pub avatar_busy: bool,
    /// A profile command was sent and not answered yet.
    pub profile_pending: bool,
    /// The keyword field.
    pub keyword_input: MessageText,
    /// The default mode of group rooms the user just picked.
    pub pending_group: Option<PendingMode>,
    /// The default mode of direct messages the user just picked.
    pub pending_dm: Option<PendingMode>,
    /// The line shown under the sections.
    pub error: Option<SettingsError>,
}

impl Default for SettingsModel {
    fn default() -> Self {
        Self {
            open: false,
            name: MessageText::from(""),
            name_edited: false,
            profile: None,
            avatar_busy: false,
            profile_pending: false,
            keyword_input: MessageText::from(""),
            pending_group: None,
            pending_dm: None,
            error: None,
        }
    }
}

// Manual impl: the name and keyword fields stay out of `Debug`.
impl fmt::Debug for SettingsModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsModel")
            .field("open", &self.open)
            .field("name_edited", &self.name_edited)
            .field("profile", &self.profile)
            .field("avatar_busy", &self.avatar_busy)
            .field("profile_pending", &self.profile_pending)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl SettingsModel {
    /// Takes a profile report: it fills the name field unless the user is
    /// editing it, and ends the wait for an answer.
    pub fn set_profile(&mut self, view: &OwnProfileView) {
        self.profile_pending = false;
        if !self.name_edited {
            self.name = MessageText::from(view.display_name.clone().unwrap_or_default());
        }
        self.profile = Some(ProfileSummary {
            display_name: view.display_name.clone(),
            has_avatar: view.avatar.is_some(),
        });
    }

    /// Takes the notification settings the core reported into account.
    pub fn observe(&mut self, settings: &NotificationSettingsView) {
        self.pending_group = self
            .pending_group
            .and_then(|p| p.observe(settings.group_default));
        self.pending_dm = self.pending_dm.and_then(|p| p.observe(settings.dm_default));
    }

    /// The default mode to show: the user's pick until the core confirms it,
    /// else what the core reported.
    pub fn shown_default(
        &self,
        settings: Option<&NotificationSettingsView>,
        one_to_one: bool,
    ) -> Option<RoomNotificationModeView> {
        let (pending, reported) = if one_to_one {
            (self.pending_dm, settings.map(|s| s.dm_default))
        } else {
            (self.pending_group, settings.map(|s| s.group_default))
        };
        pending.map(|p| p.mode).or(reported)
    }
}

/// Where the local settings are written. The real store writes
/// `settings.json`; tests use a fake.
pub trait SettingsStore: Send {
    /// Writes `settings`. The task yields whether it worked.
    fn save(&self, settings: UiSettings) -> Task<bool>;
}

/// A store that writes nothing; `App::new` installs it so tests never touch
/// the disk.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSettingsStore;

impl SettingsStore for NoSettingsStore {
    fn save(&self, _settings: UiSettings) -> Task<bool> {
        Task::none()
    }
}

/// The settings file in the data root.
///
/// Saves run on [`blocking`] threads, which can overtake one another: each
/// job writes the newest settings handed to the store, under one lock, so the
/// file always ends up with the last change.
#[derive(Debug, Clone)]
pub struct DiskSettings {
    paths: DataPaths,
    latest: Arc<Mutex<UiSettings>>,
    writing: Arc<Mutex<()>>,
}

impl DiskSettings {
    /// A store for the settings file under `paths`.
    pub fn new(paths: DataPaths) -> Self {
        Self {
            paths,
            latest: Arc::new(Mutex::new(UiSettings::default())),
            writing: Arc::new(Mutex::new(())),
        }
    }
}

fn unpoisoned<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SettingsStore for DiskSettings {
    fn save(&self, settings: UiSettings) -> Task<bool> {
        *unpoisoned(self.latest.lock()) = settings;
        let (paths, latest, writing) = (
            self.paths.clone(),
            self.latest.clone(),
            self.writing.clone(),
        );
        Task::future(async move {
            blocking::run(move || {
                let _guard = unpoisoned(writing.lock());
                let newest = *unpoisoned(latest.lock());
                paths.write_settings(&newest).is_ok()
            })
            .await
            .unwrap_or(false)
        })
    }
}

/// Least text scale, in percent.
pub const MIN_TEXT_SCALE: u16 = 80;
/// Largest text scale, in percent.
pub const MAX_TEXT_SCALE: u16 = 200;
/// The step of the zoom buttons and keys, in percent.
pub const TEXT_SCALE_STEP: u16 = 10;
/// The text scale of a fresh install, in percent.
pub const DEFAULT_TEXT_SCALE: u16 = 100;

/// A change of the text scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    /// One step larger (Ctrl+=).
    In,
    /// One step smaller (Ctrl+-).
    Out,
    /// Back to 100 % (Ctrl+0).
    Reset,
}

impl Zoom {
    /// The scale after this change, kept within the limits.
    pub fn apply(self, percent: u16) -> u16 {
        match self {
            Self::In => (percent + TEXT_SCALE_STEP).min(MAX_TEXT_SCALE),
            Self::Out => percent.saturating_sub(TEXT_SCALE_STEP).max(MIN_TEXT_SCALE),
            Self::Reset => DEFAULT_TEXT_SCALE,
        }
    }
}

/// A density as a pick-list entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DensityOption(pub DensityChoice);

impl DensityOption {
    /// Both densities, in menu order.
    pub const ALL: [Self; 2] = [
        Self(DensityChoice::Comfortable),
        Self(DensityChoice::Compact),
    ];
}

impl fmt::Display for DensityOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            DensityChoice::Comfortable => "Comfortable",
            DensityChoice::Compact => "Compact",
        })
    }
}

/// A room list layout as a pick-list entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomListOption(pub RoomListChoice);

impl RoomListOption {
    /// Both layouts, in menu order.
    pub const ALL: [Self; 2] = [Self(RoomListChoice::Expanded), Self(RoomListChoice::Rail)];
}

impl fmt::Display for RoomListOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            RoomListChoice::Expanded => "Expanded",
            RoomListChoice::Rail => "Collapsed to a rail",
        })
    }
}

/// A message of the Settings screen.
#[derive(Debug, Clone)]
pub enum SettingsMessage {
    /// The account area's **Settings** button, or Ctrl+,.
    Open,
    /// **Close**, Esc.
    Close,
    /// **Manage sessions**.
    OpenSessions,
    /// **Voice & audio** (plan Q5): closes Settings and opens that card.
    OpenVoiceAudio,
    /// The display name field changed.
    NameInput(MessageText),
    /// **Save** next to the display name.
    SaveName,
    /// **Change picture**.
    ChooseAvatar,
    /// The picture dialog answered; `None` when cancelled.
    AvatarPicked(Option<SavePath>),
    /// The picture was read and scaled off the UI thread.
    AvatarReady(Result<Box<AvatarUpload>, AvatarRefusal>),
    /// **Remove picture**.
    RemoveAvatar,
    /// A default mode menu changed.
    DefaultMode {
        /// Direct messages (true) or group rooms (false).
        one_to_one: bool,
        /// The mode picked.
        mode: RoomNotificationModeView,
    },
    /// The keyword field changed.
    KeywordInput(MessageText),
    /// **Add** next to the keyword field, or Enter in it.
    AddKeyword,
    /// **Remove** on a keyword.
    RemoveKeyword(MessageText),
    /// The desktop notifications toggle.
    DesktopNotifications(bool),
    /// The show message content toggle.
    ShowMessageContent(bool),
    /// The tray badge toggle.
    TrayBadge(bool),
    /// A theme choice.
    Theme(ThemeChoice),
    /// A colour set; `None` is the app's default.
    Palette(Option<PaletteChoice>),
    /// A density choice.
    Density(DensityChoice),
    /// A room list layout choice.
    RoomList(RoomListChoice),
    /// A change of the text scale (buttons and Ctrl+=, Ctrl+-, Ctrl+0).
    Zoom(Zoom),
    /// The settings file was written (`true`) or not.
    Saved(bool),
}

impl App {
    /// Whether another modal screen or dialog is open, so Settings stays shut.
    fn other_modal_open(&self) -> bool {
        self.sessions.open
            || self.verification.is_some()
            || self.calls.card().is_some()
            || self.audio_ui.open
            || self.uploads.dialog.is_some()
            || self.media.viewer.is_some()
            || self.rooms_ui.overlay_open()
            || self.security.visible() != crate::security::Visible::Nothing
    }

    /// Whether the Settings screen covers the window.
    pub fn settings_open(&self) -> bool {
        self.settings_ui.open
    }

    /// Takes the core's report of the user's own profile.
    pub(crate) fn on_own_profile(&mut self, view: &OwnProfileView) {
        self.settings_ui.set_profile(view);
    }

    /// The core could not change the profile.
    pub(crate) fn on_profile_failed(&mut self) {
        let ui = &mut self.settings_ui;
        ui.profile_pending = false;
        ui.avatar_busy = false;
        ui.error = Some(SettingsError::ProfileFailed);
    }

    /// The core could not change an account-wide notification setting: show
    /// what it reports again.
    pub(crate) fn on_account_notification_failed(&mut self) {
        let ui = &mut self.settings_ui;
        ui.pending_group = None;
        ui.pending_dm = None;
        ui.error = Some(SettingsError::NotificationFailed);
    }

    /// The core's notification settings changed.
    pub(crate) fn on_notification_settings(&mut self, settings: NotificationSettingsView) {
        self.settings_ui.observe(&settings);
        self.notification_settings = Some(settings);
    }

    pub(crate) fn on_settings(&mut self, message: SettingsMessage) -> Task<Message> {
        use SettingsMessage as M;
        match message {
            M::Open => self.open_settings_screen(),
            M::Close => {
                self.settings_ui.open = false;
                self.settings_ui.error = None;
                Task::none()
            }
            M::OpenVoiceAudio => {
                if !self.settings_ui.open {
                    return Task::none();
                }
                self.settings_ui.open = false;
                self.settings_ui.error = None;
                self.on_audio(crate::audio_settings::AudioMessage::Open)
            }
            M::OpenSessions => {
                if !self.settings_ui.open {
                    return Task::none();
                }
                self.settings_ui.open = false;
                self.settings_ui.error = None;
                self.on_sessions(SessionsMessage::Open)
            }
            M::NameInput(name) => {
                let ui = &mut self.settings_ui;
                ui.name = name;
                ui.name_edited = true;
                ui.error = None;
                Task::none()
            }
            M::SaveName => self.save_display_name(),
            M::ChooseAvatar => self.choose_own_avatar(),
            M::AvatarPicked(path) => self.own_avatar_picked(path),
            M::AvatarReady(result) => self.own_avatar_ready(result),
            M::RemoveAvatar => {
                let ui = &mut self.settings_ui;
                let has_avatar = ui.profile.as_ref().is_some_and(|p| p.has_avatar);
                if !has_avatar || ui.avatar_busy || ui.profile_pending {
                    return Task::none();
                }
                ui.profile_pending = true;
                ui.error = None;
                self.send(Command::RemoveAvatar)
            }
            M::DefaultMode { one_to_one, mode } => self.choose_default(one_to_one, mode),
            M::KeywordInput(input) => {
                let ui = &mut self.settings_ui;
                ui.keyword_input = input;
                ui.error = None;
                Task::none()
            }
            M::AddKeyword => self.add_keyword(),
            M::RemoveKeyword(keyword) => {
                let known = self
                    .notification_settings
                    .as_ref()
                    .is_some_and(|s| s.keywords.iter().any(|k| k == keyword.expose()));
                if !known {
                    return Task::none();
                }
                self.send(Command::RemoveKeyword {
                    keyword: keyword.expose().to_owned(),
                })
            }
            M::DesktopNotifications(on) => self.change_local(|s| &mut s.desktop_notifications, on),
            M::ShowMessageContent(on) => self.change_local(|s| &mut s.show_message_content, on),
            M::TrayBadge(on) => {
                let task = self.change_local(|s| &mut s.tray_badge, on);
                // The icon follows the toggle now, not at the next unread
                // count.
                self.tray.refresh(self.ui_settings.tray_badge);
                task
            }
            M::Theme(choice) => {
                if self.ui_settings.theme == choice {
                    return Task::none();
                }
                self.ui_settings.theme = choice;
                self.save_local()
            }
            M::Palette(choice) => {
                if self.ui_settings.palette == choice {
                    return Task::none();
                }
                self.ui_settings.palette = choice;
                self.save_local()
            }
            M::Density(choice) => {
                if self.ui_settings.density == choice {
                    return Task::none();
                }
                let before = self.room_row_height();
                self.ui_settings.density = choice;
                let rows = self.keep_list_place(before);
                // Rows change height: the measured ones are stale. The place
                // in the timeline (the offset from the bottom) stays.
                self.timeline.reset_measurements();
                if let Some(panel) = self.threads.panel.as_mut() {
                    panel.timeline.reset_measurements();
                }
                Task::batch([rows, self.save_local()])
            }
            M::RoomList(choice) => {
                if self.ui_settings.room_list == choice {
                    return Task::none();
                }
                let before = self.room_row_height();
                self.ui_settings.room_list = choice;
                let rows = self.keep_list_place(before);
                Task::batch([rows, self.save_local()])
            }
            M::Zoom(zoom) => {
                let percent = zoom.apply(self.ui_settings.text_scale_percent);
                if percent == self.ui_settings.text_scale_percent {
                    return Task::none();
                }
                self.ui_settings.text_scale_percent = percent;
                self.save_local()
            }
            M::Saved(ok) => {
                if !ok {
                    self.settings_ui.error = Some(SettingsError::SaveFailed);
                }
                Task::none()
            }
        }
    }

    fn open_settings_screen(&mut self) -> Task<Message> {
        if self.screen != Screen::Main || self.settings_ui.open || self.other_modal_open() {
            return Task::none();
        }
        let ui = &mut self.settings_ui;
        ui.open = true;
        ui.error = None;
        // Keyboard focus moves off the composer, under the screen.
        iced::widget::operation::focus(NAME_INPUT_ID)
    }

    fn save_display_name(&mut self) -> Task<Message> {
        let ui = &mut self.settings_ui;
        if !ui.name_edited || ui.profile_pending {
            return Task::none();
        }
        match check_display_name(ui.name.expose()) {
            Ok(name) => {
                ui.error = None;
                ui.name_edited = false;
                let unchanged = ui
                    .profile
                    .as_ref()
                    .is_some_and(|p| p.display_name.as_deref() == Some(name.as_str()));
                if unchanged {
                    return Task::none();
                }
                ui.profile_pending = true;
                self.send(Command::SetDisplayName { name })
            }
            Err(error) => {
                ui.error = Some(error);
                Task::none()
            }
        }
    }

    fn choose_own_avatar(&mut self) -> Task<Message> {
        let ui = &mut self.settings_ui;
        if ui.avatar_busy || ui.profile_pending {
            return Task::none();
        }
        ui.avatar_busy = true;
        ui.error = None;
        self.dialogs
            .pick_image()
            .map(|path| Message::Settings(SettingsMessage::AvatarPicked(path.map(SavePath::from))))
    }

    /// The dialog answered: read and scale the picture off the UI thread.
    fn own_avatar_picked(&mut self, path: Option<SavePath>) -> Task<Message> {
        let ui = &mut self.settings_ui;
        if !ui.avatar_busy {
            return Task::none();
        }
        let Some(path) = path else {
            ui.avatar_busy = false;
            return Task::none();
        };
        let path = path.as_path().to_path_buf();
        Task::future(async move {
            let result = match blocking::run(move || read_avatar_file(&path)).await {
                Some(Ok(upload)) => Ok(Box::new(upload)),
                Some(Err(refusal)) => Err(refusal),
                None => Err(AvatarRefusal::Unreadable),
            };
            Message::Settings(SettingsMessage::AvatarReady(result))
        })
    }

    fn own_avatar_ready(
        &mut self,
        result: Result<Box<AvatarUpload>, AvatarRefusal>,
    ) -> Task<Message> {
        let ui = &mut self.settings_ui;
        if !ui.avatar_busy {
            return Task::none();
        }
        ui.avatar_busy = false;
        match result {
            Ok(upload) => {
                ui.profile_pending = true;
                let upload = *upload;
                self.send(Command::SetAvatar {
                    data: MediaBytes::from(upload.data),
                    mimetype: upload.mimetype.to_owned(),
                })
            }
            Err(refusal) => {
                ui.error = Some(SettingsError::Picture(refusal));
                Task::none()
            }
        }
    }

    fn choose_default(
        &mut self,
        one_to_one: bool,
        mode: RoomNotificationModeView,
    ) -> Task<Message> {
        let shown = self
            .settings_ui
            .shown_default(self.notification_settings.as_ref(), one_to_one);
        if shown == Some(mode) {
            return Task::none();
        }
        let ui = &mut self.settings_ui;
        ui.error = None;
        let pending = Some(PendingMode::new(mode));
        if one_to_one {
            ui.pending_dm = pending;
        } else {
            ui.pending_group = pending;
        }
        self.send(Command::SetDefaultNotificationMode { one_to_one, mode })
    }

    fn add_keyword(&mut self) -> Task<Message> {
        let (existing, already) = {
            let keywords = self
                .notification_settings
                .as_ref()
                .map(|s| s.keywords.as_slice())
                .unwrap_or_default();
            let typed = self.settings_ui.keyword_input.expose().trim();
            (keywords.len(), keywords.iter().any(|k| k == typed))
        };
        let ui = &mut self.settings_ui;
        match check_keyword(ui.keyword_input.expose(), existing, already) {
            Ok(None) => Task::none(),
            Ok(Some(keyword)) => {
                ui.keyword_input = MessageText::from("");
                ui.error = None;
                if already {
                    return Task::none();
                }
                self.send(Command::AddKeyword { keyword })
            }
            Err(error) => {
                ui.error = Some(error);
                Task::none()
            }
        }
    }

    /// Sets one local flag; nothing is written when it already has the value.
    fn change_local(
        &mut self,
        field: impl FnOnce(&mut UiSettings) -> &mut bool,
        on: bool,
    ) -> Task<Message> {
        let slot = field(&mut self.ui_settings);
        if *slot == on {
            return Task::none();
        }
        *slot = on;
        self.save_local()
    }

    /// After the room list's row height changed from `before`: keeps the
    /// first visible room at the top by scaling the offset, moves the
    /// scrollable there (it only clamps its own offset), and asks for what
    /// the new window needs (avatars, paging).
    fn keep_list_place(&mut self, before: f32) -> Task<Message> {
        let after = self.room_row_height();
        if before <= 0.0 || (after - before).abs() < f32::EPSILON {
            return Task::none();
        }
        self.list_offset_y *= after / before;
        Task::batch([
            iced::widget::operation::scroll_to(
                crate::screens::room_list_pane::LIST_SCROLL_ID,
                iced::widget::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(self.list_offset_y),
                },
            ),
            self.after_list_change(),
        ])
    }

    /// The factor the whole window is scaled by (the text scale).
    pub fn scale_factor(&self) -> f32 {
        f32::from(self.ui_settings.text_scale_percent) / 100.0
    }

    /// Writes the local settings through the store, in a task.
    fn save_local(&mut self) -> Task<Message> {
        self.settings_ui.error = None;
        self.settings_store
            .save(self.ui_settings)
            .map(|ok| Message::Settings(SettingsMessage::Saved(ok)))
    }

    /// The keywords to draw: the account's, each as one capped line, with the
    /// text to send back on **Remove**.
    pub fn keyword_rows(&self) -> Vec<(String, MessageText)> {
        self.notification_settings
            .as_ref()
            .map(|s| {
                s.keywords
                    .iter()
                    .take(MAX_KEYWORDS)
                    .map(|k| {
                        (
                            one_line_capped(k, KEYWORD_MAX_CHARS),
                            MessageText::from(k.clone()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
pub use fake::RecordingSettings;

#[cfg(test)]
mod fake {
    use super::*;

    /// A fake store that records every write and writes nothing.
    #[derive(Debug, Clone, Default)]
    pub struct RecordingSettings {
        /// The settings handed to the store so far.
        pub saved: Arc<Mutex<Vec<UiSettings>>>,
    }

    impl SettingsStore for RecordingSettings {
        fn save(&self, settings: UiSettings) -> Task<bool> {
            self.saved.lock().expect("test mutex").push(settings);
            Task::none()
        }
    }
}
