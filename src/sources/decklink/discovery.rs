use std::ffi::c_char;
use std::sync::{Arc, Mutex};

use multiviewer_decklink::{
    DisplayMode, VideoConnections, decklink_discovery_free, decklink_discovery_input_count,
    decklink_discovery_input_get, decklink_discovery_new, decklink_discovery_output_count,
    decklink_discovery_output_get, decklink_discovery_output_mode,
};

use crate::sources::PollGate;

/// A discovered DeckLink input port.
#[derive(Clone)]
pub struct InputPort {
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
    input_ports: Arc<Mutex<Vec<InputPort>>>,
    output_ports: Arc<Mutex<Vec<OutputPort>>>,
    gate: PollGate,
}

impl Discovery {
    pub fn start() -> Self {
        let input_ports = Arc::new(Mutex::new(Vec::new()));
        let input_ports2 = input_ports.clone();
        let output_ports = Arc::new(Mutex::new(Vec::new()));
        let output_ports2 = output_ports.clone();
        let gate = PollGate::new();
        let gate2 = gate.clone();
        std::thread::Builder::new()
            .name("decklink-discovery".into())
            .spawn(move || unsafe {
                loop {
                    let d = decklink_discovery_new();
                    if d.is_null() {
                        tracing::error!("DeckLink discovery init failed");
                        gate2.wait();
                        continue;
                    }

                    let input_count = decklink_discovery_input_count(d);
                    let mut inputs = Vec::with_capacity(input_count as usize);
                    for i in 0..input_count {
                        let mut name = [0u8; 256];
                        let mut has_signal = false;
                        let mut connections = 0u32;
                        decklink_discovery_input_get(
                            d,
                            i,
                            name.as_mut_ptr() as *mut c_char,
                            name.len(),
                            &mut has_signal,
                            &mut connections,
                        );
                        let name_len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                        let name = String::from_utf8_lossy(&name[..name_len]).to_string();
                        inputs.push(InputPort {
                            name,
                            has_signal,
                            connections: VideoConnections(connections),
                        });
                    }
                    *input_ports2.lock().unwrap() = inputs;

                    let output_count = decklink_discovery_output_count(d);
                    let mut outputs = Vec::with_capacity(output_count as usize);
                    for i in 0..output_count {
                        let mut name = [0u8; 256];
                        let mut mode_count = 0i32;
                        decklink_discovery_output_get(
                            d,
                            i,
                            name.as_mut_ptr() as *mut c_char,
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
                            decklink_discovery_output_mode(
                                d,
                                i,
                                m,
                                mode_name.as_mut_ptr() as *mut c_char,
                                mode_name.len(),
                                &mut mode_id,
                                &mut w,
                                &mut h,
                                &mut fps,
                            );
                            let mode_name_len =
                                mode_name.iter().position(|&b| b == 0).unwrap_or(mode_name.len());
                            let mode_name =
                                String::from_utf8_lossy(&mode_name[..mode_name_len]).to_string();
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
                        outputs.push(OutputPort { name, modes });
                    }
                    *output_ports2.lock().unwrap() = outputs;

                    decklink_discovery_free(d);
                    gate2.wait();
                }
            })
            .expect("spawn decklink-discovery");
        Self {
            input_ports,
            output_ports,
            gate,
        }
    }

    /// Refresh now instead of waiting out the idle interval.
    pub fn request_refresh(&self) {
        self.gate.request();
    }

    pub fn list(&self) -> Vec<InputPort> {
        self.input_ports.lock().unwrap().clone()
    }

    pub fn find_by_name(&self, name: &str) -> Option<InputPort> {
        self.input_ports
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
