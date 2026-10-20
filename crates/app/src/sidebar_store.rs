//! Where the sidebar layout is kept.
//!
//! The rest of the app reaches the layout only through [`SidebarStore`]; only
//! this module names the core's `DataPaths` for it. A later implementation
//! that syncs through the account's data would be another `SidebarStore`.
//! `screens/` must never name [`DiskSidebar`] or the path functions (a test in
//! this module scans for it).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use iced::Task;
use twicecat_core::api::{SidebarLayout, SidebarRead, UserId};
use twicecat_core::paths::DataPaths;

use crate::blocking;

/// What loading an account's layout found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarLoad {
    /// The stored layout.
    Layout(SidebarLayout),
    /// No usable file (none, or a corrupt one set aside): start from the
    /// default arrangement.
    Default,
    /// Saved by a newer Twicecat: never overwritten.
    Newer,
    /// The file names another account: never overwritten.
    OtherAccount,
    /// The read could not run (its thread failed). Nothing is known about the
    /// file.
    Unavailable,
}

impl From<SidebarRead> for SidebarLoad {
    fn from(read: SidebarRead) -> Self {
        match read {
            SidebarRead::Layout(layout) => Self::Layout(layout),
            SidebarRead::Missing | SidebarRead::Corrupt => Self::Default,
            SidebarRead::Newer(_) => Self::Newer,
            SidebarRead::OtherAccount => Self::OtherAccount,
        }
    }
}

/// Where the sidebar layout is kept, one layout per account.
pub trait SidebarStore: Send {
    /// The stored layout for `account`.
    fn load(&self, account: &UserId) -> Task<SidebarLoad>;
    /// Writes `layout` to `account`'s file. Writes are coalesced: the newest
    /// layout handed over wins. Yields whether it worked (`false` when the
    /// file is protected, `Newer` or `OtherAccount`).
    fn save(&self, account: &UserId, layout: SidebarLayout) -> Task<bool>;
    /// Removes `account`'s file (Reset only; never called at logout).
    fn reset(&self, account: &UserId) -> Task<bool>;
}

/// A store that does nothing; `App::new` installs it so tests never touch the
/// disk. It finds no layout.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSidebarStore;

impl SidebarStore for NoSidebarStore {
    fn load(&self, _account: &UserId) -> Task<SidebarLoad> {
        Task::none()
    }

    fn save(&self, _account: &UserId, _layout: SidebarLayout) -> Task<bool> {
        Task::none()
    }

    fn reset(&self, _account: &UserId) -> Task<bool> {
        Task::none()
    }
}

fn unpoisoned<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The sidebar files in the data root, one per account.
///
/// Reads and writes run on [`blocking`] threads, which can overtake one
/// another: each write job writes the newest layout handed to the store for
/// its account, under one lock, so the file always ends up with the last
/// change.
#[derive(Debug, Clone)]
pub struct DiskSidebar {
    paths: DataPaths,
    latest: Arc<Mutex<HashMap<UserId, SidebarLayout>>>,
    writing: Arc<Mutex<()>>,
}

impl DiskSidebar {
    /// A store for the sidebar files under `paths`.
    pub fn new(paths: DataPaths) -> Self {
        Self {
            paths,
            latest: Arc::new(Mutex::new(HashMap::new())),
            writing: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn load_job(
        &self,
        account: &UserId,
    ) -> impl Future<Output = SidebarLoad> + Send + 'static {
        let (paths, account) = (self.paths.clone(), account.clone());
        async move {
            blocking::run(move || SidebarLoad::from(paths.read_sidebar(&account)))
                .await
                .unwrap_or(SidebarLoad::Unavailable)
        }
    }

    pub(crate) fn save_job(
        &self,
        account: &UserId,
        layout: SidebarLayout,
    ) -> impl Future<Output = bool> + Send + 'static {
        // A layout of another account is never written to this account's file.
        let ours = layout.account == *account;
        if ours {
            unpoisoned(self.latest.lock()).insert(account.clone(), layout);
        }
        let (paths, latest, writing, account) = (
            self.paths.clone(),
            self.latest.clone(),
            self.writing.clone(),
            account.clone(),
        );
        async move {
            if !ours {
                return false;
            }
            blocking::run(move || {
                let _guard = unpoisoned(writing.lock());
                let newest = unpoisoned(latest.lock()).get(&account).cloned();
                newest.is_some_and(|layout| paths.write_sidebar(&layout).is_ok())
            })
            .await
            .unwrap_or(false)
        }
    }

    pub(crate) fn reset_job(
        &self,
        account: &UserId,
    ) -> impl Future<Output = bool> + Send + 'static {
        // A save still waiting for its turn finds nothing to write.
        unpoisoned(self.latest.lock()).remove(account);
        let (paths, writing, account) = (self.paths.clone(), self.writing.clone(), account.clone());
        async move {
            blocking::run(move || {
                let _guard = unpoisoned(writing.lock());
                paths.remove_sidebar(&account).is_ok()
            })
            .await
            .unwrap_or(false)
        }
    }
}

impl SidebarStore for DiskSidebar {
    fn load(&self, account: &UserId) -> Task<SidebarLoad> {
        Task::future(self.load_job(account))
    }

    fn save(&self, account: &UserId, layout: SidebarLayout) -> Task<bool> {
        Task::future(self.save_job(account, layout))
    }

    fn reset(&self, account: &UserId) -> Task<bool> {
        Task::future(self.reset_job(account))
    }
}

/// A fake store that records every call and touches no disk.
#[cfg(test)]
pub use fake::{Call, MemorySidebar};

#[cfg(test)]
mod fake {
    use super::*;

    /// One call to a [`MemorySidebar`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Call {
        /// `load(account)`.
        Load(UserId),
        /// `save(account, layout)`.
        Save(UserId, SidebarLayout),
        /// `reset(account)`.
        Reset(UserId),
    }

    /// Records calls; `load` answers with `answer`.
    #[derive(Debug, Clone)]
    pub struct MemorySidebar {
        /// Every call so far, in order.
        pub calls: Arc<Mutex<Vec<Call>>>,
        /// What `load` yields.
        pub answer: Arc<Mutex<SidebarLoad>>,
    }

    impl Default for MemorySidebar {
        fn default() -> Self {
            Self {
                calls: Arc::default(),
                answer: Arc::new(Mutex::new(SidebarLoad::Default)),
            }
        }
    }

    impl MemorySidebar {
        /// The calls made so far.
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("test mutex").clone()
        }
    }

    impl SidebarStore for MemorySidebar {
        fn load(&self, account: &UserId) -> Task<SidebarLoad> {
            self.calls
                .lock()
                .expect("test mutex")
                .push(Call::Load(account.clone()));
            Task::done(self.answer.lock().expect("test mutex").clone())
        }

        fn save(&self, account: &UserId, layout: SidebarLayout) -> Task<bool> {
            self.calls
                .lock()
                .expect("test mutex")
                .push(Call::Save(account.clone(), layout));
            Task::done(true)
        }

        fn reset(&self, account: &UserId) -> Task<bool> {
            self.calls
                .lock()
                .expect("test mutex")
                .push(Call::Reset(account.clone()));
            Task::done(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use iced::futures::executor::block_on;
    use twicecat_core::api::{RailEntry, RoomId};

    use super::*;

    /// A fresh data root under the system temp dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("twicecat-sidebar-store-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create the test dir");
            Self(path)
        }

        fn paths(&self) -> DataPaths {
            DataPaths::at(&self.0)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn user(name: &str) -> UserId {
        UserId::from(name)
    }

    fn layout_of(account: &UserId, spaces: &[&str]) -> SidebarLayout {
        let mut layout = SidebarLayout::new(account.clone());
        layout.rail = spaces
            .iter()
            .map(|s| RailEntry::Space(RoomId::from(*s)))
            .collect();
        layout
    }

    #[test]
    fn a_read_maps_to_a_load() {
        let layout = layout_of(&user("@a:x"), &["!one:x"]);
        assert_eq!(
            SidebarLoad::from(SidebarRead::Layout(layout.clone())),
            SidebarLoad::Layout(layout)
        );
        assert_eq!(
            SidebarLoad::from(SidebarRead::Missing),
            SidebarLoad::Default
        );
        assert_eq!(
            SidebarLoad::from(SidebarRead::Corrupt),
            SidebarLoad::Default
        );
        assert_eq!(SidebarLoad::from(SidebarRead::Newer(2)), SidebarLoad::Newer);
        assert_eq!(
            SidebarLoad::from(SidebarRead::OtherAccount),
            SidebarLoad::OtherAccount
        );
    }

    #[test]
    fn a_missing_file_loads_as_the_default_and_reading_creates_nothing() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        assert_eq!(
            block_on(store.load_job(&user("@a:x"))),
            SidebarLoad::Default
        );
        assert_eq!(std::fs::read_dir(&dir.0).expect("dir").count(), 0);
    }

    #[test]
    fn two_saves_in_quick_succession_leave_the_newer_layout_on_disk() {
        let a = user("@a:x");
        for reversed in [false, true] {
            let dir = Dir::new();
            let store = DiskSidebar::new(dir.paths());
            let first = store.save_job(&a, layout_of(&a, &["!one:x"]));
            let second = store.save_job(&a, layout_of(&a, &["!one:x", "!two:x"]));
            // The blocking threads can finish in either order.
            let (x, y) = if reversed {
                (block_on(second), block_on(first))
            } else {
                (block_on(first), block_on(second))
            };
            assert!(x && y);
            assert_eq!(
                dir.paths().read_sidebar(&a),
                SidebarRead::Layout(layout_of(&a, &["!one:x", "!two:x"]))
            );
        }
    }

    #[test]
    fn a_failed_write_yields_false_without_a_panic() {
        let dir = Dir::new();
        // The data root is a file, so nothing can be created under it.
        let blocked = dir.0.join("file");
        std::fs::write(&blocked, b"x").expect("write");
        let store = DiskSidebar::new(DataPaths::at(blocked.join("root")));
        let a = user("@a:x");
        assert!(!block_on(store.save_job(&a, layout_of(&a, &["!one:x"]))));
    }

    #[test]
    fn a_layout_of_another_account_is_never_written() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let (a, b) = (user("@a:x"), user("@b:x"));
        assert!(!block_on(store.save_job(&a, layout_of(&b, &["!one:x"]))));
        assert_eq!(dir.paths().read_sidebar(&a), SidebarRead::Missing);
        assert_eq!(dir.paths().read_sidebar(&b), SidebarRead::Missing);
    }

    #[test]
    fn the_file_is_kept_at_logout_and_loads_again() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let a = user("@a:x");
        let layout = layout_of(&a, &["!one:x"]);
        assert!(block_on(store.save_job(&a, layout.clone())));
        let file = dir.paths().sidebar_file(&a);
        let before = std::fs::read(&file).expect("file");
        // A logout calls the store for nothing; a login loads again, even from
        // a new store over the same root (a new run).
        let again = DiskSidebar::new(dir.paths());
        assert_eq!(block_on(again.load_job(&a)), SidebarLoad::Layout(layout));
        assert_eq!(std::fs::read(&file).expect("file"), before);
    }

    #[test]
    fn another_accounts_file_is_ignored_and_never_overwritten() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let (a, b) = (user("@a:x"), user("@b:x"));
        assert!(block_on(store.save_job(&a, layout_of(&a, &["!one:x"]))));
        let file_a = dir.paths().sidebar_file(&a);
        let before = std::fs::read(&file_a).expect("file");
        assert_eq!(block_on(store.load_job(&b)), SidebarLoad::Default);
        assert!(block_on(store.save_job(&b, layout_of(&b, &["!two:x"]))));
        assert!(block_on(
            store.save_job(&b, layout_of(&b, &["!two:x", "!three:x"]))
        ));
        assert_eq!(std::fs::read(&file_a).expect("file"), before);
        assert_eq!(
            dir.paths().read_sidebar(&b),
            SidebarRead::Layout(layout_of(&b, &["!two:x", "!three:x"]))
        );
    }

    #[test]
    fn a_protected_file_loads_as_such_and_a_forced_save_yields_false() {
        let a = user("@a:x");
        let planted = [
            (
                r#"{"version":1,"account":"@other:x","rail":[],"spaces":[]}"#,
                SidebarLoad::OtherAccount,
            ),
            (
                r#"{"version":2,"account":"@a:x","rail":[],"spaces":[]}"#,
                SidebarLoad::Newer,
            ),
        ];
        for (text, expected) in planted {
            let dir = Dir::new();
            let store = DiskSidebar::new(dir.paths());
            let file = dir.paths().sidebar_file(&a);
            std::fs::write(&file, text).expect("plant");
            assert_eq!(block_on(store.load_job(&a)), expected);
            assert!(!block_on(store.save_job(&a, layout_of(&a, &["!one:x"]))));
            assert!(!block_on(store.reset_job(&a)));
            assert_eq!(std::fs::read(&file).expect("file"), text.as_bytes());
        }
    }

    #[test]
    fn reset_removes_the_file_and_cancels_a_waiting_save() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let a = user("@a:x");
        assert!(block_on(store.save_job(&a, layout_of(&a, &["!one:x"]))));
        let waiting = store.save_job(&a, layout_of(&a, &["!two:x"]));
        assert!(block_on(store.reset_job(&a)));
        assert!(!block_on(waiting), "nothing left to write");
        assert_eq!(dir.paths().read_sidebar(&a), SidebarRead::Missing);
    }

    #[test]
    fn only_the_store_and_main_name_the_sidebar_files_or_the_disk_store() {
        for (path, text) in crate::source_scan::sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let code = crate::source_scan::mask(&crate::source_scan::production_part(&text));
            if name != "sidebar_store.rs" {
                for word in [
                    "read_sidebar",
                    "write_sidebar",
                    "remove_sidebar",
                    "sidebar_file",
                ] {
                    assert!(!code.contains(word), "{} names {word}", path.display());
                }
            }
            if name != "sidebar_store.rs" && name != "main.rs" {
                assert!(!code.contains("DiskSidebar"), "{} names it", path.display());
            }
        }
    }

    #[test]
    fn screens_never_name_the_disk_store_or_the_path_functions() {
        let mut checked = 0;
        for (path, text) in crate::source_scan::sources() {
            let in_screens = path.components().any(|c| c.as_os_str() == "screens");
            if !in_screens {
                continue;
            }
            checked += 1;
            let code = crate::source_scan::mask(&text);
            for word in ["DiskSidebar", "paths::", "read_sidebar", "write_sidebar"] {
                assert!(!code.contains(word), "{} names {word}", path.display());
            }
        }
        assert!(checked > 0, "found the screens");
    }
}
