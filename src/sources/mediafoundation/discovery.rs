use std::collections::{HashMap, HashSet};
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

use super::source::probe_activate;

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
                // Probe results keyed by symbolic link: an id is opened once and
                // its usability remembered until the device disappears.
                let mut cache: HashMap<String, bool> = HashMap::new();
                // Ids already reported as unopenable, so the log stays one line
                // per device instead of one every 2 s.
                let mut logged: HashSet<String> = HashSet::new();
                loop {
                    let list = unsafe { enumerate_devices(&mut cache, &mut logged) };
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

/// Enumerate video capture devices, probing each new id for openability. Only
/// devices that negotiate a compositor-supported output type are published, so
/// a WDM-only device Media Foundation cannot open is hidden. Returns an empty
/// list on any failure — the discovery loop retries.
unsafe fn enumerate_devices(
    cache: &mut HashMap<String, bool>,
    logged: &mut HashSet<String>,
) -> Vec<Device> {
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

    let mut candidates: Vec<(String, String)> = Vec::with_capacity(count as usize);
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
        if !cache.contains_key(&id) {
            let usable = unsafe { probe_activate(activate) };
            cache.insert(id.clone(), usable);
        }
        candidates.push((id, name));
    }

    // The activate objects were written into a CoTaskMem block; releasing each
    // and freeing the block is the documented cleanup for MFEnumDeviceSources.
    for activate in activate_slice.iter().flatten() {
        let _ = unsafe { activate.ShutdownObject() };
    }
    unsafe { CoTaskMemFree(Some(raw_devices as *const core::ffi::c_void)) };

    filter_devices(candidates, cache, logged)
}

/// Keep only cached-usable devices, log first-time rejections, and evict cache
/// entries for devices no longer enumerated (so a reappearing device is probed
/// again). Pure, so the filtering is testable without hardware.
fn filter_devices(
    candidates: Vec<(String, String)>,
    cache: &mut HashMap<String, bool>,
    logged: &mut HashSet<String>,
) -> Vec<Device> {
    let present: HashSet<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
    cache.retain(|id, _| present.contains(id.as_str()));
    logged.retain(|id| present.contains(id.as_str()));

    let mut devices = Vec::with_capacity(candidates.len());
    for (id, name) in candidates {
        if cache.get(&id).copied().unwrap_or(false) {
            devices.push(Device { id, name });
        } else if logged.insert(id.clone()) {
            tracing::info!(
                device = %name,
                "MediaFoundation: device enumerated but not openable; hiding"
            );
        }
    }
    return devices;
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
    use super::{activate_array, filter_devices};
    use std::collections::{HashMap, HashSet};

    /// The zero-device enumeration result (null pointer, count 0) must yield an
    /// empty slice instead of tripping `from_raw_parts`'s non-null precondition.
    #[test]
    fn empty_enumeration_yields_empty_slice() {
        let empty = unsafe { activate_array(std::ptr::null(), 0) };
        assert!(empty.is_empty());
    }

    /// Devices marked unusable by the probe are hidden; usable ones are kept.
    #[test]
    fn filter_hides_unopenable_devices() {
        let candidates = vec![
            ("usb#cam".to_string(), "Webcam".to_string()),
            ("wdm#decklink".to_string(), "DeckLink".to_string()),
        ];
        let mut cache = HashMap::from([
            ("usb#cam".to_string(), true),
            ("wdm#decklink".to_string(), false),
        ]);
        let mut logged = HashSet::new();

        let devices = filter_devices(candidates, &mut cache, &mut logged);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "usb#cam");
        assert!(logged.contains("wdm#decklink"));
    }

    /// Ids no longer enumerated are evicted from the cache so a reappearing
    /// device gets probed again.
    #[test]
    fn filter_evicts_absent_devices_from_cache() {
        let mut cache = HashMap::from([
            ("gone".to_string(), true),
            ("here".to_string(), true),
        ]);
        let mut logged = HashSet::from(["gone".to_string()]);

        let candidates = vec![("here".to_string(), "Here".to_string())];
        let devices = filter_devices(candidates, &mut cache, &mut logged);

        assert_eq!(devices.len(), 1);
        assert!(!cache.contains_key("gone"));
        assert!(!logged.contains("gone"));
    }
}
