//! The update check in the app (Phase 7 Task 9, plan Q7 a): the question
//! asked once, the toggle on Settings → About, the banner when a newer
//! version is out, and the link to its release page. The core asks GitHub
//! once a day while the check is on (`Command::ScheduleUpdateChecks`);
//! nothing is downloaded or run. A Flatpak or a package from the
//! distribution is updated by Flatpak or the package manager: the app never
//! asks there.

use iced::Task;
use twicecat_core::api::{Command, CoreEvent, UpdateCheckChoice, UpdateMechanism};

use crate::links;
use crate::state::{App, Message};

/// The question, asked once (in the banner slot) where the app checks
/// itself.
pub const QUESTION: &str = "Check once a day whether a newer Twicecat is out? Twicecat asks GitHub's list of releases; nothing is downloaded or installed.";
/// The toggle on Settings → About.
pub const TOGGLE: &str = "Check for a newer version once a day";
/// Under the toggle.
pub const TOGGLE_NOTE: &str =
    "Twicecat asks GitHub's list of releases and shows a link; nothing is downloaded or installed.";
/// On Settings → About in the Flatpak.
pub const FLATPAK_NOTE: &str = "Flatpak keeps this copy of Twicecat up to date.";
/// On Settings → About for a package from the distribution.
pub const PACKAGE_NOTE: &str = "Your package manager keeps this copy of Twicecat up to date.";

/// What the app knows about updates.
#[derive(Debug)]
pub struct UpdateUi {
    /// How this copy is updated (detected at start).
    pub mechanism: UpdateMechanism,
    /// A newer release the core found, until dismissed.
    pub available: Option<Available>,
}

impl Default for UpdateUi {
    fn default() -> Self {
        Self {
            mechanism: UpdateMechanism::CheckAndLink,
            available: None,
        }
    }
}

/// A newer release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    /// Its version, such as `0.2.0`.
    pub version: String,
    /// Its page on github.com (checked by the core).
    pub url: String,
}

impl Available {
    /// The banner's words.
    pub fn text(&self) -> String {
        format!("Twicecat {} is available.", self.version)
    }
}

/// What the update UI asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateMessage {
    /// The answer to the question, or the toggle on Settings → About.
    Choose(bool),
    /// Open the release page of the newer version.
    OpenRelease,
    /// Hide the banner until the next start.
    Dismiss,
}

impl App {
    /// The question shows where the app checks itself and it was never
    /// answered.
    pub fn update_question_shown(&self) -> bool {
        self.updates.mechanism.checks_itself()
            && self.ui_settings.update_check == UpdateCheckChoice::Unasked
    }

    /// At start: the daily checks, when they are on here.
    pub fn start_update_checks(&self) -> Task<Message> {
        if self.updates.mechanism.checks_itself()
            && self.ui_settings.update_check == UpdateCheckChoice::On
        {
            self.send(Command::ScheduleUpdateChecks {
                last_checked_at: self.ui_settings.update_checked_at,
            })
        } else {
            Task::none()
        }
    }

    pub(crate) fn on_update(&mut self, message: UpdateMessage) -> Task<Message> {
        match message {
            UpdateMessage::Choose(on) => {
                if !self.updates.mechanism.checks_itself() {
                    return Task::none();
                }
                let choice = if on {
                    UpdateCheckChoice::On
                } else {
                    UpdateCheckChoice::Off
                };
                if self.ui_settings.update_check == choice {
                    return Task::none();
                }
                self.ui_settings.update_check = choice;
                let command = if on {
                    Command::ScheduleUpdateChecks {
                        last_checked_at: self.ui_settings.update_checked_at,
                    }
                } else {
                    self.updates.available = None;
                    Command::StopUpdateChecks
                };
                Task::batch([self.save_local(), self.send(command)])
            }
            UpdateMessage::OpenRelease => match &self.updates.available {
                Some(available) => match links::check_link_url(&available.url) {
                    Ok(url) => crate::state::open_link_task(url),
                    Err(_) => Task::none(),
                },
                None => Task::none(),
            },
            UpdateMessage::Dismiss => {
                self.updates.available = None;
                Task::none()
            }
        }
    }

    /// `UpdateAvailable`, `UpToDate` and `UpdateChecked` from the core.
    pub(crate) fn on_update_event(&mut self, event: CoreEvent) -> Task<Message> {
        match event {
            // A late answer after the check was turned off is dropped.
            CoreEvent::UpdateAvailable { version, url }
                if self.ui_settings.update_check == UpdateCheckChoice::On =>
            {
                self.updates.available = Some(Available { version, url });
                Task::none()
            }
            CoreEvent::UpToDate => {
                self.updates.available = None;
                Task::none()
            }
            CoreEvent::UpdateChecked { at } if self.ui_settings.update_checked_at != at => {
                self.ui_settings.update_checked_at = at;
                self.save_local()
            }
            _ => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use twicecat_core::api::{Command, CoreEvent, UpdateCheckChoice, UpdateMechanism};

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::settings::RecordingSettings;
    use crate::state::{App, Message, Startup};

    const PAGE: &str = "https://github.com/ga-lep/twicecat/releases/tag/v9.0.0";

    fn app(mechanism: UpdateMechanism) -> (App, mpsc::Receiver<Command>, RecordingSettings) {
        let (tx, rx) = mpsc::channel(16);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let store = RecordingSettings::default();
        app.set_settings_store(Box::new(store.clone()));
        app.updates.mechanism = mechanism;
        let mut rx = rx;
        // The boot's RestoreSession.
        drain(&mut rx);
        (app, rx, store)
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|c| format!("{c:?}"))
            .collect()
    }

    fn saved(store: &RecordingSettings) -> Vec<(UpdateCheckChoice, u64)> {
        store
            .saved
            .lock()
            .unwrap()
            .iter()
            .map(|s| (s.update_check, s.update_checked_at))
            .collect()
    }

    fn core(app: &mut App, event: CoreEvent) {
        let _ = app.update(Message::Core(Box::new(event)));
    }

    #[test]
    fn the_question_is_asked_once_where_the_app_checks_itself() {
        for mechanism in [UpdateMechanism::CheckAndLink, UpdateMechanism::AppImage] {
            let (mut app, mut rx, store) = app(mechanism);
            assert!(app.update_question_shown());
            let _ = app.update(Message::Update(UpdateMessage::Choose(true)));
            assert!(!app.update_question_shown());
            assert_eq!(saved(&store), [(UpdateCheckChoice::On, 0)]);
            assert_eq!(
                drain(&mut rx),
                ["ScheduleUpdateChecks { last_checked_at: 0 }"]
            );
        }
        let (mut app, mut rx, store) = app(UpdateMechanism::CheckAndLink);
        let _ = app.update(Message::Update(UpdateMessage::Choose(false)));
        assert!(!app.update_question_shown());
        assert_eq!(saved(&store), [(UpdateCheckChoice::Off, 0)]);
        assert_eq!(drain(&mut rx), ["StopUpdateChecks"]);
        // The same answer again changes nothing.
        let _ = app.update(Message::Update(UpdateMessage::Choose(false)));
        assert_eq!(saved(&store).len(), 1);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn flatpak_and_packages_never_ask_or_check() {
        for mechanism in [UpdateMechanism::Flatpak, UpdateMechanism::PackageManager] {
            let (mut app, mut rx, store) = app(mechanism);
            assert!(!app.update_question_shown());
            let _ = app.update(Message::Update(UpdateMessage::Choose(true)));
            app.ui_settings.update_check = UpdateCheckChoice::On;
            let _ = app.start_update_checks();
            assert!(saved(&store).is_empty());
            assert!(drain(&mut rx).is_empty());
        }
    }

    #[test]
    fn checks_start_at_boot_from_the_last_time_only_when_on() {
        let (mut app, mut rx, _store) = app(UpdateMechanism::CheckAndLink);
        app.ui_settings.update_checked_at = 1_800_000_000;
        for choice in [UpdateCheckChoice::Unasked, UpdateCheckChoice::Off] {
            app.ui_settings.update_check = choice;
            let _ = app.start_update_checks();
            assert!(drain(&mut rx).is_empty(), "{choice:?}");
        }
        app.ui_settings.update_check = UpdateCheckChoice::On;
        let _ = app.start_update_checks();
        assert_eq!(
            drain(&mut rx),
            ["ScheduleUpdateChecks { last_checked_at: 1800000000 }"]
        );
    }

    #[test]
    fn a_newer_version_shows_until_dismissed_and_the_time_is_kept() {
        let (mut app, mut rx, store) = app(UpdateMechanism::CheckAndLink);
        let _ = app.update(Message::Update(UpdateMessage::Choose(true)));
        drain(&mut rx);
        core(&mut app, CoreEvent::UpdateChecked { at: 1_800_000_000 });
        assert_eq!(
            saved(&store).last(),
            Some(&(UpdateCheckChoice::On, 1_800_000_000))
        );
        core(
            &mut app,
            CoreEvent::UpdateAvailable {
                version: "9.0.0".into(),
                url: PAGE.into(),
            },
        );
        let available = app.updates.available.clone().expect("the banner");
        assert_eq!(available.text(), "Twicecat 9.0.0 is available.");
        // The page the banner opens passes the app's own link check.
        assert!(crate::links::check_link_url(&available.url).is_ok());
        let _ = app.update(Message::Update(UpdateMessage::OpenRelease));
        assert!(app.updates.available.is_some());
        let _ = app.update(Message::Update(UpdateMessage::Dismiss));
        assert!(app.updates.available.is_none());
        // A later "up to date" (a release was pulled) clears it too.
        core(
            &mut app,
            CoreEvent::UpdateAvailable {
                version: "9.0.0".into(),
                url: PAGE.into(),
            },
        );
        core(&mut app, CoreEvent::UpToDate);
        assert!(app.updates.available.is_none());
    }

    #[test]
    fn turning_the_check_off_hides_the_banner_and_drops_late_answers() {
        let (mut app, mut rx, _store) = app(UpdateMechanism::CheckAndLink);
        let _ = app.update(Message::Update(UpdateMessage::Choose(true)));
        core(
            &mut app,
            CoreEvent::UpdateAvailable {
                version: "9.0.0".into(),
                url: PAGE.into(),
            },
        );
        let _ = app.update(Message::Update(UpdateMessage::Choose(false)));
        assert!(app.updates.available.is_none());
        core(
            &mut app,
            CoreEvent::UpdateAvailable {
                version: "9.0.0".into(),
                url: PAGE.into(),
            },
        );
        assert!(app.updates.available.is_none());
        assert_eq!(
            drain(&mut rx),
            [
                "ScheduleUpdateChecks { last_checked_at: 0 }",
                "StopUpdateChecks"
            ]
        );
    }
}
