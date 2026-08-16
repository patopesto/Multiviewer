use multiviewer_decklink::VideoConnections;
use multiviewer_decklink::{decklink_discovery_new, decklink_discovery_free, decklink_discovery_count, decklink_discovery_get};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A discovered DeckLink input port.
#[derive(Clone)]
pub struct Port {
    pub name: String,
    pub has_signal: bool,
    pub connections: VideoConnections,
}

/// Background DeckLink discovery thread.
pub struct Discovery {
    ports: Arc<Mutex<Vec<Port>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let ports = Arc::new(Mutex::new(Vec::new()));
        let ports2 = ports.clone();
        std::thread::Builder::new()
            .name("decklink-discovery".into())
            .spawn(move || unsafe {
                loop {
                    let d = decklink_discovery_new();
                    if d.is_null() {
                        tracing::error!("DeckLink discovery init failed");
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                    let count = decklink_discovery_count(d);
                    let mut list = Vec::with_capacity(count as usize);
                    for i in 0..count {
                        let mut name = [0u8; 256];
                        let mut has_signal = false;
                        let mut connections = 0u32;
                        decklink_discovery_get(
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
                    decklink_discovery_free(d);
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn decklink-discovery");
        Self { ports }
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
}
