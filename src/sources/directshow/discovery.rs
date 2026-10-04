use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::DirectShow::{IBaseFilter, ICreateDevEnum};
use windows::Win32::Media::MediaFoundation::{
    CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory,
};
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IBindCtx, IEnumMoniker, IMoniker,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BSTR};
use windows::core::{BSTR, PCWSTR};

/// A discovered DirectShow video capture device.
#[derive(Clone, Debug)]
pub struct Device {
    /// Stable identity, used as the persisted `source_ref`: the device path for
    /// hardware devices, falling back to the moniker display name/friendly name
    /// for software filters (e.g. virtual cameras) that expose no `DevicePath`.
    pub id: String,
    pub name: String,
}

/// Background DirectShow video device discovery.
pub struct Discovery {
    devices: Arc<Mutex<Vec<Device>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let devices2 = devices.clone();
        std::thread::Builder::new()
            .name("directshow-discovery".into())
            .spawn(move || {
                unsafe {
                    let coinit = CoInitializeEx(None, COINIT_MULTITHREADED);
                    if coinit.is_err() && coinit != RPC_E_CHANGED_MODE {
                        tracing::error!("DirectShow discovery: CoInitializeEx failed");
                        return;
                    }
                }
                tracing::info!("DirectShow discovery started");
                loop {
                    let list = unsafe { enumerate_devices() };
                    tracing::debug!(count = list.len(), "DirectShow discovery poll");
                    *devices2.lock().unwrap() = list;
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn directshow-discovery");
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

/// Enumerate DirectShow video input devices. Returns an empty list on any
/// failure — the discovery loop retries.
unsafe fn enumerate_devices() -> Vec<Device> {
    let Ok(dev_enum) = (unsafe {
        CoCreateInstance::<_, ICreateDevEnum>(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER)
    }) else {
        tracing::warn!("DirectShow discovery: CoCreateInstance(SystemDeviceEnum) failed");
        return Vec::new();
    };

    let mut enum_moniker: Option<IEnumMoniker> = None;
    // An empty category yields S_FALSE, which `is_err()` treats as success with
    // a null enumerator: no devices, not an error.
    if unsafe {
        dev_enum.CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut enum_moniker, 0)
    }
    .is_err()
    {
        return Vec::new();
    }
    let Some(enum_moniker) = enum_moniker else {
        return Vec::new();
    };

    let mut devices = Vec::new();
    loop {
        let mut moniker: [Option<IMoniker>; 1] = [None];
        let hr = unsafe { enum_moniker.Next(&mut moniker, None) };
        if hr.is_err() {
            break;
        }
        let Some(moniker) = moniker[0].take() else {
            break;
        };
        let Ok(bag) = (unsafe {
            moniker.BindToStorage::<_, _, IPropertyBag>(None::<&IBindCtx>, None::<&IMoniker>)
        }) else {
            continue;
        };
        let Some((id, name)) = (unsafe { moniker_identity(&moniker, &bag) }) else {
            continue;
        };
        tracing::debug!(device = %name, id = %id, "DirectShow discovery: device");
        devices.push(Device { id, name });
    }
    devices
}

/// Bind the DirectShow capture filter whose resolved id is `device_id`.
pub(super) unsafe fn find_capture_filter(device_id: &str) -> Result<IBaseFilter, String> {
    let dev_enum = unsafe {
        CoCreateInstance::<_, ICreateDevEnum>(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER)
    }
    .map_err(|e| format!("CoCreateInstance(SystemDeviceEnum): {e}"))?;

    let mut enum_moniker: Option<IEnumMoniker> = None;
    unsafe { dev_enum.CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut enum_moniker, 0) }
        .map_err(|e| format!("CreateClassEnumerator: {e}"))?;
    let Some(enum_moniker) = enum_moniker else {
        return Err("no video input devices".to_string());
    };

    loop {
        let mut moniker: [Option<IMoniker>; 1] = [None];
        if unsafe { enum_moniker.Next(&mut moniker, None) }.is_err() {
            break;
        }
        let Some(moniker) = moniker[0].take() else {
            break;
        };
        let Ok(bag) = (unsafe {
            moniker.BindToStorage::<_, _, IPropertyBag>(None::<&IBindCtx>, None::<&IMoniker>)
        }) else {
            continue;
        };
        let Some((id, _)) = (unsafe { moniker_identity(&moniker, &bag) }) else {
            continue;
        };
        if id != device_id {
            continue;
        }
        return unsafe {
            moniker.BindToObject::<_, _, IBaseFilter>(None::<&IBindCtx>, None::<&IMoniker>)
        }
        .map_err(|e| format!("BindToObject: {e}"));
    }
    Err(format!("no DirectShow device matches {device_id:?}"))
}

/// Resolve a moniker's identity and display name: prefer the unique
/// `DevicePath`, then the moniker display name, then the friendly name (some
/// software filters expose none of the first two).
unsafe fn moniker_identity(moniker: &IMoniker, bag: &IPropertyBag) -> Option<(String, String)> {
    let device_path = unsafe { read_property(bag, "DevicePath") };
    let friendly = unsafe { read_property(bag, "FriendlyName") };
    let display = unsafe { moniker_display_name(moniker) };
    return choose_identity(device_path, display, friendly);
}

/// Pure identity fallback ordering, kept separate so it is testable without COM.
fn choose_identity(
    device_path: Option<String>,
    display: Option<String>,
    friendly: Option<String>,
) -> Option<(String, String)> {
    let id = device_path.or(display).or_else(|| friendly.clone())?;
    let name = friendly.unwrap_or_else(|| id.clone());
    return Some((id, name));
}

/// `IMoniker::GetDisplayName`, freed into an owned `String`.
unsafe fn moniker_display_name(moniker: &IMoniker) -> Option<String> {
    let name = unsafe {
        moniker.GetDisplayName(None::<&IBindCtx>, None::<&IMoniker>)
    }
    .ok()?;
    if name.is_null() {
        return None;
    }
    let text = unsafe { name.to_string() }.ok()?;
    unsafe { CoTaskMemFree(Some(name.as_ptr() as *const core::ffi::c_void)) };
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// Read a `VARIANT` string property from a device moniker's property bag.
unsafe fn read_property(bag: &IPropertyBag, name: &str) -> Option<String> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut value = VARIANT::default();
    if unsafe { bag.Read(PCWSTR(wide.as_ptr()), &mut value, None) }.is_err() {
        return None;
    }
    let vt = unsafe { value.Anonymous.Anonymous.vt };
    if vt != VT_BSTR {
        let _ = unsafe { VariantClear(&mut value) };
        return None;
    }
    // Borrow the BSTR out of the VARIANT before clearing it.
    let text = {
        let bstr: &BSTR = unsafe { &value.Anonymous.Anonymous.Anonymous.bstrVal };
        bstr.to_string()
    };
    let _ = unsafe { VariantClear(&mut value) };
    if text.is_empty() {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::choose_identity;

    #[test]
    fn device_path_is_the_preferred_id() {
        let (id, name) = choose_identity(
            Some("path".to_string()),
            Some("display".to_string()),
            Some("Friendly".to_string()),
        )
        .unwrap();
        assert_eq!(id, "path");
        assert_eq!(name, "Friendly");
    }

    /// Software filters (virtual cameras) expose no DevicePath; the display name
    /// keeps them listed and uniquely identifiable.
    #[test]
    fn display_name_is_used_when_device_path_is_missing() {
        let (id, name) =
            choose_identity(None, Some("display".to_string()), Some("OBS".to_string())).unwrap();
        assert_eq!(id, "display");
        assert_eq!(name, "OBS");
    }

    #[test]
    fn friendly_name_is_the_last_resort() {
        let (id, name) =
            choose_identity(None, None, Some("OBS Virtual Camera".to_string())).unwrap();
        assert_eq!(id, "OBS Virtual Camera");
        assert_eq!(name, "OBS Virtual Camera");
    }

    #[test]
    fn no_identity_when_nothing_is_available() {
        assert!(choose_identity(None, None, None).is_none());
    }
}
