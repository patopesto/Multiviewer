use multiviewer_decklink::{DisplayMode, VideoConnections};
use multiviewer_decklink::{
    decklink_source_discovery_new, decklink_source_discovery_free, decklink_source_discovery_count, decklink_source_discovery_get,
};
use multiviewer_decklink::{
    decklink_output_discovery_new, decklink_output_discovery_free, decklink_output_discovery_count,
    decklink_output_discovery_get, decklink_output_discovery_get_mode,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A discovered DeckLink input port.
#[derive(Clone)]
pub struct Port {
    pub name: String,
    #[allow(dead_code)]
    pub has_signal: bool,
    pub connections: VideoConnections,
}

/// A discovered DeckLink output display mode.
#[derive(Clone)]
pub struct OutputMode {
    pub name: String,
    pub mode: DisplayMode,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

/// A discovered DeckLink output port.
#[derive(Clone)]
pub struct OutputPort {
    pub name: String,
    pub modes: Vec<OutputMode>,
}

/// Background DeckLink discovery thread.
pub struct Discovery {
    ports: Arc<Mutex<Vec<Port>>>,
    output_ports: Arc<Mutex<Vec<OutputPort>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let ports = Arc::new(Mutex::new(Vec::new()));
        let ports2 = ports.clone();
        let output_ports = Arc::new(Mutex::new(Vec::new()));
        let output_ports2 = output_ports.clone();
        std::thread::Builder::new()
            .name("decklink-discovery".into())
            .spawn(move || unsafe {
                loop {
                    // Input discovery
                    let d = decklink_source_discovery_new();
                    if d.is_null() {
                        tracing::error!("DeckLink discovery init failed");
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                    let count = decklink_source_discovery_count(d);
                    let mut list = Vec::with_capacity(count as usize);
                    for i in 0..count {
                        let mut name = [0u8; 256];
                        let mut has_signal = false;
                        let mut connections = 0u32;
                        decklink_source_discovery_get(
                            d,
                            i,
                            name.as_mut_ptr() as *mut std::ffi::c_char,
                            name.len(),
                            &mut has_signal,
                            &mut connections,
                        );
                        let name_len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                        let name = String::from_utf8_lossy(&name[..name_len]).to_string();
                        list.push(Port {
                            name,
                            has_signal,
                            connections: VideoConnections(connections),
                        });
                    }
                    *ports2.lock().unwrap() = list;
                    decklink_source_discovery_free(d);

                    // Output discovery
                    let od = decklink_output_discovery_new();
                    if !od.is_null() {
                        let out_count = decklink_output_discovery_count(od);
                        let mut out_list = Vec::with_capacity(out_count as usize);
                        for i in 0..out_count {
                            let mut name = [0u8; 256];
                            let mut mode_count = 0i32;
                            decklink_output_discovery_get(
                                od,
                                i,
                                name.as_mut_ptr() as *mut std::ffi::c_char,
                                name.len(),
                                &mut mode_count,
                            );
                            let name_len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                            let name = String::from_utf8_lossy(&name[..name_len]).to_string();

                            let mut modes = Vec::with_capacity(mode_count as usize);
                            for m in 0..mode_count {
                                let mut mode_name = [0u8; 256];
                                let mut mode_id = 0u32;
                                let mut w = 0i32;
                                let mut h = 0i32;
                                let mut fps = 0.0f64;
                                decklink_output_discovery_get_mode(
                                    od,
                                    i,
                                    m,
                                    mode_name.as_mut_ptr() as *mut std::ffi::c_char,
                                    mode_name.len(),
                                    &mut mode_id,
                                    &mut w,
                                    &mut h,
                                    &mut fps,
                                );
                                let mode_name_len = mode_name.iter().position(|&b| b == 0).unwrap_or(mode_name.len());
                                let mode_name = String::from_utf8_lossy(&mode_name[..mode_name_len]).to_string();
                                if let Ok(mode) = DisplayMode::try_from(mode_id) {
                                    modes.push(OutputMode {
                                        name: mode_name,
                                        mode,
                                        width: w as u32,
                                        height: h as u32,
                                        fps,
                                    });
                                }
                            }
                            out_list.push(OutputPort { name, modes });
                        }
                        *output_ports2.lock().unwrap() = out_list;
                        decklink_output_discovery_free(od);
                    }

                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn decklink-discovery");
        Self { ports, output_ports }
    }

    pub fn list(&self) -> Vec<Port> {
        self.ports.lock().unwrap().clone()
    }

    pub fn find_by_name(&self, name: &str) -> Option<Port> {
        self.ports
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    pub fn list_outputs(&self) -> Vec<OutputPort> {
        self.output_ports.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn find_output_by_name(&self, name: &str) -> Option<OutputPort> {
        self.output_ports
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }
}
