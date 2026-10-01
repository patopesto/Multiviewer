use screen_capture_kit::shareable_content::SCShareableContent;
use objc2::rc::Retained;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Device {
    pub display_id: String,
    pub width: u32,
    pub height: u32,
}

impl Device {
    pub fn label(&self) -> String {
        return format_display_label(&self.display_id, self.width, self.height);
    }
}

/// Shared by the discovery list and `SourceKind::display_label`, so a connected
/// source and its discovered twin render the same text in the source dropdown.
pub fn format_display_label(display_id: &str, width: u32, height: u32) -> String {
    return format!("Display {display_id} ({width}x{height})");
}

/// Block on the async `SCShareableContent` callback and return the content.
/// Not `Send` (`SCDisplay` is `!Send`), so this may only be called and used on
/// one thread — callers extract plain data before handing anything across.
pub(super) fn fetch_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    SCShareableContent::get_shareable_content_with_completion_closure(move |content, error| {
        let result = match content {
            Some(content) => Ok(content),
            None => Err(error
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "no shareable content".to_string())),
        };
        let _ = tx.send(result);
    });
    return rx.recv().map_err(|_| "shareable content request dropped".to_string())?;
}

fn fetch_devices() -> Result<Vec<Device>, String> {
    let content = fetch_content()?;
    let mut devices = Vec::new();
    for display in content.displays().iter() {
        devices.push(Device {
            display_id: display.display_id().to_string(),
            width: display.width() as u32,
            height: display.height() as u32,
        });
    }
    return Ok(devices);
}

/// Background display discovery.
pub struct Discovery {
    devices: Arc<Mutex<Vec<Device>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let devices2 = devices.clone();
        std::thread::Builder::new()
            .name("screencapturekit-discovery".into())
            .spawn(move || {
                // Screen-recording permission errors repeat every cycle; log
                // each distinct one once instead of every 2 seconds.
                let mut last_error: Option<String> = None;
                loop {
                    match fetch_devices() {
                        Ok(list) => {
                            *devices2.lock().unwrap() = list;
                            last_error = None;
                        }
                        Err(e) => {
                            *devices2.lock().unwrap() = Vec::new();
                            if last_error.as_deref() != Some(e.as_str()) {
                                tracing::error!("ScreenCaptureKit display discovery failed: {e}");
                                last_error = Some(e);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn screencapturekit-discovery");
        Self { devices }
    }

    pub fn list(&self) -> Vec<Device> {
        self.devices.lock().unwrap().clone()
    }

    pub fn find_by_display_id(&self, display_id: &str) -> Option<Device> {
        self.devices
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.display_id == display_id)
            .cloned()
    }
}
