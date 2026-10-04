use std::sync::{Arc, Mutex};
use std::time::Duration;
use syphon_core::SyphonServerDirectory;

use super::SyphonServerInfo;

/// UI label for a discovered server: `"<app> - <name>"`.
pub fn format_syphon_label(app_name: &str, name: &str) -> String {
    if name.is_empty() {
        return app_name.to_string();
    }
    if app_name.is_empty() {
        return name.to_string();
    }
    format!("{app_name} - {name}")
}

/// Background Syphon server discovery.
pub struct Discovery {
    servers: Arc<Mutex<Vec<SyphonServerInfo>>>,
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
                    *servers2.lock().unwrap() = list;
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn syphon-discovery");
        Self { servers }
    }

    pub fn list(&self) -> Vec<SyphonServerInfo> {
        self.servers.lock().unwrap().clone()
    }

    pub fn find_by_display_name(&self, name: &str) -> Option<SyphonServerInfo> {
        SyphonServerDirectory::servers()
            .into_iter()
            .find(|s| s.display_name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::format_syphon_label;

    #[test]
    fn label_joins_app_and_name() {
        assert_eq!(format_syphon_label("Resolume", "output"), "Resolume - output");
    }

    #[test]
    fn label_falls_back_when_one_side_is_empty() {
        assert_eq!(format_syphon_label("Resolume", ""), "Resolume");
        assert_eq!(format_syphon_label("", "output"), "output");
        assert_eq!(format_syphon_label("", ""), "");
    }
}
