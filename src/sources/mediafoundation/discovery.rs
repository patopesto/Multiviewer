use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, MFCreateAttributes, MFEnumDeviceSources, MFStartup, MFSTARTUP_LITE, MF_VERSION,
    MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::core::PWSTR;

/// A discovered Media Foundation video capture device.
#[derive(Clone, Debug)]
pub struct Device {
    /// Stable symbolic link, used as the persisted `source_ref`.
    pub id: String,
    pub name: String,
}

/// Background Media Foundation video device discovery.
pub struct Discovery {
    devices: Arc<Mutex<Vec<Device>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let devices2 = devices.clone();
        std::thread::Builder::new()
            .name("mediafoundation-discovery".into())
            .spawn(move || {
                // MFStartup/CoInitializeEx are per-thread; this thread owns them
                // for its lifetime.
                unsafe {
                    let coinit = CoInitializeEx(None, COINIT_MULTITHREADED);
                    if coinit.is_err() && coinit != RPC_E_CHANGED_MODE {
                        tracing::error!("MediaFoundation discovery: CoInitializeEx failed");
                        return;
                    }
                    if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
                        tracing::error!("MediaFoundation discovery: MFStartup failed");
                        CoUninitialize();
                        return;
                    }
                }
                tracing::info!("MediaFoundation discovery started");
                loop {
                    let list = unsafe { enumerate_devices() };
                    tracing::debug!(count = list.len(), "MediaFoundation discovery poll");
                    *devices2.lock().unwrap() = list;
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn mediafoundation-discovery");
        Self { devices }
    }

    pub fn list(&self) -> Vec<Device> {
        self.devices.lock().unwrap().clone()
    }

    pub fn find_by_id(&self, id: &str) -> Option<Device> {
        self.devices
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }
}

/// Enumerate video capture devices. Returns an empty list on any failure — the
/// discovery loop retries; it must not panic on a transient device error.
unsafe fn enumerate_devices() -> Vec<Device> {
    let mut attributes = None;
    if let Err(e) = unsafe { MFCreateAttributes(&mut attributes, 1) } {
        tracing::warn!("MediaFoundation discovery: MFCreateAttributes failed: {e}");
        return Vec::new();
    }
    let Some(attributes) = attributes else {
        tracing::warn!("MediaFoundation discovery: MFCreateAttributes returned null");
        return Vec::new();
    };
    if let Err(e) = unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    } {
        tracing::warn!("MediaFoundation discovery: SetGUID(source type) failed: {e}");
        return Vec::new();
    }

    let mut raw_devices: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    if let Err(e) = unsafe { MFEnumDeviceSources(&attributes, &mut raw_devices, &mut count) } {
        tracing::warn!("MediaFoundation discovery: MFEnumDeviceSources failed: {e}");
        return Vec::new();
    }
    let activate_slice = unsafe { activate_array(raw_devices, count) };

    let mut devices = Vec::with_capacity(count as usize);
    for activate in activate_slice.iter().flatten() {
        let Some(id) = (unsafe {
            allocate_string(
                activate,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
            )
        }) else {
            continue;
        };
        let name = unsafe { allocate_string(activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME) }
            .unwrap_or_else(|| id.clone());
        devices.push(Device { id, name });
    }

    // The activate objects were written into a CoTaskMem block; releasing each
    // and freeing the block is the documented cleanup for MFEnumDeviceSources.
    for activate in activate_slice.iter().flatten() {
        let _ = unsafe { activate.ShutdownObject() };
    }
    unsafe { CoTaskMemFree(Some(raw_devices as *const core::ffi::c_void)) };

    devices
}

/// Borrow the `IMFActivate` array `MFEnumDeviceSources` wrote. The no-devices
/// result is a null pointer with `count == 0`, which `from_raw_parts` rejects
/// (it requires non-null even for a zero-length slice), so that case returns an
/// empty slice instead of panicking.
///
/// # Safety
/// `raw` must point to `count` consecutive, initialized `Option<IMFActivate>`
/// values allocated by `MFEnumDeviceSources`, or be null.
pub(super) unsafe fn activate_array<'a>(
    raw: *const Option<IMFActivate>,
    count: u32,
) -> &'a [Option<IMFActivate>] {
    if raw.is_null() || count == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(raw, count as usize) }
}

/// Read a string attribute into an owned `String`, freeing the returned
/// allocation. `None` when the attribute is absent or empty.
unsafe fn allocate_string(attributes: &IMFActivate, key: &windows::core::GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut len = 0u32;
    unsafe { attributes.GetAllocatedString(key, &mut value, &mut len) }.ok()?;
    if value.is_null() {
        return None;
    }
    let s = unsafe { value.to_string() }.ok()?;
    unsafe { CoTaskMemFree(Some(value.as_ptr() as *const core::ffi::c_void)) };
    if s.is_empty() {
        return None;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::activate_array;

    /// The zero-device enumeration result (null pointer, count 0) must yield an
    /// empty slice instead of tripping `from_raw_parts`'s non-null precondition.
    #[test]
    fn empty_enumeration_yields_empty_slice() {
        let empty = unsafe { activate_array(std::ptr::null(), 0) };
        assert!(empty.is_empty());
    }
}
