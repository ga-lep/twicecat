//! The app's model of the sidebar layout: the logged-in account's arrangement
//! of the spaces rail and of the sections in each space.
//!
//! The model owns no storage. It calls a [`SidebarStore`] that the app hands
//! to each method, and it saves the layout of its own account after every
//! successful [`Op`]. A file the app may not touch (`Newer`, `OtherAccount`)
//! turns customizing off: every operation is then refused and nothing is
//! saved. Room IDs, names and the account never appear in `Debug`.

use std::fmt;

use iced::Task;
use twicecat_core::api::{RailEntry, RoomId, SectionKey, SidebarLayout, SpaceLayout, UserId};

use crate::sidebar_store::{SidebarLoad, SidebarStore};

/// Where the model stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarState {
    /// No layout: logged out, or the load has not answered yet (or could not
    /// run). Customizing is off.
    NotLoaded,
    /// A layout is loaded and can be changed.
    Loaded,
    /// The file was saved by a newer Twicecat. Customizing is off; the file is
    /// never changed.
    Newer,
    /// The file names another account. Customizing is off; the file is never
    /// changed.
    OtherAccount,
    /// The last change was refused because the layout would pass a limit
    /// ("The sidebar layout is full"). Other changes are still tried.
    Full,
}

/// A change of the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Opens or closes the folder with this ID.
    ToggleFolder(u16),
    /// Collapses or expands a section of a space.
    ToggleSection {
        /// The space.
        space: RoomId,
        /// The section.
        key: SectionKey,
    },
}

/// Why an [`Op`] (or a reset) was not done. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// No layout is loaded.
    NotLoaded,
    /// The stored file is `Newer` or names `OtherAccount`: it is not touched.
    Protected,
    /// The layout would pass a limit.
    Full,
    /// The folder or section the change names does not exist.
    Unknown,
}

/// The logged-in account's sidebar layout.
#[derive(Clone, PartialEq, Eq)]
pub struct SidebarModel {
    account: Option<UserId>,
    layout: Option<SidebarLayout>,
    state: SidebarState,
}

impl Default for SidebarModel {
    fn default() -> Self {
        Self {
            account: None,
            layout: None,
            state: SidebarState::NotLoaded,
        }
    }
}

impl fmt::Debug for SidebarModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SidebarModel")
            .field("state", &self.state)
            .field("account", &self.account.as_ref().map(|_| "<redacted>"))
            // The layout's own `Debug` shows counts only.
            .field("layout", &self.layout)
            .finish()
    }
}

impl SidebarModel {
    /// Where the model stands.
    pub fn state(&self) -> SidebarState {
        self.state
    }

    /// The loaded layout, if any.
    pub fn layout(&self) -> Option<&SidebarLayout> {
        self.layout.as_ref()
    }

    /// Starts loading `account`'s layout. Until the answer comes back
    /// ([`Self::loaded`]) the model has no layout and refuses every change.
    pub fn load_for(&mut self, account: UserId, store: &dyn SidebarStore) -> Task<SidebarLoad> {
        let task = store.load(&account);
        self.account = Some(account);
        self.layout = None;
        self.state = SidebarState::NotLoaded;
        task
    }

    /// Takes the answer of a load. An answer for any other account than the
    /// one the model is loading for (a late one) is ignored, and so is a
    /// second answer for an account that is already loaded.
    pub fn loaded(&mut self, account: &UserId, load: SidebarLoad) {
        if self.account.as_ref() != Some(account) || self.state != SidebarState::NotLoaded {
            return;
        }
        match load {
            SidebarLoad::Layout(layout) if layout.account == *account => {
                self.layout = Some(layout);
                self.state = SidebarState::Loaded;
            }
            // A layout of another account must not get here; treat it as no
            // answer rather than showing it.
            SidebarLoad::Layout(_) | SidebarLoad::Unavailable => {}
            SidebarLoad::Default => {
                self.layout = Some(SidebarLayout::new(account.clone()));
                self.state = SidebarState::Loaded;
            }
            SidebarLoad::Newer => self.state = SidebarState::Newer,
            SidebarLoad::OtherAccount => self.state = SidebarState::OtherAccount,
        }
    }

    /// Forgets the layout and the account. Calls no store: nothing on disk
    /// changes at logout.
    pub fn unload(&mut self) {
        *self = Self::default();
    }

    /// Whether changes are allowed, and if not, why.
    fn check_open(&self) -> Result<(), Refused> {
        match self.state {
            SidebarState::Loaded | SidebarState::Full => Ok(()),
            SidebarState::NotLoaded => Err(Refused::NotLoaded),
            SidebarState::Newer | SidebarState::OtherAccount => Err(Refused::Protected),
        }
    }

    /// Does `op` and saves the new layout for the model's own account. The
    /// task yields whether the save worked.
    pub fn apply(&mut self, op: Op, store: &dyn SidebarStore) -> Result<Task<bool>, Refused> {
        self.check_open()?;
        let (Some(account), Some(current)) = (self.account.clone(), self.layout.as_ref()) else {
            return Err(Refused::NotLoaded);
        };
        let mut next = current.clone();
        change(&mut next, op)?;
        if !next.fits() {
            self.state = SidebarState::Full;
            return Err(Refused::Full);
        }
        let task = store.save(&account, next.clone());
        self.layout = Some(next);
        self.state = SidebarState::Loaded;
        Ok(task)
    }

    /// Removes the account's file and goes back to the default layout (the
    /// Reset button, Task 13). Refused for a protected file.
    pub fn reset(&mut self, store: &dyn SidebarStore) -> Result<Task<bool>, Refused> {
        self.check_open()?;
        let Some(account) = self.account.clone() else {
            return Err(Refused::NotLoaded);
        };
        let task = store.reset(&account);
        self.layout = Some(SidebarLayout::new(account));
        self.state = SidebarState::Loaded;
        Ok(task)
    }
}

/// Does `op` on `layout`. The caller checks the limits afterwards.
fn change(layout: &mut SidebarLayout, op: Op) -> Result<(), Refused> {
    match op {
        Op::ToggleFolder(id) => {
            let folder = layout.rail.iter_mut().find_map(|entry| match entry {
                RailEntry::Folder(folder) if folder.id == id => Some(folder),
                _ => None,
            });
            let folder = folder.ok_or(Refused::Unknown)?;
            folder.open = !folder.open;
        }
        Op::ToggleSection { space, key } => {
            let at = match layout.spaces.iter().position(|s| s.space == space) {
                Some(at) => at,
                None => {
                    layout.spaces.push(SpaceLayout::new(space));
                    layout.spaces.len() - 1
                }
            };
            let collapsed = &mut layout.spaces[at].collapsed;
            match collapsed.iter().position(|k| *k == key) {
                Some(n) => {
                    collapsed.remove(n);
                }
                None => collapsed.push(key),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::FolderLayout;

    use super::*;
    use crate::sidebar_store::{Call, MemorySidebar};

    fn user(name: &str) -> UserId {
        UserId::from(name)
    }

    fn rid(name: &str) -> RoomId {
        RoomId::from(name)
    }

    fn folder_layout(account: &UserId) -> SidebarLayout {
        let mut layout = SidebarLayout::new(account.clone());
        layout.rail = vec![
            RailEntry::Space(rid("!solo:x")),
            RailEntry::Folder(FolderLayout {
                id: 1,
                name: None,
                open: false,
                spaces: vec![rid("!in1:x"), rid("!in2:x")],
            }),
        ];
        layout
    }

    fn loaded_model(store: &MemorySidebar, layout: SidebarLayout) -> SidebarModel {
        let account = layout.account.clone();
        let mut model = SidebarModel::default();
        let _ = model.load_for(account.clone(), store);
        model.loaded(&account, SidebarLoad::Layout(layout));
        model
    }

    #[test]
    fn a_fresh_model_has_nothing_and_refuses_changes() {
        let store = MemorySidebar::default();
        let mut model = SidebarModel::default();
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
        assert_eq!(
            model.apply(Op::ToggleFolder(1), &store).err(),
            Some(Refused::NotLoaded)
        );
        assert!(store.calls().is_empty());
    }

    #[test]
    fn load_for_asks_the_store_once_and_waits_for_the_answer() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        assert_eq!(store.calls(), vec![Call::Load(a.clone())]);
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.apply(Op::ToggleFolder(1), &store).is_err());
        model.loaded(&a, SidebarLoad::Default);
        assert_eq!(model.state(), SidebarState::Loaded);
        assert_eq!(model.layout(), Some(&SidebarLayout::new(a)));
    }

    #[test]
    fn a_late_answer_for_another_account_is_ignored() {
        let store = MemorySidebar::default();
        let (a, b) = (user("@a:x"), user("@b:x"));
        let mut model = SidebarModel::default();
        let _ = model.load_for(b.clone(), &store);
        model.loaded(&a, SidebarLoad::Layout(folder_layout(&a)));
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
        // And one with no load under way.
        model.unload();
        model.loaded(&b, SidebarLoad::Default);
        assert_eq!(model.state(), SidebarState::NotLoaded);
    }

    #[test]
    fn a_layout_of_another_account_is_not_taken() {
        let store = MemorySidebar::default();
        let (a, b) = (user("@a:x"), user("@b:x"));
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::Layout(folder_layout(&b)));
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
    }

    #[test]
    fn newer_other_account_and_unavailable_turn_customizing_off() {
        for (load, state) in [
            (SidebarLoad::Newer, SidebarState::Newer),
            (SidebarLoad::OtherAccount, SidebarState::OtherAccount),
            (SidebarLoad::Unavailable, SidebarState::NotLoaded),
        ] {
            let store = MemorySidebar::default();
            let a = user("@a:x");
            let mut model = SidebarModel::default();
            let _ = model.load_for(a.clone(), &store);
            model.loaded(&a, load);
            assert_eq!(model.state(), state);
            assert!(model.layout().is_none());
            assert!(model.apply(Op::ToggleFolder(1), &store).is_err());
            assert!(model.reset(&store).is_err());
            assert_eq!(store.calls(), vec![Call::Load(a)], "no save, no reset");
        }
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::Newer);
        assert_eq!(
            model.apply(Op::ToggleFolder(1), &store).err(),
            Some(Refused::Protected)
        );
    }

    #[test]
    fn a_successful_change_saves_once_for_the_account_with_the_new_layout() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        assert!(model.apply(Op::ToggleFolder(1), &store).is_ok());
        let mut expected = folder_layout(&a);
        if let RailEntry::Folder(folder) = &mut expected.rail[1] {
            folder.open = true;
        }
        assert_eq!(
            store.calls(),
            vec![Call::Load(a.clone()), Call::Save(a, expected.clone())]
        );
        assert_eq!(model.layout(), Some(&expected));
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn toggling_a_section_collapses_and_expands_it() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        let op = Op::ToggleSection {
            space: rid("!sp:x"),
            key: SectionKey::Direct,
        };
        assert!(model.apply(op.clone(), &store).is_ok());
        let record = |m: &SidebarModel| {
            m.layout()
                .and_then(|l| l.spaces.iter().find(|s| s.space == rid("!sp:x")))
                .map(|s| s.collapsed.clone())
        };
        assert_eq!(record(&model), Some(vec![SectionKey::Direct]));
        assert!(model.apply(op, &store).is_ok());
        assert_eq!(record(&model), Some(Vec::new()));
        let saves = store
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Save(..)))
            .count();
        assert_eq!(saves, 2);
    }

    #[test]
    fn an_unknown_folder_is_refused_and_saves_nothing() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        assert_eq!(
            model.apply(Op::ToggleFolder(9), &store).err(),
            Some(Refused::Unknown)
        );
        assert_eq!(store.calls(), vec![Call::Load(a)]);
    }

    #[test]
    fn a_change_that_passes_a_limit_is_refused_as_full_and_saves_nothing() {
        use twicecat_core::api::sidebar::limits::MAX_SPACE_RECORDS;
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut layout = SidebarLayout::new(a.clone());
        layout.spaces = (0..MAX_SPACE_RECORDS)
            .map(|n| SpaceLayout::new(rid(&format!("!s{n}:x"))))
            .collect();
        assert!(layout.fits());
        let mut model = loaded_model(&store, layout.clone());
        let op = Op::ToggleSection {
            space: rid("!one-too-many:x"),
            key: SectionKey::Starred,
        };
        assert_eq!(model.apply(op, &store).err(), Some(Refused::Full));
        assert_eq!(model.state(), SidebarState::Full);
        assert_eq!(model.layout(), Some(&layout), "the layout is unchanged");
        assert_eq!(store.calls(), vec![Call::Load(a)], "nothing saved");
        // Other changes are still tried, and a good one leaves Full.
        let op = Op::ToggleSection {
            space: rid("!s0:x"),
            key: SectionKey::Starred,
        };
        assert!(model.apply(op, &store).is_ok());
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn unload_drops_everything_and_calls_no_store() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        let before = store.calls();
        model.unload();
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
        assert_eq!(
            model.apply(Op::ToggleFolder(1), &store).err(),
            Some(Refused::NotLoaded)
        );
        assert_eq!(store.calls(), before);
    }

    #[test]
    fn reset_removes_the_file_and_goes_back_to_the_default() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        assert!(model.reset(&store).is_ok());
        assert_eq!(store.calls().last(), Some(&Call::Reset(a.clone())));
        assert_eq!(model.layout(), Some(&SidebarLayout::new(a)));
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn debug_shows_no_room_id_and_no_account() {
        let store = MemorySidebar::default();
        let a = user("@secretme:x");
        let mut layout = folder_layout(&a);
        layout.spaces.push(SpaceLayout::new(rid("!secretspace:x")));
        let model = loaded_model(&store, layout);
        let text = format!("{model:?}");
        for secret in ["!solo", "!in1", "!secretspace", "@secretme"] {
            assert!(!text.contains(secret), "{text}");
        }
    }

    /// A store over the real files that does each job at once, so a test can
    /// follow a layout through the model and the disk.
    struct DiskNow(crate::sidebar_store::DiskSidebar);

    impl SidebarStore for DiskNow {
        fn load(&self, account: &UserId) -> Task<SidebarLoad> {
            Task::done(iced::futures::executor::block_on(self.0.load_job(account)))
        }

        fn save(&self, account: &UserId, layout: SidebarLayout) -> Task<bool> {
            Task::done(iced::futures::executor::block_on(
                self.0.save_job(account, layout),
            ))
        }

        fn reset(&self, account: &UserId) -> Task<bool> {
            Task::done(iced::futures::executor::block_on(self.0.reset_job(account)))
        }
    }

    #[test]
    fn changes_survive_logout_and_never_touch_another_accounts_file() {
        use twicecat_core::api::SidebarRead;
        use twicecat_core::paths::DataPaths;

        let root =
            std::env::temp_dir().join(format!("twicecat-sidebar-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let paths = DataPaths::at(&root);
        let store = DiskNow(crate::sidebar_store::DiskSidebar::new(paths.clone()));
        let (a, b) = (user("@a:x"), user("@b:x"));
        let space_op = |key| Op::ToggleSection {
            space: rid("!sp:x"),
            key,
        };

        // @a changes the layout and logs out.
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::Default);
        assert!(model.apply(space_op(SectionKey::Direct), &store).is_ok());
        let a_layout = model.layout().cloned().expect("a layout");
        let file_a = paths.sidebar_file(&a);
        let bytes_a = std::fs::read(&file_a).expect("a's file");
        model.unload();
        assert_eq!(std::fs::read(&file_a).expect("kept"), bytes_a);

        // @a logs back in: the same layout loads.
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::from(paths.read_sidebar(&a)));
        assert_eq!(model.layout(), Some(&a_layout));
        model.unload();

        // @b gets the default, makes two changes; @a's file is untouched.
        let _ = model.load_for(b.clone(), &store);
        model.loaded(&b, SidebarLoad::from(paths.read_sidebar(&b)));
        assert_eq!(model.layout(), Some(&SidebarLayout::new(b.clone())));
        assert!(model.apply(space_op(SectionKey::Starred), &store).is_ok());
        assert!(model.apply(space_op(SectionKey::Direct), &store).is_ok());
        assert_eq!(std::fs::read(&file_a).expect("a's file"), bytes_a);
        assert_eq!(
            paths.read_sidebar(&b),
            SidebarRead::Layout(model.layout().cloned().expect("a layout"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
