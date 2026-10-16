//! Screen sharing through the ScreenCast portal (plan Task 8, Q9): the
//! system's own picker, then the chosen monitor or window as a PipeWire
//! stream ([`super::pw_video`]) on the remote the portal opens for us.
//!
//! The steps (create a session, say what may be picked, start, which shows
//! the picker, open the remote) are a small state machine over
//! [`Portal`], so it is tested with a fake portal. The cursor is embedded
//! in the frames when the portal offers it. "Remember this choice" is
//! opt-in: only then is the portal asked to persist the choice, and the
//! restore token it hands back is given to the caller to keep (it is a
//! credential for that choice: never logged).

use std::fmt;
use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
};
use ashpd::desktop::{PersistMode, ResponseError, Session};

use super::pw_video::{self, StreamOptions, VideoStream};
use super::{CaptureError, FrameSink};

/// What the person may pick and how the stream is recorded.
#[derive(Clone, PartialEq, Eq)]
pub struct ShareRequest {
    /// Ask the portal to remember the choice; with a `restore_token`,
    /// share that choice again without showing the picker.
    pub remember: bool,
    /// A token from an earlier share that was remembered.
    pub restore_token: Option<String>,
    /// Frames larger than this are scaled down to fit.
    pub max_size: (u32, u32),
    /// Frames beyond this rate are dropped.
    pub max_fps: u32,
}

impl fmt::Debug for ShareRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShareRequest")
            .field("remember", &self.remember)
            .field(
                "restore_token",
                &self.restore_token.as_ref().map(|_| "<set>"),
            )
            .field("max_size", &self.max_size)
            .field("max_fps", &self.max_fps)
            .finish()
    }
}

/// What `select_sources` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    /// Monitors and windows.
    pub windows: bool,
    /// The cursor drawn into the frames (else left out).
    pub embed_cursor: bool,
    /// Ask the portal to keep the choice until revoked.
    pub persist: bool,
    /// Reuse an earlier choice.
    pub restore_token: Option<String>,
}

/// One stream the portal started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Started {
    pub node: u32,
    /// Its size in physical pixels, when the portal says.
    pub size: Option<(u32, u32)>,
}

/// The portal calls the state machine makes; the real one is
/// [`DesktopPortal`].
pub(crate) trait Portal {
    type Session;
    async fn embedded_cursor(&self) -> bool;
    async fn create_session(&self) -> Result<Self::Session, CaptureError>;
    async fn select_sources(
        &self,
        session: &Self::Session,
        selection: Selection,
    ) -> Result<(), CaptureError>;
    /// Shows the picker; the streams and a restore token.
    async fn start(
        &self,
        session: &Self::Session,
    ) -> Result<(Vec<Started>, Option<String>), CaptureError>;
    async fn open_remote(&self, session: &Self::Session) -> Result<OwnedFd, CaptureError>;
    async fn close(&self, session: &Self::Session);
}

/// What a successful negotiation gives.
pub(crate) struct Negotiated<S> {
    pub session: S,
    pub stream: Started,
    pub remote: OwnedFd,
    /// Present only when the choice was to be remembered.
    pub restore_token: Option<String>,
}

/// Runs the portal's steps. The session is closed again on every failure
/// after it was made.
pub(crate) async fn negotiate<P: Portal>(
    portal: &P,
    request: &ShareRequest,
) -> Result<Negotiated<P::Session>, CaptureError> {
    let embed_cursor = portal.embedded_cursor().await;
    let session = portal.create_session().await?;
    let result = async {
        let selection = Selection {
            windows: true,
            embed_cursor,
            persist: request.remember,
            restore_token: request
                .restore_token
                .clone()
                .filter(|_| request.remember)
                .filter(|t| !t.is_empty()),
        };
        portal.select_sources(&session, selection).await?;
        let (streams, token) = portal.start(&session).await?;
        // We asked for one source; a portal that gives several gets its first.
        let stream = *streams
            .first()
            .ok_or(CaptureError::Failed("the portal shared nothing"))?;
        let remote = portal.open_remote(&session).await?;
        Ok((stream, remote, token))
    }
    .await;
    match result {
        Ok((stream, remote, token)) => Ok(Negotiated {
            session,
            stream,
            remote,
            restore_token: token.filter(|_| request.remember),
        }),
        Err(e) => {
            portal.close(&session).await;
            Err(e)
        }
    }
}

fn portal_error(e: ashpd::Error) -> CaptureError {
    match e {
        ashpd::Error::Response(ResponseError::Cancelled) => CaptureError::Cancelled,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::Zbus(_) => {
            CaptureError::Unavailable("no screen-cast portal")
        }
        ashpd::Error::RequiresVersion(_, _) => {
            CaptureError::Unavailable("screen-cast portal too old")
        }
        _ => CaptureError::Failed("the screen-cast portal refused"),
    }
}

/// The desktop's ScreenCast portal over D-Bus.
pub(crate) struct DesktopPortal(Screencast);

impl DesktopPortal {
    async fn connect() -> Result<Self, CaptureError> {
        Screencast::new().await.map(Self).map_err(portal_error)
    }
}

impl Portal for DesktopPortal {
    type Session = Session<Screencast>;

    async fn embedded_cursor(&self) -> bool {
        self.0
            .available_cursor_modes()
            .await
            .is_ok_and(|modes| modes.contains(CursorMode::Embedded))
    }

    async fn create_session(&self) -> Result<Self::Session, CaptureError> {
        self.0
            .create_session(Default::default())
            .await
            .map_err(portal_error)
    }

    async fn select_sources(
        &self,
        session: &Self::Session,
        selection: Selection,
    ) -> Result<(), CaptureError> {
        let mut types: ashpd::enumflags2::BitFlags<SourceType> = SourceType::Monitor.into();
        if selection.windows {
            types |= SourceType::Window;
        }
        let options = SelectSourcesOptions::default()
            .set_sources(types)
            .set_multiple(false)
            .set_cursor_mode(if selection.embed_cursor {
                CursorMode::Embedded
            } else {
                CursorMode::Hidden
            })
            .set_persist_mode(if selection.persist {
                PersistMode::ExplicitlyRevoked
            } else {
                PersistMode::DoNot
            })
            .set_restore_token(selection.restore_token.as_deref());
        self.0
            .select_sources(session, options)
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)
    }

    async fn start(
        &self,
        session: &Self::Session,
    ) -> Result<(Vec<Started>, Option<String>), CaptureError> {
        let streams = self
            .0
            .start(session, None, StartCastOptions::default())
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;
        let started = streams
            .streams()
            .iter()
            .map(|s| Started {
                node: s.pipe_wire_node_id(),
                size: s
                    .size()
                    .and_then(|(w, h)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?))),
            })
            .collect();
        Ok((started, streams.restore_token().map(str::to_owned)))
    }

    async fn open_remote(&self, session: &Self::Session) -> Result<OwnedFd, CaptureError> {
        self.0
            .open_pipe_wire_remote(session, Default::default())
            .await
            .map_err(portal_error)
    }

    async fn close(&self, session: &Self::Session) {
        let _ = session.close().await;
    }
}

/// A screen share from the portal: its session and its stream. Dropping
/// it stops the frames; [`ScreenShare::close`] also ends the portal session
/// (the compositor's "sharing" indicator goes away).
pub struct ScreenShare {
    portal: DesktopPortal,
    session: Session<Screencast>,
    stream: Option<VideoStream>,
    size: Option<(u32, u32)>,
    restore_token: Option<String>,
}

impl fmt::Debug for ScreenShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenShare")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl ScreenShare {
    /// Shows the system's picker (or reuses a remembered choice) and starts
    /// capturing what was picked; frames go to `sink`. Must run inside a
    /// Tokio runtime (the portal is reached over D-Bus).
    ///
    /// # Errors
    /// [`CaptureError::Cancelled`] when the person closes the picker,
    /// [`CaptureError::Unavailable`] without a ScreenCast portal,
    /// [`CaptureError::Failed`] for the rest.
    pub async fn start(
        request: &ShareRequest,
        sink: Box<dyn FrameSink>,
    ) -> Result<Self, CaptureError> {
        let portal = DesktopPortal::connect().await?;
        let negotiated = negotiate(&portal, request).await?;
        let options = StreamOptions {
            remote: Some(negotiated.remote),
            node: Some(negotiated.stream.node),
            max_size: request.max_size,
            max_fps: request.max_fps,
            dma_buf: true,
        };
        let stream = match pw_video::open(options, sink) {
            Ok(stream) => stream,
            Err(e) => {
                portal.close(&negotiated.session).await;
                return Err(e);
            }
        };
        Ok(Self {
            portal,
            session: negotiated.session,
            stream: Some(stream),
            size: negotiated.stream.size,
            restore_token: negotiated.restore_token,
        })
    }

    /// The shared monitor's or window's size in physical pixels, when the
    /// portal said.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// The token that shares the same choice again without the picker,
    /// when remembering was asked for and the portal gave one.
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// Stops the frames (joining the capture thread: call it off the async
    /// threads' hot path, it takes up to a frame) and closes the portal
    /// session.
    pub async fn close(mut self) {
        drop(self.stream.take());
        self.portal.close(&self.session).await;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A portal that records what it was asked and answers from a script.
    #[derive(Default)]
    struct Fake {
        cursor: bool,
        cancel: bool,
        no_streams: bool,
        token: Option<&'static str>,
        selected: RefCell<Option<Selection>>,
        closed: RefCell<u32>,
    }

    impl Portal for Fake {
        type Session = ();

        async fn embedded_cursor(&self) -> bool {
            self.cursor
        }

        async fn create_session(&self) -> Result<(), CaptureError> {
            Ok(())
        }

        async fn select_sources(&self, _: &(), selection: Selection) -> Result<(), CaptureError> {
            *self.selected.borrow_mut() = Some(selection);
            Ok(())
        }

        async fn start(&self, _: &()) -> Result<(Vec<Started>, Option<String>), CaptureError> {
            if self.cancel {
                return Err(CaptureError::Cancelled);
            }
            let streams = if self.no_streams {
                Vec::new()
            } else {
                vec![
                    Started {
                        node: 42,
                        size: Some((2560, 1440)),
                    },
                    Started {
                        node: 43,
                        size: None,
                    },
                ]
            };
            Ok((streams, self.token.map(str::to_owned)))
        }

        async fn open_remote(&self, _: &()) -> Result<OwnedFd, CaptureError> {
            std::fs::File::open("/dev/null")
                .map(OwnedFd::from)
                .map_err(|_| CaptureError::Failed("test"))
        }

        async fn close(&self, _: &()) {
            *self.closed.borrow_mut() += 1;
        }
    }

    fn request(remember: bool, token: Option<&str>) -> ShareRequest {
        ShareRequest {
            remember,
            restore_token: token.map(str::to_owned),
            max_size: (1920, 1080),
            max_fps: 30,
        }
    }

    #[tokio::test]
    async fn a_share_takes_the_first_stream_with_the_cursor_drawn_in() {
        let fake = Fake {
            cursor: true,
            ..Fake::default()
        };
        let n = negotiate(&fake, &request(false, None))
            .await
            .expect("shared");
        assert_eq!(n.stream.node, 42);
        assert_eq!(n.stream.size, Some((2560, 1440)));
        let selected = fake.selected.borrow().clone().expect("selected");
        assert!(selected.embed_cursor && selected.windows);
        assert!(!selected.persist);
        assert_eq!(*fake.closed.borrow(), 0);
    }

    #[tokio::test]
    async fn without_an_embedded_cursor_mode_the_cursor_is_left_out() {
        let fake = Fake::default();
        negotiate(&fake, &request(false, None))
            .await
            .expect("shared");
        assert!(
            !fake
                .selected
                .borrow()
                .as_ref()
                .expect("selected")
                .embed_cursor
        );
    }

    #[tokio::test]
    async fn a_cancelled_picker_closes_the_session_and_says_so() {
        let fake = Fake {
            cancel: true,
            ..Fake::default()
        };
        let err = negotiate(&fake, &request(false, None)).await.err();
        assert_eq!(err, Some(CaptureError::Cancelled));
        assert_eq!(*fake.closed.borrow(), 1);
    }

    #[tokio::test]
    async fn a_portal_that_shares_nothing_is_an_error() {
        let fake = Fake {
            no_streams: true,
            ..Fake::default()
        };
        assert!(matches!(
            negotiate(&fake, &request(false, None)).await,
            Err(CaptureError::Failed(_))
        ));
        assert_eq!(*fake.closed.borrow(), 1);
    }

    #[tokio::test]
    async fn the_choice_is_remembered_only_when_asked() {
        // Asked: persisted, the old token reused, the new one handed back.
        let fake = Fake {
            token: Some("new-token"),
            ..Fake::default()
        };
        let n = negotiate(&fake, &request(true, Some("old-token")))
            .await
            .expect("shared");
        let selected = fake.selected.borrow().clone().expect("selected");
        assert!(selected.persist);
        assert_eq!(selected.restore_token.as_deref(), Some("old-token"));
        assert_eq!(n.restore_token.as_deref(), Some("new-token"));

        // Not asked: no persistence, a stale token is not sent, and a token
        // the portal gives anyway is dropped.
        let fake = Fake {
            token: Some("unasked"),
            ..Fake::default()
        };
        let n = negotiate(&fake, &request(false, Some("old-token")))
            .await
            .expect("shared");
        let selected = fake.selected.borrow().clone().expect("selected");
        assert!(!selected.persist);
        assert_eq!(selected.restore_token, None);
        assert_eq!(n.restore_token, None);
    }

    #[test]
    fn the_restore_token_is_not_in_debug_output() {
        let text = format!("{:?}", request(true, Some("secret-token")));
        assert!(!text.contains("secret-token"), "{text}");
    }

    #[test]
    fn portal_errors_map_to_what_the_person_did() {
        assert_eq!(
            portal_error(ashpd::Error::Response(ResponseError::Cancelled)),
            CaptureError::Cancelled
        );
        assert!(matches!(
            portal_error(ashpd::Error::Response(ResponseError::Other)),
            CaptureError::Failed(_)
        ));
        assert!(matches!(
            portal_error(ashpd::Error::RequiresVersion(4, 2)),
            CaptureError::Unavailable(_)
        ));
    }
}
