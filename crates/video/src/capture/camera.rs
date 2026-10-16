//! Cameras on Linux (plan Task 9, Q10): PipeWire's camera nodes (`Video/Source`
//! with the `Camera` role, which the session manager makes for V4L2 and
//! libcamera devices), their modes from the formats they enumerate, and a
//! stream on one of them in one mode ([`super::pw_video`]).
//!
//! Inside a Flatpak sandbox the cameras are reached through the Camera
//! portal, whose PipeWire remote shows only them ([`remote`]); outside, the
//! session's PipeWire daemon is asked directly, as the portal would add a
//! permission question for nothing. A camera's identifier is its node name:
//! stable across restarts, but it can hold the device's serial, so it is
//! never logged.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::context::ContextRc;
use pw::core::{CoreRc, PW_ID_CORE};
use pw::main_loop::MainLoopRc;
use pw::node::{Node, NodeListener};
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::video::VideoFormat;
use pw::spa::param::{ParamInfoFlags, ParamType};
use pw::spa::pod::deserialize::PodDeserializer;
use pw::spa::pod::{ChoiceValue, Object, Value};
use pw::spa::utils::result::AsyncSeq;
use pw::spa::utils::{Choice, ChoiceEnum, Fraction, Rectangle};
use pw::types::ObjectType;

use super::pw_video::{self, Offer, StreamOptions, Target, VideoStream};
use super::{CaptureError, FrameSink};

/// How long listing may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(3);

/// A camera's pixel format, as it sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CameraFormat {
    /// NV12 (raw 4:2:0).
    Nv12,
    /// YUYV, packed 4:2:2 (raw).
    Yuyv,
    /// Motion JPEG (decoded on our side).
    Mjpeg,
}

/// One way a camera can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CameraMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
    /// The pixel format.
    pub format: CameraFormat,
}

/// A camera.
#[derive(Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// Identifier for [`Camera::open`] (the node's name).
    pub id: String,
    /// What people call it.
    pub name: String,
    /// The system's preferred camera.
    pub is_default: bool,
    /// Its modes, largest first, then fastest, raw formats before MJPEG.
    pub modes: Vec<CameraMode>,
}

impl std::fmt::Debug for CameraInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraInfo")
            .field("id", &format_args!("<{} bytes>", self.id.len()))
            .field("is_default", &self.is_default)
            .field("modes", &self.modes.len())
            .finish_non_exhaustive()
    }
}

/// Whether this process runs in a Flatpak sandbox.
fn in_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// The PipeWire remote cameras are reached on: the Camera portal's inside a
/// Flatpak sandbox (asking for access, once per call; the portal remembers
/// the answer), `None` (the session's daemon) outside. Must run inside a
/// Tokio runtime. Each listing and each camera opened takes its own remote.
///
/// # Errors
/// [`CaptureError::Unavailable`] when the sandbox has no Camera portal or
/// access is refused.
pub async fn remote() -> Result<Option<OwnedFd>, CaptureError> {
    if !in_flatpak() {
        return Ok(None);
    }
    use ashpd::desktop::camera::Camera as Portal;
    let portal = Portal::new()
        .await
        .map_err(|_| CaptureError::Unavailable("no camera portal"))?;
    portal
        .request_access(Default::default())
        .await
        .and_then(|request| request.response())
        .map_err(|_| CaptureError::Unavailable("camera access was refused"))?;
    portal
        .open_pipe_wire_remote(Default::default())
        .await
        .map(Some)
        .map_err(|_| CaptureError::Unavailable("no camera portal"))
}

/// Whether a node's properties make it a camera.
fn is_camera(get: impl Fn(&str) -> Option<String>) -> bool {
    get("media.class").as_deref() == Some("Video/Source")
        && (get("media.role").as_deref() == Some("Camera")
            || matches!(get("device.api").as_deref(), Some("v4l2" | "libcamera")))
}

/// What one camera node said.
#[derive(Default)]
struct Found {
    name: String,
    label: String,
    priority: i64,
    formats: Vec<Object>,
}

fn fps_of(f: Fraction) -> Option<u32> {
    (f.denom > 0 && f.num > 0).then(|| (f.num + f.denom / 2) / f.denom)
}

/// The values a property allows: a plain value, or a choice's.
fn choices<T: Copy>(
    value: &Value,
    plain: impl Fn(&Value) -> Option<T>,
    choice: impl Fn(&ChoiceValue) -> Option<Vec<T>>,
) -> Vec<T> {
    match value {
        Value::Choice(c) => choice(c).unwrap_or_default(),
        v => plain(v).into_iter().collect(),
    }
}

fn list_of<T: Copy + pw::spa::pod::CanonicalFixedSizedPod>(choice: &Choice<T>) -> Vec<T> {
    match &choice.1 {
        ChoiceEnum::None(v) => vec![*v],
        ChoiceEnum::Enum {
            default,
            alternatives,
        } => std::iter::once(*default)
            .chain(alternatives.iter().copied())
            .collect(),
        // A range: its ends (and its default) stand for it.
        ChoiceEnum::Range { default, min, max }
        | ChoiceEnum::Step {
            default, min, max, ..
        } => {
            vec![*max, *default, *min]
        }
        ChoiceEnum::Flags { default, .. } => vec![*default],
    }
}

/// The modes one enumerated format allows.
fn modes_in(object: &Object) -> Vec<CameraMode> {
    let get = |key: FormatProperties| {
        object
            .properties
            .iter()
            .find(|p| p.key == key.as_raw())
            .map(|p| &p.value)
    };
    let id = |v: &Value| match v {
        Value::Id(id) => Some(id.0),
        _ => None,
    };
    if get(FormatProperties::MediaType).and_then(id) != Some(MediaType::Video.as_raw()) {
        return Vec::new();
    }
    let formats: Vec<CameraFormat> = match get(FormatProperties::MediaSubtype).and_then(id) {
        Some(s) if s == MediaSubtype::Mjpg.as_raw() => vec![CameraFormat::Mjpeg],
        Some(s) if s == MediaSubtype::Raw.as_raw() => {
            let Some(value) = get(FormatProperties::VideoFormat) else {
                return Vec::new();
            };
            choices(value, id, |c| match c {
                ChoiceValue::Id(c) => Some(list_of(c).into_iter().map(|i| i.0).collect()),
                _ => None,
            })
            .into_iter()
            .filter_map(|f| match VideoFormat::from_raw(f) {
                VideoFormat::NV12 => Some(CameraFormat::Nv12),
                VideoFormat::YUY2 => Some(CameraFormat::Yuyv),
                _ => None,
            })
            .collect()
        }
        _ => return Vec::new(),
    };
    let sizes: Vec<Rectangle> = get(FormatProperties::VideoSize).map_or_else(Vec::new, |v| {
        choices(
            v,
            |v| match v {
                Value::Rectangle(r) => Some(*r),
                _ => None,
            },
            |c| match c {
                ChoiceValue::Rectangle(c) => Some(list_of(c)),
                _ => None,
            },
        )
    });
    let rates: Vec<u32> = get(FormatProperties::VideoFramerate).map_or_else(Vec::new, |v| {
        choices(
            v,
            |v| match v {
                Value::Fraction(f) => Some(*f),
                _ => None,
            },
            |c| match c {
                ChoiceValue::Fraction(c) => Some(list_of(c)),
                _ => None,
            },
        )
        .into_iter()
        .filter_map(fps_of)
        .collect()
    });
    let mut out = Vec::new();
    for &format in &formats {
        for size in &sizes {
            if size.width < 2 || size.height < 2 || size.width > 8192 || size.height > 8192 {
                continue;
            }
            for &fps in &rates {
                if (1..=240).contains(&fps) {
                    out.push(CameraMode {
                        width: size.width,
                        height: size.height,
                        fps,
                        format,
                    });
                }
            }
        }
    }
    out
}

/// Every mode the enumerated formats allow, largest first, then fastest,
/// raw formats before MJPEG; no repeats.
fn modes_of(objects: &[Object]) -> Vec<CameraMode> {
    let mut modes: Vec<CameraMode> = objects.iter().flat_map(modes_in).collect();
    modes.sort_by(|a, b| {
        (u64::from(b.width) * u64::from(b.height))
            .cmp(&(u64::from(a.width) * u64::from(a.height)))
            .then(b.fps.cmp(&a.fps))
            .then(a.format.cmp(&b.format))
    });
    modes.dedup();
    modes
}

/// The mode to open a camera in, to send `target` (width, height, fps):
/// `wanted` when the camera has it, else the smallest that covers the
/// target's size at its rate or more, else the largest at its rate, else the
/// largest and fastest. At one size and rate, `format` when the camera has
/// it, else raw formats before MJPEG (no decoding). `None` without modes.
pub fn choose_mode(
    modes: &[CameraMode],
    target: (u32, u32, u32),
    wanted: Option<(u32, u32, u32)>,
    format: Option<CameraFormat>,
) -> Option<CameraMode> {
    let (tw, th, tfps) = target;
    let area = |m: &CameraMode| u64::from(m.width) * u64::from(m.height);
    let best_format = |w: u32, h: u32, fps: u32| {
        let at: Vec<&CameraMode> = modes
            .iter()
            .filter(|m| (m.width, m.height, m.fps) == (w, h, fps))
            .collect();
        at.iter()
            .find(|m| Some(m.format) == format)
            .or_else(|| at.iter().min_by_key(|m| m.format))
            .map(|m| **m)
    };
    if let Some((w, h, fps)) = wanted
        && let Some(mode) = best_format(w, h, fps)
    {
        return Some(mode);
    }
    let pick = |m: &CameraMode| best_format(m.width, m.height, m.fps);
    // Covers the size, at the rate: the smallest such, the slowest rate
    // that is enough.
    if let Some(m) = modes
        .iter()
        .filter(|m| m.width >= tw && m.height >= th && m.fps >= tfps)
        .min_by_key(|m| (area(m), m.fps))
    {
        return pick(m);
    }
    // At the rate, as large as it goes.
    if let Some(m) = modes
        .iter()
        .filter(|m| m.fps >= tfps)
        .max_by_key(|m| (area(m), std::cmp::Reverse(m.fps)))
    {
        return pick(m);
    }
    modes.iter().max_by_key(|m| (area(m), m.fps)).and_then(pick)
}

/// A connection to PipeWire on the current thread.
struct Session {
    mainloop: MainLoopRc,
    _context: ContextRc,
    core: CoreRc,
}

impl Session {
    fn connect(remote: Option<OwnedFd>) -> Result<Self, CaptureError> {
        pw::init();
        let mainloop =
            MainLoopRc::new(None).map_err(|_| CaptureError::Failed("no PipeWire main loop"))?;
        let context = ContextRc::new(&mainloop, None)
            .map_err(|_| CaptureError::Failed("no PipeWire context"))?;
        let core = match remote {
            Some(fd) => context.connect_fd_rc(fd, None),
            None => context.connect_rc(None),
        }
        .map_err(|_| CaptureError::Unavailable("no PipeWire daemon"))?;
        Ok(Self {
            mainloop,
            _context: context,
            core,
        })
    }
}

/// Waits for the server to answer everything asked so far: each `sync` adds
/// a sequence; the loop quits when all came back.
struct Roundtrips {
    pending: Rc<RefCell<Vec<AsyncSeq>>>,
    _listener: pw::core::Listener,
}

impl Roundtrips {
    fn new(mainloop: &MainLoopRc, core: &CoreRc) -> Self {
        let pending: Rc<RefCell<Vec<AsyncSeq>>> = Rc::default();
        let mainloop = mainloop.clone();
        let listener = core
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

/// Runs the loop until the round trips are done, at most [`LIST_TIMEOUT`];
/// `false` on a timeout.
fn run_until_done(mainloop: &MainLoopRc) -> bool {
    let timed_out = Rc::new(RefCell::new(false));
    let timer = mainloop.loop_().add_timer({
        let mainloop = mainloop.clone();
        let timed_out = timed_out.clone();
        move |_| {
            *timed_out.borrow_mut() = true;
            mainloop.quit();
        }
    });
    let _ = timer.update_timer(Some(LIST_TIMEOUT), None);
    mainloop.run();
    !*timed_out.borrow()
}

/// The id of the video source node named `name` (a camera's identifier),
/// asked on a connection the caller made; `None` when there is none.
pub(super) fn node_named(mainloop: &MainLoopRc, core: &CoreRc, name: &str) -> Option<u32> {
    let registry = core.get_registry_rc().ok()?;
    let found: Rc<RefCell<Option<u32>>> = Rc::default();
    let _listener = registry
        .add_listener_local()
        .global({
            let found = found.clone();
            let name = name.to_owned();
            move |global| {
                if global.type_ == ObjectType::Node
                    && let Some(props) = global.props
                    && props.get("node.name") == Some(name.as_str())
                    && props.get("media.class") == Some("Video/Source")
                {
                    *found.borrow_mut() = Some(global.id);
                }
            }
        })
        .register();
    let roundtrips = Roundtrips::new(mainloop, core);
    roundtrips.sync(core);
    if !run_until_done(mainloop) {
        return None;
    }
    *found.borrow()
}

/// The cameras now, with their modes. Blocking, a few hundred milliseconds
/// at most (three seconds before it gives up).
///
/// # Errors
/// [`CaptureError::Unavailable`] without a PipeWire daemon (or on
/// `remote`), [`CaptureError::Failed`] when it does not answer in time.
pub fn list(remote: Option<OwnedFd>) -> Result<Vec<CameraInfo>, CaptureError> {
    let session = Session::connect(remote)?;
    let registry = session
        .core
        .get_registry_rc()
        .map_err(|_| CaptureError::Failed("no PipeWire registry"))?;
    let roundtrips = Rc::new(Roundtrips::new(&session.mainloop, &session.core));
    let found: Rc<RefCell<BTreeMap<u32, Found>>> = Rc::default();
    let nodes: Rc<RefCell<Vec<(Node, NodeListener)>>> = Rc::default();
    let _registry = registry
        .add_listener_local()
        .global({
            let registry = registry.clone();
            let found = found.clone();
            let nodes = nodes.clone();
            let roundtrips = roundtrips.clone();
            let core = session.core.clone();
            move |global| {
                if global.type_ != ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else {
                    return;
                };
                let get = |k: &str| props.get(k).map(str::to_owned);
                if !is_camera(get) {
                    return;
                }
                let Some(name) = props.get("node.name") else {
                    return;
                };
                let label = props
                    .get("node.description")
                    .or_else(|| props.get("node.nick"))
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or(name)
                    .to_owned();
                let priority = props
                    .get("priority.session")
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
                found.borrow_mut().insert(
                    global.id,
                    Found {
                        name: name.to_owned(),
                        label,
                        priority,
                        formats: Vec::new(),
                    },
                );
                let Ok(node) = registry.bind::<Node, _>(global) else {
                    return;
                };
                let id = global.id;
                let found = found.clone();
                let listener = node
                    .add_listener_local()
                    .param(move |_, kind, _, _, param| {
                        if kind != ParamType::EnumFormat {
                            return;
                        }
                        let Some(param) = param else {
                            return;
                        };
                        if let Ok((_, Value::Object(object))) =
                            PodDeserializer::deserialize_any_from(param.as_bytes())
                            && let Some(camera) = found.borrow_mut().get_mut(&id)
                        {
                            camera.formats.push(object);
                        }
                    })
                    .register();
                node.enum_params(0, Some(ParamType::EnumFormat), 0, u32::MAX);
                nodes.borrow_mut().push((node, listener));
                // The formats come before this round trip ends.
                roundtrips.sync(&core);
            }
        })
        .register();
    roundtrips.sync(&session.core);
    if !run_until_done(&session.mainloop) {
        return Err(CaptureError::Failed("PipeWire did not answer"));
    }
    let found = std::mem::take(&mut *found.borrow_mut());
    Ok(cameras_from(found.into_values().collect()))
}

/// What was found, as cameras: those with a mode we read (not a webcam's
/// metadata node), named apart, the default marked.
fn cameras_from(mut found: Vec<Found>) -> Vec<CameraInfo> {
    found.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.name.cmp(&b.name)));
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    found
        .into_iter()
        .filter_map(|f| {
            let modes = modes_of(&f.formats);
            (!modes.is_empty()).then_some((f, modes))
        })
        .enumerate()
        .map(|(i, (f, modes))| {
            let n = seen.entry(f.label.clone()).or_insert(0);
            *n += 1;
            let name = if *n == 1 {
                f.label
            } else {
                format!("{} ({n})", f.label)
            };
            CameraInfo {
                id: f.name,
                name,
                is_default: i == 0,
                modes,
            }
        })
        .collect()
}

/// Says when cameras come or go, from the registry, on its own thread.
/// Dropping it stops the thread.
pub struct CameraWatcher {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for CameraWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraWatcher").finish_non_exhaustive()
    }
}

impl Drop for CameraWatcher {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl CameraWatcher {
    /// Calls `on_change` after each camera that comes (once its formats
    /// can be read) or goes; not for the ones there at start.
    ///
    /// # Errors
    /// As [`list`].
    pub fn start(
        remote: Option<OwnedFd>,
        on_change: Box<dyn Fn() + Send>,
    ) -> Result<Self, CaptureError> {
        let (stop, stopped) = pw::channel::channel::<()>();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("twicecat-cameras".into())
            .spawn(move || watch(remote, on_change, stopped, &ready_tx))
            .map_err(|_| CaptureError::Failed("no thread for the camera watcher"))?;
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
                Err(CaptureError::Failed("the camera watcher stopped"))
            }
        }
    }
}

fn watch(
    remote: Option<OwnedFd>,
    on_change: Box<dyn Fn() + Send>,
    stopped: pw::channel::Receiver<()>,
    ready: &mpsc::SyncSender<Result<(), CaptureError>>,
) {
    let session = match Session::connect(remote) {
        Ok(session) => session,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let Ok(registry) = session.core.get_registry_rc() else {
        let _ = ready.send(Err(CaptureError::Failed("no PipeWire registry")));
        return;
    };
    // Each camera node, bound to hear when its formats can be read (a new
    // node's come a moment after the node itself: listed before then, it
    // would have no mode and not be listed at all).
    let nodes: Rc<RefCell<BTreeMap<u32, (Node, NodeListener)>>> = Rc::default();
    // The cameras said so far: their formats can be read.
    let ready_cameras: Rc<RefCell<std::collections::BTreeSet<u32>>> = Rc::default();
    let started = Rc::new(RefCell::new(false));
    let on_change = Rc::new(on_change);
    let roundtrips = Rc::new(Roundtrips::new(&session.mainloop, &session.core));
    let _listener = registry
        .add_listener_local()
        .global({
            let registry = registry.clone();
            let nodes = nodes.clone();
            let ready_cameras = ready_cameras.clone();
            let started = started.clone();
            let on_change = on_change.clone();
            let roundtrips = roundtrips.clone();
            let core = session.core.clone();
            move |global| {
                if global.type_ != ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else {
                    return;
                };
                if !is_camera(|k| props.get(k).map(str::to_owned)) {
                    return;
                }
                let Ok(node) = registry.bind::<Node, _>(global) else {
                    return;
                };
                let id = global.id;
                let listener = node
                    .add_listener_local()
                    .info({
                        let ready_cameras = ready_cameras.clone();
                        let started = started.clone();
                        let on_change = on_change.clone();
                        move |info| {
                            let readable = info.params().iter().any(|p| {
                                p.id() == ParamType::EnumFormat
                                    && p.flags().contains(ParamInfoFlags::READ)
                            });
                            if readable
                                && ready_cameras.borrow_mut().insert(id)
                                && *started.borrow()
                            {
                                on_change();
                            }
                        }
                    })
                    .register();
                nodes.borrow_mut().insert(id, (node, listener));
                // The cameras there at start are known before it returns
                // (later, a round trip's end would quit the loop).
                if !*started.borrow() {
                    roundtrips.sync(&core);
                }
            }
        })
        .global_remove({
            let nodes = nodes.clone();
            let ready_cameras = ready_cameras.clone();
            let started = started.clone();
            let on_change = on_change.clone();
            move |id| {
                nodes.borrow_mut().remove(&id);
                if ready_cameras.borrow_mut().remove(&id) && *started.borrow() {
                    on_change();
                }
            }
        })
        .register();
    roundtrips.sync(&session.core);
    if !run_until_done(&session.mainloop) {
        let _ = ready.send(Err(CaptureError::Failed("PipeWire did not answer")));
        return;
    }
    *started.borrow_mut() = true;
    let _ = ready.send(Ok(()));
    let quit = session.mainloop.clone();
    let _stop = stopped.attach(session.mainloop.loop_(), move |()| quit.quit());
    session.mainloop.run();
}

/// A camera being read; dropping it stops it.
pub struct Camera {
    _stream: VideoStream,
}

impl std::fmt::Debug for Camera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Camera").finish_non_exhaustive()
    }
}

impl Camera {
    /// Opens camera `id` (from [`list`]) in `mode`; frames go to `sink`,
    /// NV12 at the camera's own size, in the colour space it says it sends.
    /// Blocking until the stream is connected.
    ///
    /// # Errors
    /// [`CaptureError::NoSuchSource`] when the camera is gone,
    /// [`CaptureError::Unavailable`] without PipeWire.
    pub fn open(
        remote: Option<OwnedFd>,
        id: &str,
        mode: CameraMode,
        sink: Box<dyn FrameSink>,
    ) -> Result<Self, CaptureError> {
        let stream = pw_video::open(
            StreamOptions {
                remote,
                target: Target::Named(id.to_owned()),
                max_size: (mode.width, mode.height),
                max_fps: mode.fps,
                offer: Offer::Camera(mode),
            },
            sink,
        )?;
        Ok(Self { _stream: stream })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pw::spa::pod::Property;
    use pw::spa::utils::{ChoiceFlags, Id, SpaTypes};

    use super::super::test_camera::{Picture, TestCamera};

    use super::*;

    fn format(subtype: MediaSubtype, raw: Option<VideoFormat>, size: Value, rate: Value) -> Object {
        let mut properties = vec![
            Property::new(
                FormatProperties::MediaType.as_raw(),
                Value::Id(Id(MediaType::Video.as_raw())),
            ),
            Property::new(
                FormatProperties::MediaSubtype.as_raw(),
                Value::Id(Id(subtype.as_raw())),
            ),
        ];
        if let Some(raw) = raw {
            properties.push(Property::new(
                FormatProperties::VideoFormat.as_raw(),
                Value::Id(Id(raw.as_raw())),
            ));
        }
        properties.push(Property::new(FormatProperties::VideoSize.as_raw(), size));
        properties.push(Property::new(
            FormatProperties::VideoFramerate.as_raw(),
            rate,
        ));
        Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties,
        }
    }

    fn size(width: u32, height: u32) -> Value {
        Value::Rectangle(Rectangle { width, height })
    }

    fn rates(list: &[u32]) -> Value {
        Value::Choice(ChoiceValue::Fraction(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum {
                default: Fraction {
                    num: list[0],
                    denom: 1,
                },
                alternatives: list
                    .iter()
                    .map(|&n| Fraction { num: n, denom: 1 })
                    .collect(),
            },
        )))
    }

    fn mode(width: u32, height: u32, fps: u32, format: CameraFormat) -> CameraMode {
        CameraMode {
            width,
            height,
            fps,
            format,
        }
    }

    /// As a typical USB webcam enumerates: MJPEG up to 1080p30, YUYV fast
    /// only when small.
    fn webcam() -> Vec<Object> {
        vec![
            format(MediaSubtype::Mjpg, None, size(1920, 1080), rates(&[30, 15])),
            format(MediaSubtype::Mjpg, None, size(1280, 720), rates(&[30])),
            format(
                MediaSubtype::Raw,
                Some(VideoFormat::YUY2),
                size(1920, 1080),
                rates(&[5]),
            ),
            format(
                MediaSubtype::Raw,
                Some(VideoFormat::YUY2),
                size(640, 360),
                rates(&[30]),
            ),
            // Not a format we read.
            format(
                MediaSubtype::Raw,
                Some(VideoFormat::RGB),
                size(640, 360),
                rates(&[30]),
            ),
        ]
    }

    #[test]
    fn modes_come_from_the_enumerated_formats_largest_first() {
        let modes = modes_of(&webcam());
        assert_eq!(
            modes,
            vec![
                mode(1920, 1080, 30, CameraFormat::Mjpeg),
                mode(1920, 1080, 15, CameraFormat::Mjpeg),
                mode(1920, 1080, 5, CameraFormat::Yuyv),
                mode(1280, 720, 30, CameraFormat::Mjpeg),
                mode(640, 360, 30, CameraFormat::Yuyv),
            ]
        );
    }

    #[test]
    fn fractional_rates_round_and_ranges_give_their_ends() {
        let object = format(
            MediaSubtype::Raw,
            Some(VideoFormat::NV12),
            Value::Choice(ChoiceValue::Rectangle(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Rectangle {
                        width: 1280,
                        height: 720,
                    },
                    min: Rectangle {
                        width: 160,
                        height: 120,
                    },
                    max: Rectangle {
                        width: 3840,
                        height: 2160,
                    },
                },
            ))),
            Value::Fraction(Fraction {
                num: 30000,
                denom: 1001,
            }),
        );
        let modes = modes_of(&[object]);
        assert_eq!(
            modes.iter().map(|m| (m.width, m.fps)).collect::<Vec<_>>(),
            vec![(3840, 30), (1280, 30), (160, 30)]
        );
    }

    #[test]
    fn the_mode_covers_the_target_at_its_rate_raw_first() {
        let modes = modes_of(&webcam());
        // 720p30: the 720p MJPEG mode (no raw mode that fast at that size).
        assert_eq!(
            choose_mode(&modes, (1280, 720, 30), None, None),
            Some(mode(1280, 720, 30, CameraFormat::Mjpeg))
        );
        // 360p30: raw YUYV, no decoding.
        assert_eq!(
            choose_mode(&modes, (640, 360, 30), None, None),
            Some(mode(640, 360, 30, CameraFormat::Yuyv))
        );
        // 1440p60: nothing covers it nor reaches 60: the largest, fastest.
        assert_eq!(
            choose_mode(&modes, (2560, 1440, 60), None, None),
            Some(mode(1920, 1080, 30, CameraFormat::Mjpeg))
        );
        // A mode the settings name wins; one the camera lacks does not.
        assert_eq!(
            choose_mode(&modes, (1280, 720, 30), Some((1920, 1080, 5)), None),
            Some(mode(1920, 1080, 5, CameraFormat::Yuyv))
        );
        assert_eq!(
            choose_mode(&modes, (1280, 720, 30), Some((4096, 2160, 60)), None),
            Some(mode(1280, 720, 30, CameraFormat::Mjpeg))
        );
        assert_eq!(choose_mode(&[], (1280, 720, 30), None, None), None);
    }

    #[test]
    fn a_format_the_settings_name_wins_at_one_size_and_rate() {
        let modes = vec![
            mode(1280, 720, 30, CameraFormat::Nv12),
            mode(1280, 720, 30, CameraFormat::Mjpeg),
        ];
        assert_eq!(
            choose_mode(&modes, (1280, 720, 30), None, None).map(|m| m.format),
            Some(CameraFormat::Nv12)
        );
        assert_eq!(
            choose_mode(&modes, (1280, 720, 30), None, Some(CameraFormat::Mjpeg)).map(|m| m.format),
            Some(CameraFormat::Mjpeg)
        );
        // A format it lacks: the best it has.
        assert_eq!(
            choose_mode(&modes, (1280, 720, 30), None, Some(CameraFormat::Yuyv)).map(|m| m.format),
            Some(CameraFormat::Nv12)
        );
    }

    #[test]
    fn cameras_are_named_apart_and_the_preferred_is_the_default() {
        let found = vec![
            Found {
                name: "v4l2_input.b".into(),
                label: "FHD Webcam".into(),
                priority: 980,
                formats: webcam(),
            },
            Found {
                name: "v4l2_input.a".into(),
                label: "FHD Webcam".into(),
                priority: 1000,
                formats: webcam(),
            },
            // Its metadata node: no video format.
            Found {
                name: "v4l2_input.c".into(),
                label: "FHD Webcam".into(),
                priority: 990,
                formats: Vec::new(),
            },
        ];
        let cameras = cameras_from(found);
        assert_eq!(cameras.len(), 2);
        assert_eq!(cameras[0].id, "v4l2_input.a");
        assert!(cameras[0].is_default && !cameras[1].is_default);
        assert_eq!(cameras[0].name, "FHD Webcam");
        assert_eq!(cameras[1].name, "FHD Webcam (2)");
        assert!(!format!("{:?}", cameras[0]).contains("v4l2_input"));
    }

    #[test]
    fn only_camera_nodes_count() {
        let props = |pairs: &'static [(&str, &str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(is_camera(props(&[
            ("media.class", "Video/Source"),
            ("media.role", "Camera")
        ])));
        assert!(is_camera(props(&[
            ("media.class", "Video/Source"),
            ("device.api", "libcamera")
        ])));
        // A screen cast or another application's video is not a camera.
        assert!(!is_camera(props(&[("media.class", "Video/Source")])));
        assert!(!is_camera(props(&[
            ("media.class", "Stream/Output/Video"),
            ("media.role", "Camera")
        ])));
    }

    /// A JPEG frame of 640 × 360, as a camera sends it.
    const JPEG: &[u8] = include_bytes!("../../tests/fixtures/camera-360p.jpg");

    /// Hands frames and the end to channels.
    struct ToChannel {
        frames: mpsc::Sender<crate::CpuFrame>,
        ended: mpsc::Sender<super::super::CaptureEnd>,
    }

    impl FrameSink for ToChannel {
        fn frame(&mut self, frame: crate::CpuFrame) {
            let _ = self.frames.send(frame);
        }

        fn ended(&mut self, end: super::super::CaptureEnd) {
            let _ = self.ended.send(end);
        }
    }

    #[test]
    #[ignore = "needs a PipeWire daemon"]
    fn a_virtual_camera_is_listed_watched_and_read_in_each_format() {
        use crate::{Matrix, PixelFormat, Range};

        let (changed_tx, changed) = mpsc::channel();
        let watcher = CameraWatcher::start(
            None,
            Box::new(move || {
                let _ = changed_tx.send(());
            }),
        )
        .expect("watcher");

        let formats = [CameraFormat::Yuyv, CameraFormat::Nv12, CameraFormat::Mjpeg];
        for (i, format) in formats.into_iter().enumerate() {
            let name = [
                "twicecat-test-camera-0",
                "twicecat-test-camera-1",
                "twicecat-test-camera-2",
            ][i];
            let camera = TestCamera::start_with(
                name,
                (640, 360, 30),
                &formats,
                Picture::Flat { jpeg: JPEG },
            )
            .expect("the virtual camera");
            changed
                .recv_timeout(Duration::from_secs(3))
                .expect("the watcher saw it come");

            let cameras = list(None).expect("cameras");
            let info = cameras
                .iter()
                .find(|c| c.id == name)
                .expect("the virtual camera is listed");
            assert!(info.name.starts_with("Twicecat Test Camera"));
            let mode = mode(640, 360, 30, format);
            assert!(info.modes.contains(&mode), "{:?}", info.modes);

            let (frames_tx, frames) = mpsc::channel();
            let (ended_tx, ended) = mpsc::channel();
            let opened = Camera::open(
                None,
                name,
                mode,
                Box::new(ToChannel {
                    frames: frames_tx,
                    ended: ended_tx,
                }),
            )
            .expect("opens");
            let mut got = 0;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while got < 10 && std::time::Instant::now() < deadline {
                let Ok(frame) = frames.recv_timeout(Duration::from_millis(500)) else {
                    continue;
                };
                assert_eq!(frame.format(), PixelFormat::Nv12);
                assert_eq!((frame.width(), frame.height()), (640, 360));
                if format == CameraFormat::Mjpeg {
                    // Decoded as the decoder alone does it.
                    let alone = super::super::mjpeg::decode(JPEG, frame.timestamp()).unwrap();
                    assert_eq!(frame, alone);
                } else {
                    // The camera said nothing of its colours: BT.601 limited.
                    assert_eq!(frame.color().matrix, Matrix::Bt601);
                    assert_eq!(frame.color().range, Range::Limited);
                    let (y, uv) = (&frame.planes()[0], &frame.planes()[1]);
                    assert_eq!((y.data[0], uv.data[0], uv.data[1]), (81, 90, 240));
                }
                got += 1;
            }
            assert!(got >= 10, "{format:?}: only {got} frames in 5 s");

            drop(camera);
            assert_eq!(
                ended.recv_timeout(Duration::from_secs(3)),
                Ok(super::super::CaptureEnd::Stopped)
            );
            drop(opened);
            changed
                .recv_timeout(Duration::from_secs(3))
                .expect("the watcher saw it go");
        }
        drop(watcher);

        // One that is not there.
        let gone = Camera::open(
            None,
            "twicecat-no-such-camera",
            mode(640, 360, 30, CameraFormat::Yuyv),
            Box::new(ToChannel {
                frames: mpsc::channel().0,
                ended: mpsc::channel().0,
            }),
        );
        assert!(matches!(gone, Err(CaptureError::NoSuchSource)));
    }

    #[test]
    #[ignore = "needs a PipeWire daemon"]
    fn a_camera_that_says_its_colours_is_read_in_them() {
        use crate::{Matrix, Range, test_pattern};

        let name = "twicecat-test-camera-pattern";
        let _camera = TestCamera::start(name, 320, 180, 30).expect("the virtual camera");
        let (frames_tx, frames) = mpsc::channel();
        let _opened = Camera::open(
            None,
            name,
            mode(320, 180, 30, CameraFormat::Nv12),
            Box::new(ToChannel {
                frames: frames_tx,
                ended: mpsc::channel().0,
            }),
        )
        .expect("opens");
        let mut counters = Vec::new();
        while counters.len() < 5 {
            let frame = frames
                .recv_timeout(Duration::from_secs(5))
                .expect("a frame");
            assert_eq!(frame.color().matrix, Matrix::Bt709);
            assert_eq!(frame.color().range, Range::Limited);
            counters.push(test_pattern::read_stamp(&frame).expect("a stamp").counter);
        }
        assert!(counters.windows(2).all(|w| w[0] < w[1]), "{counters:?}");
    }
}
