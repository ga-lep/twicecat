//! The Wayland source: the xdg-desktop-portal GlobalShortcuts portal (KDE
//! Plasma; GNOME and Hyprland in recent versions). The compositor owns the binding: we
//! suggest a trigger, the user confirms or changes it in the portal's
//! dialog, and the portal reports presses (`Activated`) and releases
//! (`Deactivated`).

use std::sync::Arc;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use tokio::sync::oneshot;

use crate::{APP_ID, PttEvent, Shared, Source};

/// Our shortcut's ID in the portal (kept by the compositor across runs).
const SHORTCUT_ID: &str = "push-to-talk";

/// The running portal session; dropping it closes the session.
pub(crate) struct Portal {
    stop: Option<oneshot::Sender<()>>,
}

impl Portal {
    pub(crate) fn start(
        trigger: Option<String>,
        shared: Arc<Shared>,
        runtime: &tokio::runtime::Handle,
    ) -> Self {
        let (stop, stopped) = oneshot::channel();
        runtime.spawn(async move {
            if let Err(e) = run(trigger, &shared, stopped).await {
                shared.notify(PttEvent::Unavailable {
                    source: Source::Portal,
                    reason: e.to_string(),
                });
            }
            shared.report(Source::Portal, false);
        });
        Self { stop: Some(stop) }
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        // The task closes the session, lets go and ends on its own.
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn run(
    trigger: Option<String>,
    shared: &Shared,
    mut stopped: oneshot::Receiver<()>,
) -> Result<(), ashpd::Error> {
    // A host (unsandboxed) app names itself so the portal can keep its
    // shortcuts; it only works as the first portal call on the connection,
    // so a failure is not fatal (the portal may then still work, or refuse).
    if let Ok(app_id) = ashpd::AppID::try_from(APP_ID)
        && let Err(e) = ashpd::register_host_app(app_id).await
    {
        tracing::debug!("portal host app registration failed: {e}");
    }
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;
    let mut activated = portal.receive_activated().await?;
    let mut deactivated = portal.receive_deactivated().await?;
    let shortcut =
        NewShortcut::new(SHORTCUT_ID, "Push to talk").preferred_trigger(trigger.as_deref());
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    let description = bound
        .shortcuts()
        .iter()
        .find(|s| s.id() == SHORTCUT_ID)
        .map(|s| s.trigger_description().to_owned())
        .unwrap_or_default();
    shared.notify(PttEvent::Ready {
        source: Source::Portal,
        trigger: description,
    });
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            Some(event) = activated.next() => {
                if event.shortcut_id() == SHORTCUT_ID {
                    shared.report(Source::Portal, true);
                }
            }
            Some(event) = deactivated.next() => {
                if event.shortcut_id() == SHORTCUT_ID {
                    shared.report(Source::Portal, false);
                }
            }
            else => break,
        }
    }
    let _ = session.close().await;
    Ok(())
}
