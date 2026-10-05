//! The bridge between Iced and the core (plan Q3/Q4).
//!
//! The core runs on its own tokio runtime, owned by [`Bridge`]. Commands go
//! down the core's channel with a non-blocking `try_send`; events come back
//! through an Iced [`Subscription`] built from [`EventsSlot`].

use std::hash::{Hash, Hasher};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::Subscription;
use iced::Task;
use iced::futures::{Stream, stream};
use tessera_core::api::{Command, CoreEvent};
use tessera_core::secrets::SecretStore;
use tessera_core::{Core, CoreConfig};
use tokio::runtime::{Builder, Handle, Runtime};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::state::Message;

/// How long dropping a [`Bridge`] waits for the core runtime to stop.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(5);

/// One item of the event stream.
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    /// The core sent an event.
    Core(CoreEvent),
    /// The core's event channel closed (after `Shutdown`).
    Closed,
}

/// Holds the core's event receiver until the subscription takes it.
///
/// All clones share one receiver, and all hash alike, so Iced sees a single
/// subscription for the lifetime of the app.
#[derive(Clone, Debug)]
pub struct EventsSlot(Arc<Mutex<Option<mpsc::Receiver<CoreEvent>>>>);

impl EventsSlot {
    /// Wraps a receiver.
    pub fn new(events: mpsc::Receiver<CoreEvent>) -> Self {
        Self(Arc::new(Mutex::new(Some(events))))
    }

    /// Takes the receiver out; `None` if it was already taken.
    fn take(&self) -> Option<mpsc::Receiver<CoreEvent>> {
        // A poisoned lock only means another thread panicked while holding it;
        // the `Option` inside is still valid.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

impl Hash for EventsSlot {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "tessera-core-events".hash(state);
    }
}

/// The core's events as a stream: every event in order, then one
/// [`BridgeEvent::Closed`], then the end. The first call takes the receiver; a
/// later call on the same slot yields an empty stream.
pub fn events_stream(slot: &EventsSlot) -> impl Stream<Item = BridgeEvent> + use<> {
    stream::unfold(slot.take(), |receiver| async move {
        let mut receiver = receiver?;
        match receiver.recv().await {
            Some(event) => Some((BridgeEvent::Core(event), Some(receiver))),
            None => Some((BridgeEvent::Closed, None)),
        }
    })
}

/// The Iced subscription that turns the core's events into messages.
pub fn events_subscription(slot: EventsSlot) -> Subscription<Message> {
    Subscription::run_with(slot, events_stream).map(|event| match event {
        BridgeEvent::Core(event) => Message::Core(Box::new(event)),
        BridgeEvent::Closed => Message::CoreClosed,
    })
}

/// A command that did not fit in the channel yet.
#[derive(Debug)]
pub struct Pending {
    commands: mpsc::Sender<Command>,
    // Boxed: `Command` is large, and this is the `Err` of `try_send`.
    command: Box<Command>,
}

impl Pending {
    /// Waits for room in the channel and queues the command. This is a
    /// channel send, not network work.
    pub async fn deliver(self) {
        // An error means the core is gone; there is nobody to tell.
        let _ = self.commands.send(*self.command).await;
    }

    /// The same, as an Iced task that produces no message.
    pub fn into_task(self) -> Task<Message> {
        Task::future(self.deliver()).discard()
    }
}

/// Queues `command` without waiting. A full channel gives the command back as
/// a [`Pending`] to deliver later; a closed one drops it.
pub fn try_send(commands: &mpsc::Sender<Command>, command: Command) -> Result<(), Pending> {
    match commands.try_send(command) {
        Ok(()) | Err(TrySendError::Closed(_)) => Ok(()),
        Err(TrySendError::Full(command)) => Err(Pending {
            commands: commands.clone(),
            command: Box::new(command),
        }),
    }
}

/// The running core and the runtime it lives on.
#[derive(Debug)]
pub struct Bridge {
    commands: mpsc::Sender<Command>,
    handle: Handle,
    runtime: Option<Runtime>,
}

impl Bridge {
    /// Builds the core's runtime and starts the core on it.
    ///
    /// # Errors
    ///
    /// Fails if the operating system refuses to create the runtime's threads.
    pub fn start(
        config: CoreConfig,
        secrets: Arc<dyn SecretStore>,
    ) -> io::Result<(Self, EventsSlot)> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("tessera-core")
            .build()?;
        let core = {
            let _guard = runtime.enter();
            Core::start(config, secrets)
        };
        let bridge = Self {
            commands: core.commands,
            handle: runtime.handle().clone(),
            runtime: Some(runtime),
        };
        Ok((bridge, EventsSlot::new(core.events)))
    }

    /// Queues a command without blocking. `None` when it is queued; `Some`
    /// task when the channel was full (the task queues it later).
    pub fn send(&self, command: Command) -> Option<Task<Message>> {
        try_send(&self.commands, command)
            .err()
            .map(Pending::into_task)
    }

    /// A task that yields [`Message::ShutdownDeadline`] after `after`. The
    /// timer runs on the core's runtime.
    pub fn deadline(&self, after: Duration) -> Task<Message> {
        self.timer(after, Message::ShutdownDeadline)
    }

    /// A task that yields `message` after `after`. The timer runs on the
    /// core's runtime.
    pub fn timer(&self, after: Duration, message: Message) -> Task<Message> {
        let timer = spawn_timer(&self.handle, after);
        Task::future(async move {
            // The timer ends either way (done, or runtime gone).
            let _ = timer.await;
            message
        })
    }
}

/// Starts a timer on `handle`'s runtime from any thread. The `sleep` future is
/// created inside the spawned task, because creating it needs a runtime
/// context, which the calling (UI) thread does not have.
fn spawn_timer(handle: &Handle, after: Duration) -> tokio::task::JoinHandle<()> {
    handle.spawn(async move { tokio::time::sleep(after).await })
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use iced::futures::StreamExt as _;
    use tessera_core::api::SyncState;

    use super::*;

    fn slot_with(events: Vec<CoreEvent>) -> EventsSlot {
        let (tx, rx) = mpsc::channel(events.len().max(1));
        for event in events {
            tx.try_send(event).expect("channel has room");
        }
        EventsSlot::new(rx)
    }

    fn collect(slot: &EventsSlot) -> Vec<BridgeEvent> {
        iced::futures::executor::block_on(events_stream(slot).collect())
    }

    #[test]
    fn stream_yields_events_in_order_then_closed() {
        let slot = slot_with(vec![
            CoreEvent::NoSession,
            CoreEvent::SyncState(SyncState::Running),
        ]);
        let got = collect(&slot);
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(matches!(got[0], BridgeEvent::Core(CoreEvent::NoSession)));
        assert!(matches!(
            got[1],
            BridgeEvent::Core(CoreEvent::SyncState(SyncState::Running))
        ));
        assert!(matches!(got[2], BridgeEvent::Closed));
    }

    #[test]
    fn second_build_yields_an_empty_stream() {
        let slot = slot_with(vec![CoreEvent::NoSession]);
        assert_eq!(collect(&slot).len(), 2);
        assert!(collect(&slot).is_empty());
        // A clone shares the same (taken) receiver.
        assert!(collect(&slot.clone()).is_empty());
    }

    #[test]
    fn slots_hash_alike() {
        use std::hash::{BuildHasher, RandomState};
        let state = RandomState::new();
        let a = slot_with(vec![]);
        let b = slot_with(vec![CoreEvent::NoSession]);
        assert_eq!(state.hash_one(&a), state.hash_one(&b));
    }

    #[test]
    fn full_channel_gives_the_command_back_and_delivers_later() {
        let (tx, mut rx) = mpsc::channel(1);
        assert!(try_send(&tx, Command::RestoreSession).is_ok());
        let started = Instant::now();
        let pending = try_send(&tx, Command::Shutdown).expect_err("channel is full");
        assert!(started.elapsed() < Duration::from_millis(50));
        let first = rx.try_recv().expect("first command");
        assert!(matches!(first, Command::RestoreSession));
        iced::futures::executor::block_on(pending.deliver());
        assert!(matches!(rx.try_recv(), Ok(Command::Shutdown)));
    }

    #[test]
    fn timer_can_be_started_from_a_thread_without_a_runtime() {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let timer = spawn_timer(runtime.handle(), Duration::from_millis(10));
        runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), timer).await })
            .expect("the timer ends in time")
            .expect("the timer task does not panic");
    }

    #[test]
    fn closed_channel_drops_the_command() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        assert!(try_send(&tx, Command::Shutdown).is_ok());
    }
}
