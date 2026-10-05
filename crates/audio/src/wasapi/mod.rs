//! Windows: what the MMDevice API knows and cpal does not expose. The
//! default *communications* devices (the ones Windows sets apart for calls,
//! "Default Communication Device" in the sound settings), and notifications
//! when endpoints come, go, or change role, instead of polling.
//!
//! COM objects here are created, used and released on one thread (they are
//! not `Send`): the device watcher's thread, or the processing thread. The
//! notification callbacks run on the MMDevice API's own thread; they only
//! hand an [`EndpointEvent`] to a function that must not block (set a flag,
//! unpark a thread), as the API requires.

use windows::Win32::Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, eCapture, eCommunications, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{HRESULT, PCWSTR};
use windows_core::implement;

mod stream;

pub(crate) use stream::{Side, Stream, open as open_stream};

/// cpal's prefix for WASAPI device IDs (`"wasapi:<endpoint ID>"`).
pub(crate) const CPAL_PREFIX: &str = "wasapi:";

/// COM for the current thread, released when the thread ends. Single-
/// threaded apartment, as cpal asks for on the same threads; a thread whose
/// COM was already set up otherwise keeps it (`RPC_E_CHANGED_MODE`).
struct Com(HRESULT);

impl Drop for Com {
    fn drop(&mut self) {
        if self.0.is_ok() {
            // SAFETY: pairs the successful CoInitializeEx of this thread
            // (thread-local: dropped on the thread that initialised).
            unsafe { CoUninitialize() };
        }
    }
}

thread_local! {
    // SAFETY: CoInitializeEx has no preconditions beyond being paired with
    // CoUninitialize on success, which `Com`'s drop does.
    static COM: Com = Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) });
}

/// The endpoint enumerator, with COM set up on this thread.
pub(super) fn enumerator() -> Option<IMMDeviceEnumerator> {
    let usable = COM.with(|com| com.0.is_ok() || com.0 == RPC_E_CHANGED_MODE);
    if !usable {
        return None;
    }
    // SAFETY: COM is initialised on this thread (above); MMDeviceEnumerator
    // is the documented class for IMMDeviceEnumerator.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() }
}

/// The default communications device's ID for capture (`input`) or
/// playback, in cpal's form, so that `cpal::Host::device_by_id` opens it.
/// `None` without one (no device of that kind) or when COM is unusable.
pub(crate) fn communications_default(input: bool) -> Option<String> {
    let flow = if input { eCapture } else { eRender };
    let enumerator = enumerator()?;
    // SAFETY: a COM call on the thread that created `enumerator`; the ID
    // string is the callee's allocation, read once and freed with
    // CoTaskMemFree as documented for IMMDevice::GetId.
    unsafe {
        let device = enumerator
            .GetDefaultAudioEndpoint(flow, eCommunications)
            .ok()?;
        let id = device.GetId().ok()?;
        let text = id.to_string().ok();
        CoTaskMemFree(Some(id.as_ptr().cast_const().cast()));
        text.map(|t| format!("{CPAL_PREFIX}{t}"))
    }
}

/// What changed among the endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndpointEvent {
    /// A device came or went, or was enabled, disabled, plugged or
    /// unplugged.
    Devices,
    /// The default communications device for capture (`input`) or playback
    /// changed.
    CommunicationsDefault {
        /// Capture (true) or playback.
        input: bool,
    },
}

/// Keeps the notification callback registered while it lives; dropped on
/// the thread that registered it (it is not `Send`).
pub(crate) struct EndpointWatch {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

impl EndpointWatch {
    /// Registers `on_event`, which runs on the MMDevice API's thread and
    /// must return at once. `None` if COM or the registration fails (the
    /// caller then polls).
    pub(crate) fn register(
        on_event: impl Fn(EndpointEvent) + Send + Sync + 'static,
    ) -> Option<Self> {
        let enumerator = enumerator()?;
        let client: IMMNotificationClient = Notifications {
            on_event: Box::new(on_event),
        }
        .into();
        // SAFETY: a COM call on the thread that created `enumerator`;
        // `client` stays alive in `Self` until it is unregistered in drop.
        unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }.ok()?;
        Some(Self { enumerator, client })
    }
}

impl Drop for EndpointWatch {
    fn drop(&mut self) {
        // SAFETY: same thread and objects as the registration. A callback
        // still running on the MMDevice API's thread holds its own COM
        // reference to `client`, and the closure owns everything it uses
        // (`'static`), so nothing it touches is freed under it.
        let _ = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.client)
        };
    }
}

#[implement(IMMNotificationClient)]
struct Notifications {
    on_event: Box<dyn Fn(EndpointEvent) + Send + Sync>,
}

impl IMMNotificationClient_Impl for Notifications_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        (self.on_event)(EndpointEvent::Devices);
        Ok(())
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        (self.on_event)(EndpointEvent::Devices);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        (self.on_event)(EndpointEvent::Devices);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        if role == eCommunications && (flow == eCapture || flow == eRender) {
            (self.on_event)(EndpointEvent::CommunicationsDefault {
                input: flow == eCapture,
            });
        }
        Ok(())
    }

    // Names and formats: the device list shows names, but a renamed device
    // is rare and the next change refreshes it; property changes come in
    // bursts on every plug.
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_communications_defaults_are_listed_devices() {
        // CI machines may have no audio device: then there is nothing to
        // compare, which is fine.
        let Ok(list) = crate::list_devices() else {
            return;
        };
        for (input, devices) in [(true, &list.inputs), (false, &list.outputs)] {
            if let Some(id) = communications_default(input) {
                let listed = devices.iter().find(|d| d.id == id);
                assert!(listed.is_some(), "{id} is not among {devices:?}");
                assert!(listed.unwrap().is_default, "{id} is the default");
            }
        }
    }

    #[test]
    fn registering_and_dropping_a_watch_does_not_hang() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let watch = EndpointWatch::register(move |e| {
            let _ = tx.lock().map(|tx| tx.send(e));
        });
        drop(watch);
    }
}
