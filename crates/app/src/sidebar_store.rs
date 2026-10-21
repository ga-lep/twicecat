//! Where the sidebar layout is kept.
//!
//! The rest of the app reaches the layout only through [`SidebarStore`]; only
//! this module names the core's `DataPaths` for it. A later implementation
//! that syncs through the account's data would be another `SidebarStore`.
//! `screens/` must never name [`DiskSidebar`] or the path functions (a test in
//! this module scans for it).

use std::collections::HashMap;
use std::fmt;
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

/// The answer to a load, with the account it was asked for (a late answer
/// for another account is ignored). `Debug` hides the account.
#[derive(Clone, PartialEq, Eq)]
pub struct SidebarAnswer {
    /// The account the load was for.
    pub account: UserId,
    /// What it found.
    pub load: SidebarLoad,
}

impl SidebarAnswer {
    /// The answer `load` for `account`.
    pub fn new(account: UserId, load: SidebarLoad) -> Self {
        Self { account, load }
    }
}

impl fmt::Debug for SidebarAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SidebarAnswer")
            .field("account", &"<redacted>")
            // A layout's own `Debug` shows counts only.
            .field("load", &self.load)
            .finish()
    }
}

/// What a reset did to an account's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetOutcome {
    /// The file was removed (or was not there).
    Removed,
    /// The file could not be removed (or the request could not run).
    Failed,
    /// A later change of the layout ran first, so the file holds that
    /// layout: nothing failed, and nothing was removed.
    Overtaken,
}

/// The outcome of a reset, with the account it was for (a late answer after
/// another account logged in is ignored). `Debug` hides the account.
#[derive(Clone, PartialEq, Eq)]
pub struct ResetAnswer {
    /// The account the reset was for.
    pub account: UserId,
    /// What happened.
    pub outcome: ResetOutcome,
}

impl ResetAnswer {
    /// The answer `outcome` for `account`.
    pub fn new(account: UserId, outcome: ResetOutcome) -> Self {
        Self { account, outcome }
    }
}

impl fmt::Debug for ResetAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResetAnswer")
            .field("account", &"<redacted>")
            .field("outcome", &self.outcome)
            .finish()
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
    fn reset(&self, account: &UserId) -> Task<ResetOutcome>;
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

    fn reset(&self, _account: &UserId) -> Task<ResetOutcome> {
        Task::none()
    }
}

fn unpoisoned<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What an account's file is to become, as the newest request asked.
#[derive(Clone)]
enum Pending {
    /// Write this layout.
    Write(SidebarLayout),
    /// Remove the file (Reset).
    Remove,
}

/// What the last request that ran did to an account's file.
#[derive(Clone, Copy)]
struct Applied {
    /// The file was removed (else a layout was written).
    removed: bool,
    /// The disk work worked.
    ok: bool,
}

/// What the store knows about one account's file.
#[derive(Default)]
struct Account {
    /// The newest request that has not run yet. Requests are coalesced: the
    /// newest one decides how the file ends, because a write replaces the
    /// whole file and a removal deletes it.
    pending: Option<Pending>,
    /// The last request that ran.
    applied: Option<Applied>,
}

/// The sidebar files in the data root, one per account.
///
/// Reads and writes run on [`blocking`] threads, which can overtake one
/// another. So a request does not carry its own work: it records what the
/// account's file is to become (`Write(layout)` or `Remove`, the newest
/// request wins) and each job, under one lock, applies whatever is recorded
/// when its turn comes. The file therefore always ends the way the last
/// request asked, whichever job runs first. A load that finds a request
/// waiting answers from it (the file on disk is older); otherwise it reads
/// the file under the same lock, after any write in progress.
///
/// `Debug` shows how many accounts have something waiting and no path, ID or
/// name.
#[derive(Clone)]
pub struct DiskSidebar {
    paths: DataPaths,
    accounts: Arc<Mutex<HashMap<UserId, Account>>>,
    writing: Arc<Mutex<()>>,
}

impl fmt::Debug for DiskSidebar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let accounts = unpoisoned(self.accounts.lock());
        f.debug_struct("DiskSidebar")
            .field("accounts", &accounts.len())
            .field(
                "waiting",
                &accounts.values().filter(|a| a.pending.is_some()).count(),
            )
            .finish_non_exhaustive()
    }
}

impl DiskSidebar {
    /// A store for the sidebar files under `paths`.
    pub fn new(paths: DataPaths) -> Self {
        Self {
            paths,
            accounts: Arc::new(Mutex::new(HashMap::new())),
            writing: Arc::new(Mutex::new(())),
        }
    }

    /// Records what the account's file is to become.
    fn request(&self, account: &UserId, pending: Pending) {
        unpoisoned(self.accounts.lock())
            .entry(account.clone())
            .or_default()
            .pending = Some(pending);
    }

    /// Runs what is recorded for `account`, under the write lock, and says
    /// what happened. When nothing is recorded, a job that ran later already
    /// covered this request: `None`.
    fn run_pending(
        paths: &DataPaths,
        accounts: &Mutex<HashMap<UserId, Account>>,
        account: &UserId,
    ) -> Option<Applied> {
        let pending = unpoisoned(accounts.lock())
            .get_mut(account)
            .and_then(|a| a.pending.take())?;
        // The disk work runs with only the write lock held, so a request from
        // the UI never waits for the disk.
        let applied = match pending {
            Pending::Write(layout) => Applied {
                removed: false,
                ok: paths.write_sidebar(&layout).is_ok(),
            },
            Pending::Remove => Applied {
                removed: true,
                ok: paths.remove_sidebar(account).is_ok(),
            },
        };
        unpoisoned(accounts.lock())
            .entry(account.clone())
            .or_default()
            .applied = Some(applied);
        Some(applied)
    }

    pub(crate) fn load_job(
        &self,
        account: &UserId,
    ) -> impl Future<Output = SidebarLoad> + Send + 'static {
        // A request still waiting is newer than the file.
        let waiting = unpoisoned(self.accounts.lock())
            .get(account)
            .and_then(|a| a.pending.clone());
        let (paths, writing, account) = (self.paths.clone(), self.writing.clone(), account.clone());
        async move {
            match waiting {
                Some(Pending::Write(layout)) => SidebarLoad::Layout(layout),
                Some(Pending::Remove) => SidebarLoad::Default,
                None => blocking::run(move || {
                    // After any write in progress, so the file is current.
                    let _guard = unpoisoned(writing.lock());
                    SidebarLoad::from(paths.read_sidebar(&account))
                })
                .await
                .unwrap_or(SidebarLoad::Unavailable),
            }
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
            self.request(account, Pending::Write(layout));
        }
        let (paths, accounts, writing, account) = (
            self.paths.clone(),
            self.accounts.clone(),
            self.writing.clone(),
            account.clone(),
        );
        async move {
            if !ours {
                return false;
            }
            blocking::run(move || {
                let _guard = unpoisoned(writing.lock());
                match Self::run_pending(&paths, &accounts, &account) {
                    Some(applied) => !applied.removed && applied.ok,
                    // A later request ran first and covered this one. A later
                    // write holds this layout or a newer one; a later reset
                    // cancelled it.
                    None => unpoisoned(accounts.lock())
                        .get(&account)
                        .and_then(|a| a.applied)
                        .is_some_and(|applied| !applied.removed && applied.ok),
                }
            })
            .await
            .unwrap_or(false)
        }
    }

    pub(crate) fn reset_job(
        &self,
        account: &UserId,
    ) -> impl Future<Output = ResetOutcome> + Send + 'static {
        self.request(account, Pending::Remove);
        let (paths, accounts, writing, account) = (
            self.paths.clone(),
            self.accounts.clone(),
            self.writing.clone(),
            account.clone(),
        );
        async move {
            blocking::run(move || {
                let _guard = unpoisoned(writing.lock());
                // Reset worked only if the file was removed. When a later
                // save ran first and covered this request, the file holds
                // that layout: that save's success is not the reset's, and
                // it is not a failure either.
                let outcome = |applied: Applied| match (applied.removed, applied.ok) {
                    (true, true) => ResetOutcome::Removed,
                    (true, false) => ResetOutcome::Failed,
                    (false, true) => ResetOutcome::Overtaken,
                    // The later write failed too: the old layout is still there.
                    (false, false) => ResetOutcome::Failed,
                };
                match Self::run_pending(&paths, &accounts, &account) {
                    Some(applied) => outcome(applied),
                    None => unpoisoned(accounts.lock())
                        .get(&account)
                        .and_then(|a| a.applied)
                        .map_or(ResetOutcome::Failed, outcome),
                }
            })
            .await
            .unwrap_or(ResetOutcome::Failed)
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

    fn reset(&self, account: &UserId) -> Task<ResetOutcome> {
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

        fn reset(&self, account: &UserId) -> Task<ResetOutcome> {
            self.calls
                .lock()
                .expect("test mutex")
                .push(Call::Reset(account.clone()));
            Task::done(ResetOutcome::Removed)
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
            assert_ne!(block_on(store.reset_job(&a)), ResetOutcome::Removed);
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
        assert_eq!(block_on(store.reset_job(&a)), ResetOutcome::Removed);
        assert!(!block_on(waiting), "nothing left to write");
        assert_eq!(dir.paths().read_sidebar(&a), SidebarRead::Missing);
    }

    // ---- Debug shows counts only ----

    fn secret_layout(account: &UserId) -> SidebarLayout {
        use twicecat_core::api::{FolderLayout, SidebarLabel};
        let mut layout = layout_of(account, &["!secretroom:x"]);
        layout.rail.push(RailEntry::Folder(FolderLayout {
            id: 3,
            name: Some(SidebarLabel::try_from("Payroll").expect("a valid name")),
            open: true,
            spaces: vec![RoomId::from("!secretroom2:x")],
        }));
        layout
    }

    fn assert_no_secret(text: &str) {
        for secret in ["!secretroom", "@a:x", "Payroll", "Payroll-root"] {
            assert!(!text.contains(secret), "{secret} is in {text}");
        }
    }

    #[test]
    fn the_stores_debug_shows_counts_only() {
        let dir = Dir::new();
        let root = dir.0.join("Payroll-root");
        std::fs::create_dir_all(&root).expect("dir");
        let store = DiskSidebar::new(DataPaths::at(&root));
        let a = user("@a:x");
        // A write is waiting: the store holds the layout, the account and a path.
        let _queued = store.save_job(&a, secret_layout(&a));
        assert_no_secret(&format!("{store:?}"));
        assert_no_secret(&format!("{:?}", SidebarLoad::Layout(secret_layout(&a))));
    }

    // ---- ordering ----

    #[test]
    fn a_load_after_a_queued_save_gets_the_queued_layout_not_the_old_file() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let a = user("@a:x");
        assert!(block_on(store.save_job(&a, layout_of(&a, &["!old:x"]))));
        // The newer save has been handed over but has not run yet.
        let queued = store.save_job(&a, layout_of(&a, &["!old:x", "!new:x"]));
        assert_eq!(
            block_on(store.load_job(&a)),
            SidebarLoad::Layout(layout_of(&a, &["!old:x", "!new:x"]))
        );
        assert!(block_on(queued));
        assert_eq!(
            block_on(store.load_job(&a)),
            SidebarLoad::Layout(layout_of(&a, &["!old:x", "!new:x"]))
        );
    }

    #[test]
    fn a_load_after_a_queued_reset_finds_the_default() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let a = user("@a:x");
        assert!(block_on(store.save_job(&a, layout_of(&a, &["!old:x"]))));
        let queued = store.reset_job(&a);
        assert_eq!(block_on(store.load_job(&a)), SidebarLoad::Default);
        assert_eq!(block_on(queued), ResetOutcome::Removed);
    }

    #[test]
    fn a_load_queued_for_another_account_is_not_answered_from_this_ones_save() {
        let dir = Dir::new();
        let store = DiskSidebar::new(dir.paths());
        let (a, b) = (user("@a:x"), user("@b:x"));
        let _queued = store.save_job(&a, layout_of(&a, &["!one:x"]));
        assert_eq!(block_on(store.load_job(&b)), SidebarLoad::Default);
    }

    #[test]
    fn a_reset_then_a_save_leave_the_save_whichever_finishes_first() {
        let a = user("@a:x");
        for save_first in [false, true] {
            let dir = Dir::new();
            let store = DiskSidebar::new(dir.paths());
            assert!(block_on(store.save_job(&a, layout_of(&a, &["!old:x"]))));
            let reset = store.reset_job(&a);
            let save = store.save_job(&a, layout_of(&a, &["!new:x"]));
            if save_first {
                assert!(block_on(save));
                let _ = block_on(reset);
            } else {
                let _ = block_on(reset);
                assert!(block_on(save));
            }
            assert_eq!(
                dir.paths().read_sidebar(&a),
                SidebarRead::Layout(layout_of(&a, &["!new:x"])),
                "save_first={save_first}"
            );
        }
    }

    #[test]
    fn a_reset_overtaken_by_a_later_save_does_not_report_a_removal() {
        let a = user("@a:x");
        for save_first in [false, true] {
            let dir = Dir::new();
            let store = DiskSidebar::new(dir.paths());
            assert!(block_on(store.save_job(&a, layout_of(&a, &["!old:x"]))));
            let reset = store.reset_job(&a);
            let save = store.save_job(&a, layout_of(&a, &["!new:x"]));
            let (reset_ok, save_ok) = if save_first {
                let save_ok = block_on(save);
                (block_on(reset), save_ok)
            } else {
                let reset_ok = block_on(reset);
                (reset_ok, block_on(save))
            };
            // The save wrote the file, so it succeeded. The reset did not
            // remove anything that is still there: it must not claim to.
            assert!(save_ok, "save_first={save_first}");
            assert_eq!(
                reset_ok,
                ResetOutcome::Overtaken,
                "the file holds the new layout, save_first={save_first}"
            );
            assert!(dir.paths().sidebar_file(&a).exists());
        }
    }

    #[test]
    fn a_reset_overtaken_by_a_save_that_fails_is_a_failure() {
        let a = user("@a:x");
        for save_first in [false, true] {
            let dir = Dir::new();
            // Nothing can be written under a data root that is a file; the
            // removal of a file that is not there works.
            let blocked = dir.0.join("file");
            std::fs::write(&blocked, b"x").expect("write");
            let store = DiskSidebar::new(DataPaths::at(blocked.join("root")));
            let reset = store.reset_job(&a);
            let save = store.save_job(&a, layout_of(&a, &["!new:x"]));
            let (reset_out, save_ok) = if save_first {
                let save_ok = block_on(save);
                (block_on(reset), save_ok)
            } else {
                let reset_out = block_on(reset);
                (reset_out, block_on(save))
            };
            assert!(!save_ok, "the write fails");
            assert_eq!(
                reset_out,
                ResetOutcome::Failed,
                "the old layout is still there, save_first={save_first}"
            );
        }
    }

    #[test]
    fn a_save_then_a_reset_leave_no_file_whichever_finishes_first() {
        let a = user("@a:x");
        for reset_first in [false, true] {
            let dir = Dir::new();
            let store = DiskSidebar::new(dir.paths());
            assert!(block_on(store.save_job(&a, layout_of(&a, &["!old:x"]))));
            let save = store.save_job(&a, layout_of(&a, &["!new:x"]));
            let reset = store.reset_job(&a);
            if reset_first {
                assert_eq!(block_on(reset), ResetOutcome::Removed);
                let _ = block_on(save);
            } else {
                let _ = block_on(save);
                assert_eq!(block_on(reset), ResetOutcome::Removed);
            }
            assert_eq!(
                dir.paths().read_sidebar(&a),
                SidebarRead::Missing,
                "reset_first={reset_first}"
            );
        }
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
