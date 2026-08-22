use av_foundation::capture_device::AVCaptureDevice;
use av_foundation::media_format::AVMediaTypeVideo;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Device {
    pub unique_id: String,
    pub name: String,
}

/// Background AVFoundation video device discovery.
pub struct Discovery {
    devices: Arc<Mutex<Vec<Device>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let devices2 = devices.clone();
        std::thread::Builder::new()
            .name("avfoundation-discovery".into())
            .spawn(move || {
                loop {
                    let list = unsafe {
                        AVCaptureDevice::devices_with_media_type(AVMediaTypeVideo)
                    };
                    let mut discovered = Vec::new();
                    for device in list.iter() {
                        let unique_id = device.unique_id().to_string();
                        let name = device.localized_name().to_string();
                        discovered.push(Device { unique_id, name });
                    }
                    *devices2.lock().unwrap() = discovered;
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn avfoundation-discovery");
        Self { devices }
    }

    pub fn list(&self) -> Vec<Device> {
        self.devices.lock().unwrap().clone()
    }

    pub fn find_by_name(&self, name: &str) -> Option<Device> {
        self.devices.lock().unwrap().iter().find(|d| d.name == name).cloned()
    }
}
