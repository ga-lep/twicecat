//! Devices from the PipeWire registry: a one-shot list, and a watcher that
//! reports changes as they happen (devices added or removed, defaults
//! moved), instead of polling.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::context::ContextRc;
use pw::core::{CoreRc, PW_ID_CORE};
use pw::main_loop::MainLoopRc;
use pw::metadata::{Metadata, MetadataListener};
use pw::registry::{Listener as RegistryListener, RegistryRc};
use pw::spa::utils::result::AsyncSeq;
use pw::types::ObjectType;

use super::graph::{DEFAULT_METADATA, Graph, Node};
use crate::AudioError;
use crate::devices::DeviceList;

/// How long a one-shot list may take before it gives up.
const LIST_TIMEOUT: Duration = Duration::from_secs(2);

/// A connection to the PipeWire daemon, on the current thread.
pub(crate) struct Session {
    pub mainloop: MainLoopRc,
    _context: ContextRc,
    pub core: CoreRc,
    registry: RegistryRc,
}

impl Session {
    /// The real-time module's thread utilities, if the context loaded it.
    pub(crate) fn thread_utils(&self) -> Option<super::thread::ThreadUtils> {
        super::thread::ThreadUtils::of(&self._context)
    }

    pub(crate) fn connect() -> Result<Self, AudioError> {
        pw::init();
        let mainloop = MainLoopRc::new(None).map_err(|_| AudioError::Backend)?;
        let context = ContextRc::new(&mainloop, None).map_err(|_| AudioError::Backend)?;
        let core = context.connect_rc(None).map_err(|_| AudioError::Backend)?;
        let registry = core.get_registry_rc().map_err(|_| AudioError::Backend)?;
        Ok(Self {
            mainloop,
            _context: context,
            core,
            registry,
        })
    }
}

/// What keeps a registry follow running.
struct Following {
    _registry: RegistryListener,
    _metadata: Rc<RefCell<Vec<(Metadata, MetadataListener)>>>,
}

/// Mirrors the registry's device nodes and default devices into `graph`;
/// `changed` runs after each change. `bound` runs when the default metadata
/// is bound, whose properties then arrive.
fn follow(
    session: &Session,
    graph: Rc<RefCell<Graph>>,
    changed: Rc<dyn Fn()>,
    bound: Rc<dyn Fn()>,
) -> Following {
    let metadata: Rc<RefCell<Vec<(Metadata, MetadataListener)>>> = Rc::default();
    let registry = session.registry.clone();
    let listener = session
        .registry
        .add_listener_local()
        .global({
            let graph = graph.clone();
            let changed = changed.clone();
            let metadata = metadata.clone();
            move |global| {
                let Some(props) = global.props else {
                    return;
                };
                match global.type_ {
                    ObjectType::Node => {
                        if let Some(node) = Node::from_props(|key| props.get(key)) {
                            graph.borrow_mut().nodes.insert(global.id, node);
                            changed();
                        }
                    }
                    ObjectType::Metadata
                        if props.get("metadata.name") == Some(DEFAULT_METADATA) =>
                    {
                        let Ok(proxy) = registry.bind::<Metadata, _>(global) else {
                            return;
                        };
                        let graph = graph.clone();
                        let changed = changed.clone();
                        let listener = proxy
                            .add_listener_local()
                            .property(move |subject, key, _, value| {
                                if subject == PW_ID_CORE
                                    && graph.borrow_mut().set_default(key, value)
                                {
                                    changed();
                                }
                                0
                            })
                            .register();
                        metadata.borrow_mut().push((proxy, listener));
                        bound();
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let graph = graph.clone();
            move |id| {
                if graph.borrow_mut().nodes.remove(&id).is_some() {
                    changed();
                }
            }
        })
        .register();
    Following {
        _registry: listener,
        _metadata: metadata,
    }
}

/// Waits for the server to have answered everything asked so far: each
/// `sync` adds a sequence, and the loop quits when all came back.
struct Roundtrips {
    pending: Rc<RefCell<Vec<AsyncSeq>>>,
    _listener: pw::core::Listener,
}

impl Roundtrips {
    fn new(session: &Session) -> Self {
        let pending: Rc<RefCell<Vec<AsyncSeq>>> = Rc::default();
        let mainloop = session.mainloop.clone();
        let listener = session
            .core
            .add_listener_local()
            .done({
                let pending = pending.clone();
                move |id, seq| {
                    if id != PW_ID_CORE {
                        return;
                    }
                    let mut pending = pending.borrow_mut();
                    pending.retain(|s| *s != seq);
                    if pending.is_empty() {
                        mainloop.quit();
                    }
                }
            })
            .register();
        Self {
            pending,
            _listener: listener,
        }
    }

    fn sync(&self, core: &CoreRc) {
        if let Ok(seq) = core.sync(0) {
            self.pending.borrow_mut().push(seq);
        }
    }
}

/// The devices now. Blocking, at most [`LIST_TIMEOUT`].
pub(crate) fn list() -> Result<DeviceList, AudioError> {
    let session = Session::connect().map_err(|_| AudioError::Enumerate)?;
    let graph: Rc<RefCell<Graph>> = Rc::default();
    let roundtrips = Rc::new(Roundtrips::new(&session));
    roundtrips.sync(&session.core);
    let _following = follow(
        &session,
        graph.clone(),
        Rc::new(|| {}),
        Rc::new({
            let roundtrips = roundtrips.clone();
            let core = session.core.clone();
            // The metadata's properties come after the bind: wait for them.
            move || roundtrips.sync(&core)
        }),
    );
    let timed_out = Rc::new(RefCell::new(false));
    let timer = session.mainloop.loop_().add_timer({
        let mainloop = session.mainloop.clone();
        let timed_out = timed_out.clone();
        move |_| {
            *timed_out.borrow_mut() = true;
            mainloop.quit();
        }
    });
    let _ = timer.update_timer(Some(LIST_TIMEOUT), None);
    session.mainloop.run();
    if *timed_out.borrow() {
        return Err(AudioError::Enumerate);
    }
    let devices = graph.borrow().devices();
    Ok(devices)
}

/// Reports every change of the device list, from registry and metadata
/// events, on its own thread. Dropping it stops the thread.
pub(crate) struct Watcher {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Calls `on_change` with the new list after each change (not for the
    /// list at start).
    pub(crate) fn spawn(
        on_change: impl FnMut(DeviceList) + Send + 'static,
    ) -> Result<Self, AudioError> {
        let (stop, stopped) = pw::channel::channel::<()>();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("twicecat-pw-devices".into())
            .spawn(move || {
                let session = match Session::connect() {
                    Ok(session) => session,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let graph: Rc<RefCell<Graph>> = Rc::default();
                let last: Rc<RefCell<Option<DeviceList>>> = Rc::default();
                let on_change = RefCell::new(on_change);
                let started = Rc::new(RefCell::new(false));
                let roundtrips = Rc::new(Roundtrips::new(&session));
                roundtrips.sync(&session.core);
                let _following = follow(
                    &session,
                    graph.clone(),
                    Rc::new({
                        let graph = graph.clone();
                        let started = started.clone();
                        let last = last.clone();
                        move || {
                            if !*started.borrow() {
                                return;
                            }
                            let now = graph.borrow().devices();
                            let mut last = last.borrow_mut();
                            if last.as_ref() != Some(&now) {
                                *last = Some(now.clone());
                                (on_change.borrow_mut())(now);
                            }
                        }
                    }),
                    Rc::new({
                        let started = started.clone();
                        let roundtrips = roundtrips.clone();
                        let core = session.core.clone();
                        // Until the baseline is taken, wait for the default
                        // metadata's properties too.
                        move || {
                            if !*started.borrow() {
                                roundtrips.sync(&core);
                            }
                        }
                    }),
                );
                // The list at start is the baseline, not a change.
                session.mainloop.run();
                *last.borrow_mut() = Some(graph.borrow().devices());
                *started.borrow_mut() = true;
                let _ = ready_tx.send(Ok(()));
                let mainloop = session.mainloop.clone();
                let _stop = stopped.attach(session.mainloop.loop_(), move |()| mainloop.quit());
                session.mainloop.run();
            })
            .map_err(|_| AudioError::Internal)?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(AudioError::Internal)
            }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
