//! The global mouse button source: the bound button pressed over any
//! application, on Windows (a low-level mouse hook), X11 (XInput 2 raw
//! events) and macOS (an `NSEvent` global monitor). Wayland lets no
//! application see buttons outside its own windows, so there the button
//! only works in Twicecat's window (the app's window source).
//!
//! Unlike the global keyboard shortcut, the button is not taken from other
//! applications: a side button still goes back in the browser (#34, human
//! decision 2026-10-18). X11 raw events and macOS monitors could not take it
//! anyway.
//!
//! The decoders are pure and compiled for the tests on every system, so each
//! system's numbering is tested everywhere.

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
use crate::binding::MouseButton;

/// The button an X11 raw button event's `detail` names: 2 is the middle
/// button, 8 and 9 back and forward, 10 and up the others; 1 and 3 are left
/// and right, 4 to 7 the wheel (as winit reads them).
#[cfg(any(test, target_os = "linux"))]
pub(crate) fn x11_button(detail: u32) -> Option<MouseButton> {
    match detail {
        2 => Some(MouseButton::MIDDLE),
        8 => Some(MouseButton::BACK),
        9 => Some(MouseButton::FORWARD),
        10.. => u16::try_from(detail).ok().map(MouseButton::numbered),
        _ => None,
    }
}

/// The button an `NSEvent`'s `buttonNumber` names: 2 is the middle button,
/// 3 and 4 back and forward, 5 and up the others; 0 and 1 are left and
/// right (as winit reads them).
#[cfg(any(test, target_os = "macos"))]
pub(crate) fn macos_button(number: isize) -> Option<MouseButton> {
    match number {
        2 => Some(MouseButton::MIDDLE),
        3 => Some(MouseButton::BACK),
        4 => Some(MouseButton::FORWARD),
        5.. => u16::try_from(number).ok().map(MouseButton::numbered),
        _ => None,
    }
}

/// Win32 message numbers (`WM_MBUTTONDOWN` and the others, as windows-sys
/// names them) and `XBUTTON1`/`XBUTTON2`; checked against windows-sys in the
/// tests on Windows.
#[cfg(any(test, target_os = "windows"))]
mod win32 {
    pub(super) const WM_MBUTTONDOWN: u32 = 0x0207;
    pub(super) const WM_MBUTTONUP: u32 = 0x0208;
    pub(super) const WM_XBUTTONDOWN: u32 = 0x020B;
    pub(super) const WM_XBUTTONUP: u32 = 0x020C;
    pub(super) const XBUTTON1: u16 = 1;
    pub(super) const XBUTTON2: u16 = 2;
}

/// The button and whether it went down, for a low-level mouse hook's
/// message and its `MSLLHOOKSTRUCT::mouseData` (whose high word names the
/// side button). Other messages (moves, the wheel, left and right) are
/// `None`.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn windows_button(
    message: u32,
    mouse_data: u32,
) -> Option<(crate::binding::MouseButton, bool)> {
    use crate::binding::MouseButton;
    use win32::*;
    let side = || match (mouse_data >> 16) as u16 {
        XBUTTON1 => Some(MouseButton::BACK),
        XBUTTON2 => Some(MouseButton::FORWARD),
        _ => None,
    };
    match message {
        WM_MBUTTONDOWN => Some((MouseButton::MIDDLE, true)),
        WM_MBUTTONUP => Some((MouseButton::MIDDLE, false)),
        WM_XBUTTONDOWN => side().map(|b| (b, true)),
        WM_XBUTTONUP => side().map(|b| (b, false)),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
pub(crate) use windows::Hook;

#[cfg(target_os = "windows")]
mod windows {
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, HC_ACTION, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW,
        PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL, WM_QUIT, WM_USER,
    };

    use crate::binding::MouseButton;
    use crate::{PttEvent, Shared, Source};

    /// What the hook procedure compares and reports to, on the hook's
    /// thread (the procedure runs there, from its message loop).
    struct Watch {
        button: MouseButton,
        shared: Arc<Shared>,
        held: bool,
    }

    thread_local! {
        static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
    }

    /// The hook's thread. Dropping it ends the thread's message loop.
    pub(crate) struct Hook {
        thread_id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl Hook {
        /// Hooks the mouse on a thread of its own; reports `Ready` or
        /// `Unavailable`.
        pub(crate) fn start(button: MouseButton, shared: Arc<Shared>) -> Option<Self> {
            let (started, thread_id) = mpsc::channel();
            let events = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("ptt-mouse".into())
                .spawn(move || run(button, events, &started));
            let thread = match spawned {
                Ok(thread) => thread,
                Err(e) => {
                    shared.notify(PttEvent::Unavailable {
                        source: Source::Mouse,
                        reason: e.to_string(),
                    });
                    return None;
                }
            };
            match thread_id.recv() {
                Ok(Some(thread_id)) => Some(Self {
                    thread_id,
                    thread: Some(thread),
                }),
                _ => {
                    let _ = thread.join();
                    None
                }
            }
        }
    }

    impl Drop for Hook {
        fn drop(&mut self) {
            // SAFETY: a plain call; the thread has a message queue (`run`
            // made it before sending its ID).
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn run(button: MouseButton, shared: Arc<Shared>, started: &mpsc::Sender<Option<u32>>) {
        // SAFETY: MSG is plain data; all zeroes is a valid value.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: plain calls with valid arguments. Peeking makes the
        // thread's message queue, so `Drop`'s WM_QUIT reaches it. A
        // low-level hook may name the executable's own module.
        let hook = unsafe {
            PeekMessageW(
                &mut msg,
                std::ptr::null_mut(),
                WM_USER,
                WM_USER,
                PM_NOREMOVE,
            );
            SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(on_mouse),
                GetModuleHandleW(std::ptr::null()),
                0,
            )
        };
        if hook.is_null() {
            shared.notify(PttEvent::Unavailable {
                source: Source::Mouse,
                reason: std::io::Error::last_os_error().to_string(),
            });
            let _ = started.send(None);
            return;
        }
        WATCH.with(|watch| {
            *watch.borrow_mut() = Some(Watch {
                button,
                shared: shared.clone(),
                held: false,
            });
        });
        shared.notify(PttEvent::Ready {
            source: Source::Mouse,
            trigger: button.to_string(),
        });
        // SAFETY: a plain call.
        let _ = started.send(Some(unsafe { GetCurrentThreadId() }));
        // The hook procedure runs from here, until WM_QUIT (0) or an error
        // (-1).
        // SAFETY: `msg` is a valid MSG to write into.
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {}
        // SAFETY: the hook this thread set.
        unsafe { UnhookWindowsHookEx(hook) };
        let watch = WATCH.with(|watch| watch.borrow_mut().take());
        if watch.is_some_and(|w| w.held) {
            shared.report(Source::Mouse, false);
        }
    }

    /// The low-level mouse hook procedure. Short: Windows drops a hook that
    /// keeps the mouse waiting. The event always goes on to other
    /// applications.
    unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && lparam != 0 {
            // SAFETY: for WH_MOUSE_LL with HC_ACTION, `lparam` points to an
            // MSLLHOOKSTRUCT valid for this call.
            let mouse_data = unsafe { (*(lparam as *const MSLLHOOKSTRUCT)).mouseData };
            if let Some((button, pressed)) = super::windows_button(wparam as u32, mouse_data) {
                WATCH.with(|watch| {
                    if let Some(w) = watch.borrow_mut().as_mut()
                        && w.button == button
                        && w.held != pressed
                    {
                        w.held = pressed;
                        w.shared.report(Source::Mouse, pressed);
                    }
                });
            }
        }
        // SAFETY: passes the event on unchanged, as a hook must.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use x11::Hook;

#[cfg(target_os = "linux")]
mod x11 {
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xinput::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::{self, ConnectionExt as _};
    use x11rb::rust_connection::RustConnection;

    use crate::binding::MouseButton;
    use crate::{PttEvent, Shared, Source};

    /// The X11 connection and the thread reading its raw button events.
    /// Dropping it wakes the thread with a message to `window` and joins it.
    pub(crate) struct Hook {
        conn: Arc<RustConnection>,
        window: xproto::Window,
        thread: Option<JoinHandle<()>>,
    }

    impl Hook {
        /// Listens to every pointer's raw button events; reports `Ready` or
        /// `Unavailable`.
        pub(crate) fn start(button: MouseButton, shared: Arc<Shared>) -> Option<Self> {
            let (conn, window) = match listen() {
                Ok(listening) => listening,
                Err(reason) => {
                    shared.notify(PttEvent::Unavailable {
                        source: Source::Mouse,
                        reason,
                    });
                    return None;
                }
            };
            let conn = Arc::new(conn);
            let reader = conn.clone();
            let events = shared.clone();
            let thread = std::thread::Builder::new()
                .name("ptt-mouse".into())
                .spawn(move || run(&reader, window, button, &events))
                .ok();
            if thread.is_none() {
                shared.notify(PttEvent::Unavailable {
                    source: Source::Mouse,
                    reason: "no thread".into(),
                });
                return None;
            }
            shared.notify(PttEvent::Ready {
                source: Source::Mouse,
                trigger: button.to_string(),
            });
            Some(Self {
                conn,
                window,
                thread,
            })
        }
    }

    impl Drop for Hook {
        fn drop(&mut self) {
            let wake = xproto::ClientMessageEvent::new(
                32,
                self.window,
                xproto::AtomEnum::NONE,
                [0_u32; 5],
            );
            // An error means the connection is gone, and the thread with it.
            let _ = self
                .conn
                .send_event(false, self.window, xproto::EventMask::NO_EVENT, wake);
            let _ = self.conn.flush();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Connects, checks for XInput 2.1 (raw events reach every client only
    /// from 2.1), makes the window `Drop` wakes the thread through, and asks
    /// for the master pointers' raw button events on the root window.
    fn listen() -> Result<(RustConnection, xproto::Window), String> {
        let text = |e: &dyn std::fmt::Display| e.to_string();
        let (conn, screen) = x11rb::connect(None).map_err(|e| text(&e))?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .ok_or("the display has no such screen")?
            .root;
        let version = conn
            .xinput_xi_query_version(2, 2)
            .map_err(|e| text(&e))?
            .reply()
            .map_err(|e| text(&e))?;
        if (version.major_version, version.minor_version) < (2, 1) {
            return Err(format!(
                "the X server has XInput {}.{}; 2.1 is needed",
                version.major_version, version.minor_version
            ));
        }
        let window = conn.generate_id().map_err(|e| text(&e))?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &xproto::CreateWindowAux::new(),
        )
        .map_err(|e| text(&e))?
        .check()
        .map_err(|e| text(&e))?;
        conn.xinput_xi_select_events(
            root,
            &[xinput::EventMask {
                deviceid: xinput::Device::ALL_MASTER.into(),
                mask: vec![
                    xinput::XIEventMask::RAW_BUTTON_PRESS | xinput::XIEventMask::RAW_BUTTON_RELEASE,
                ],
            }],
        )
        .map_err(|e| text(&e))?
        .check()
        .map_err(|e| text(&e))?;
        Ok((conn, window))
    }

    fn run(conn: &RustConnection, window: xproto::Window, button: MouseButton, shared: &Shared) {
        let mut held = false;
        loop {
            let (detail, pressed) = match conn.wait_for_event() {
                Ok(Event::XinputRawButtonPress(e)) => (e.detail, true),
                Ok(Event::XinputRawButtonRelease(e)) => (e.detail, false),
                Ok(Event::ClientMessage(e)) if e.window == window => break,
                Ok(_) => continue,
                Err(e) => {
                    shared.notify(PttEvent::Unavailable {
                        source: Source::Mouse,
                        reason: e.to_string(),
                    });
                    break;
                }
            };
            if super::x11_button(detail) == Some(button) && held != pressed {
                held = pressed;
                shared.report(Source::Mouse, pressed);
            }
        }
        if held {
            shared.report(Source::Mouse, false);
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::Hook;

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};

    use crate::binding::MouseButton;
    use crate::{PttEvent, Shared, Source};

    /// The global monitor. AppKit calls it on the main thread, for events
    /// sent to other applications; Twicecat's own window is the app's window
    /// source. Start and drop it on the main thread.
    pub(crate) struct Hook {
        monitor: Retained<AnyObject>,
        held: Arc<AtomicBool>,
        shared: Arc<Shared>,
    }

    impl Hook {
        /// Adds the monitor; reports `Ready` or `Unavailable`.
        pub(crate) fn start(button: MouseButton, shared: Arc<Shared>) -> Option<Self> {
            let held = Arc::new(AtomicBool::new(false));
            let (events, down) = (shared.clone(), held.clone());
            let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
                // SAFETY: AppKit passes a valid event for the call.
                let event = unsafe { event.as_ref() };
                let pressed = match event.r#type() {
                    NSEventType::OtherMouseDown => true,
                    NSEventType::OtherMouseUp => false,
                    _ => return,
                };
                if super::macos_button(event.buttonNumber()) == Some(button)
                    && down.swap(pressed, Ordering::Relaxed) != pressed
                {
                    events.report(Source::Mouse, pressed);
                }
            });
            let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
                NSEventMask::OtherMouseDown | NSEventMask::OtherMouseUp,
                &handler,
            );
            let Some(monitor) = monitor else {
                shared.notify(PttEvent::Unavailable {
                    source: Source::Mouse,
                    reason: "macOS refused the mouse monitor".into(),
                });
                return None;
            };
            shared.notify(PttEvent::Ready {
                source: Source::Mouse,
                trigger: button.to_string(),
            });
            Some(Self {
                monitor,
                held,
                shared,
            })
        }
    }

    impl Drop for Hook {
        fn drop(&mut self) {
            // SAFETY: the object addGlobalMonitor… returned, removed once.
            unsafe { NSEvent::removeMonitor(&self.monitor) };
            if self.held.swap(false, Ordering::Relaxed) {
                self.shared.report(Source::Mouse, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::MouseButton;

    #[test]
    fn x11_numbers_name_the_middle_side_and_other_buttons() {
        assert_eq!(x11_button(2), Some(MouseButton::MIDDLE));
        assert_eq!(x11_button(8), Some(MouseButton::BACK));
        assert_eq!(x11_button(9), Some(MouseButton::FORWARD));
        assert_eq!(x11_button(12), Some(MouseButton::numbered(12)));
        for click_or_wheel in [0, 1, 3, 4, 5, 6, 7, 70_000] {
            assert_eq!(x11_button(click_or_wheel), None, "{click_or_wheel}");
        }
    }

    #[test]
    fn macos_numbers_name_the_middle_side_and_other_buttons() {
        assert_eq!(macos_button(2), Some(MouseButton::MIDDLE));
        assert_eq!(macos_button(3), Some(MouseButton::BACK));
        assert_eq!(macos_button(4), Some(MouseButton::FORWARD));
        assert_eq!(macos_button(5), Some(MouseButton::numbered(5)));
        for click in [-1, 0, 1, 70_000] {
            assert_eq!(macos_button(click), None, "{click}");
        }
    }

    #[test]
    fn windows_messages_name_the_middle_and_side_buttons() {
        let x = |n: u16| u32::from(n) << 16;
        assert_eq!(
            windows_button(win32::WM_MBUTTONDOWN, 0),
            Some((MouseButton::MIDDLE, true))
        );
        assert_eq!(
            windows_button(win32::WM_MBUTTONUP, 0),
            Some((MouseButton::MIDDLE, false))
        );
        assert_eq!(
            windows_button(win32::WM_XBUTTONDOWN, x(win32::XBUTTON1)),
            Some((MouseButton::BACK, true))
        );
        assert_eq!(
            windows_button(win32::WM_XBUTTONUP, x(win32::XBUTTON2)),
            Some((MouseButton::FORWARD, false))
        );
        assert_eq!(windows_button(win32::WM_XBUTTONDOWN, x(3)), None);
        // WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MOUSEWHEEL.
        for other in [0x0200, 0x0201, 0x0204, 0x020A] {
            assert_eq!(windows_button(other, x(win32::XBUTTON1)), None);
        }
    }

    /// The real hook starts on its thread and dropping it ends that thread
    /// (no input is sent). Where Windows refuses the hook, the source says
    /// so instead.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_windows_hook_starts_and_stops() {
        use std::sync::{Arc, Mutex};

        use crate::merge::Merger;
        use crate::{PttEvent, Shared, Source};

        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let shared = Arc::new(Shared {
            merger: Mutex::new(Merger::default()),
            sink: Arc::new(move |e| seen.lock().unwrap().push(e)),
        });
        let hook = Hook::start(MouseButton::BACK, shared);
        let first = events.lock().unwrap().first().cloned();
        match (hook.is_some(), first) {
            (true, Some(PttEvent::Ready { source, trigger })) => {
                assert_eq!((source, trigger.as_str()), (Source::Mouse, "back"));
            }
            (false, Some(PttEvent::Unavailable { source, .. })) => {
                assert_eq!(source, Source::Mouse);
            }
            other => panic!("{other:?}"),
        }
        drop(hook);
        assert_eq!(events.lock().unwrap().len(), 1, "nothing was pressed");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn the_win32_numbers_are_windows_sys_ones() {
        use windows_sys::Win32::UI::WindowsAndMessaging as w;
        assert_eq!(
            [
                win32::WM_MBUTTONDOWN,
                win32::WM_MBUTTONUP,
                win32::WM_XBUTTONDOWN,
                win32::WM_XBUTTONUP
            ],
            [
                w::WM_MBUTTONDOWN,
                w::WM_MBUTTONUP,
                w::WM_XBUTTONDOWN,
                w::WM_XBUTTONUP
            ]
        );
        assert_eq!(
            [win32::XBUTTON1, win32::XBUTTON2],
            [w::XBUTTON1, w::XBUTTON2]
        );
    }
}
