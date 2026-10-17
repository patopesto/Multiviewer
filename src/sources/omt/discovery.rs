use std::sync::{Arc, Mutex};
use std::time::Duration;

use openmediatransport::Discovery as OmtDiscoveryClient;

use super::OmtSourceInfo;

/// Background OMT discovery thread (DNS-SD, or a discovery server from settings.xml).
pub struct Discovery {
    sources: Arc<Mutex<Vec<OmtSourceInfo>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let sources = Arc::new(Mutex::new(Vec::new()));
        let sources2 = sources.clone();
        std::thread::Builder::new()
            .name("omt-discovery".into())
            .spawn(move || {
                let mut discovery = match OmtDiscoveryClient::new() {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("OMT discovery init failed: {}", e);
                        return;
                    }
                };
                loop {
                    // `refresh_for` blocks for the mDNS browse window.
                    match discovery.refresh_for(Duration::from_millis(1000)) {
                        Ok(()) => *sources2.lock().unwrap() = discovery.sources().to_vec(),
                        Err(e) => tracing::warn!("OMT discovery error: {}", e),
                    }
                }
            })
            .expect("spawn omt-discovery");
        Self { sources }
    }

    pub fn list(&self) -> Vec<OmtSourceInfo> {
        return self.sources.lock().unwrap().clone();
    }

    pub fn find_by_name(&self, name: &str) -> Option<OmtSourceInfo> {
        return self
            .sources
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.instance_name() == name)
            .cloned();
    }
}
