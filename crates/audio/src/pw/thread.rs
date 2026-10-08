//! The processing thread, made through PipeWire's thread utilities so that
//! PipeWire's real-time module (`libpipewire-module-rt`, loaded with our
//! streams' context, as in PipeWire's own `client.conf`) can give it
//! real-time priority the way it gives its own data loop: directly when the
//! session's limits allow it, otherwise through rtkit or, in a Flatpak, the
//! realtime portal. Through rtkit the module only raises threads it created,
//! hence this module.
//!
//! The module registers its utilities on the context (`pw_context_get_object`
//! with the thread-utils interface); without it, threads are made by
//! PipeWire's defaults and real-time priority is not asked for.

use std::ffi::{c_int, c_ulong, c_void};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::properties::PropertiesBox;
use pw::spa::sys::{spa_dict, spa_thread, spa_thread_utils, spa_thread_utils_methods};

use crate::AudioError;
use crate::io::Realtime;

unsafe extern "C" {
    // Exported by libpipewire (its header makes them inline wrappers, so the
    // bindings lack them): PipeWire's default thread utilities.
    fn pw_thread_utils_create(
        props: *const spa_dict,
        start_routine: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void,
    ) -> *mut spa_thread;
    fn pw_thread_utils_join(thread: *mut spa_thread, retval: *mut *mut c_void) -> c_int;
    /// From the C library (`pthread_t` is an unsigned long on Linux).
    fn pthread_self() -> c_ulong;
}

/// PipeWire's default priority for the data loops (`rt.prio`): ours stays
/// just under it, so the device I/O is never held up by our processing.
const DATA_LOOP_PRIORITY: c_int = 83;

/// How long to watch for the priority to change: through rtkit the request
/// is handled on the module's own thread, after `acquire_rt` returns.
const GRANT_WAIT: Duration = Duration::from_millis(500);

/// The real-time module's thread utilities, from a context.
#[derive(Clone, Copy)]
pub(crate) struct ThreadUtils(NonNull<spa_thread_utils>);

// SAFETY: the module's methods lock their own state and PipeWire itself
// calls them from several threads; the pointer stays valid while the context
// lives, and `AudioIo` joins every thread using it before dropping the
// streams that own the context.
unsafe impl Send for ThreadUtils {}

impl ThreadUtils {
    /// The utilities registered on `context`, if the real-time module is
    /// loaded.
    pub(crate) fn of(context: &pw::context::ContextRc) -> Option<Self> {
        let type_ = pw::spa::sys::SPA_TYPE_INTERFACE_ThreadUtils;
        // SAFETY: a live context and a NUL-terminated interface name.
        let object =
            unsafe { pw::sys::pw_context_get_object(context.as_raw_ptr(), type_.as_ptr().cast()) };
        NonNull::new(object.cast()).map(Self)
    }

    /// The module's methods (a copy of its table) and its object.
    fn methods(self) -> (spa_thread_utils_methods, *mut c_void) {
        // SAFETY: a registered interface: its callbacks point at the
        // module's methods table and object, alive with the context.
        unsafe {
            let callbacks = &self.0.as_ref().iface.cb;
            (
                *callbacks.funcs.cast::<spa_thread_utils_methods>(),
                callbacks.data,
            )
        }
    }
}

/// A thread started with [`spawn`]; joined on drop.
pub(crate) struct PwThread {
    thread: Option<NonNull<spa_thread>>,
    utils: Option<ThreadUtils>,
}

// SAFETY: the handle is an opaque `pthread_t`, joinable from any thread.
unsafe impl Send for PwThread {}

impl PwThread {
    /// Waits for the thread to end.
    pub(crate) fn join(mut self) {
        self.join_now();
    }

    fn join_now(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let null = std::ptr::null_mut();
        let join = self
            .utils
            .map(|u| u.methods())
            .and_then(|(m, o)| Some((m.join?, o)));
        // SAFETY: a thread made by these utilities, joined once.
        unsafe {
            match join {
                Some((join, object)) => join(object, thread.as_ptr(), null),
                None => pw_thread_utils_join(thread.as_ptr(), null),
            };
        }
    }
}

impl Drop for PwThread {
    fn drop(&mut self) {
        self.join_now();
    }
}

type Body = Box<dyn FnOnce() + Send>;

unsafe extern "C" fn start(arg: *mut c_void) -> *mut c_void {
    // SAFETY: `arg` is the boxed body `spawn` leaked for this thread, taken
    // back exactly once, here.
    let body = unsafe { Box::from_raw(arg.cast::<Body>()) };
    // A panic must not unwind into C; the owner sees the thread end.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    std::ptr::null_mut()
}

/// Starts `body` on a thread named `name`, made by `utils` (PipeWire's
/// defaults without them). `body` runs once the utilities know the thread,
/// so it may ask for real-time priority at once.
pub(crate) fn spawn(
    utils: Option<ThreadUtils>,
    name: &str,
    body: impl FnOnce() + Send + 'static,
) -> Result<PwThread, AudioError> {
    let mut props = PropertiesBox::new();
    props.insert("thread.name", name);
    let (known, wait) = mpsc::sync_channel::<()>(1);
    let body: Body = Box::new(move || {
        // The real-time module lists a new thread only after it started.
        if wait.recv().is_ok() {
            body();
        }
    });
    let arg = Box::into_raw(Box::new(body)).cast::<c_void>();
    let dict = props.dict().as_raw();
    let create = utils
        .map(|u| u.methods())
        .and_then(|(m, o)| Some((m.create?, o)));
    // SAFETY: the properties are only read during the call; `start` takes
    // ownership of `arg` when the thread starts.
    let thread = unsafe {
        match create {
            Some((create, object)) => create(object, dict, Some(start), arg),
            None => pw_thread_utils_create(dict, start, arg),
        }
    };
    let Some(thread) = NonNull::new(thread) else {
        // No thread started, so `arg` is still ours.
        // SAFETY: the box leaked above, not taken by any thread.
        drop(unsafe { Box::from_raw(arg.cast::<Body>()) });
        return Err(AudioError::Internal);
    };
    let _ = known.send(());
    Ok(PwThread {
        thread: Some(thread),
        utils,
    })
}

/// Asks `utils` for real-time priority for the calling thread (one started
/// with [`spawn`] with them) and reports what the kernel says it got.
pub(crate) fn make_current_realtime(utils: Option<ThreadUtils>) -> Realtime {
    let Some((methods, object)) = utils.map(|u| u.methods()) else {
        return Realtime::Refused("PipeWire's real-time module is not loaded".into());
    };
    let (Some(get_rt_range), Some(acquire_rt)) = (methods.get_rt_range, methods.acquire_rt) else {
        return Realtime::Refused("PipeWire's real-time module has no priority calls".into());
    };
    let (mut min, mut max) = (0, 0);
    // SAFETY: the module's own object; plain out-parameters, no properties.
    let res = unsafe { get_rt_range(object, std::ptr::null(), &mut min, &mut max) };
    if res < 0 {
        return Realtime::Refused(format!("no real-time range ({})", errno_text(res)));
    }
    let priority = (max.min(DATA_LOOP_PRIORITY) - 1).max(min);
    // SAFETY: `pthread_self` has no preconditions; the module identifies the
    // thread by that handle (`pthread_t` as `spa_thread *`).
    let res = unsafe {
        let me = std::ptr::without_provenance_mut::<spa_thread>(pthread_self() as usize);
        acquire_rt(object, me, priority)
    };
    if res < 0 {
        return Realtime::Refused(errno_text(res));
    }
    let asked = Instant::now();
    loop {
        match current_rt_priority() {
            Some(granted) if granted > 0 => return Realtime::Granted { priority: granted },
            None => return Realtime::Refused("could not read the thread's scheduling".into()),
            Some(_) if asked.elapsed() >= GRANT_WAIT => {
                return Realtime::Refused(
                    "not granted (no rtkit or portal answer, or a limit; PipeWire's log says why)"
                        .into(),
                );
            }
            Some(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// The calling thread's real-time priority (0: not real-time), from
/// `/proc/thread-self/stat`.
fn current_rt_priority() -> Option<u32> {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
    // Fields after the command name, which may hold spaces and parentheses:
    // field 3 (state) first, so `rt_priority` (field 40) is at index 37.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(37)?.parse().ok()
}

fn errno_text(res: c_int) -> String {
    std::io::Error::from_raw_os_error(-res).to_string()
}
