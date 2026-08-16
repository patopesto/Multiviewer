use std::sync::{Arc, Mutex};
use std::time::Duration;
use syphon_core::SyphonServerDirectory;

/// Background Syphon server discovery.
pub struct Discovery {
    servers: Arc<Mutex<Vec<String>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let servers = Arc::new(Mutex::new(Vec::new()));
        let servers2 = servers.clone();
        std::thread::Builder::new()
            .name("syphon-discovery".into())
            .spawn(move || {
                loop {
                    let list = SyphonServerDirectory::servers();
                    let names: Vec<String> =
                        list.iter().map(|s| s.display_name().to_string()).collect();
                    *servers2.lock().unwrap() = names;
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn syphon-discovery");
        Self { servers }
    }

    pub fn list(&self) -> Vec<String> {
        self.servers.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn find_by_name(&self, name: &str) -> Option<syphon_core::ServerInfo> {
        let list = SyphonServerDirectory::servers();
        list.into_iter().find(|s| s.display_name() == name)
    }
}
