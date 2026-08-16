use grafton_ndi::{Finder, FinderOptions, NDI};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Background NDI discovery thread.
pub struct Discovery {
    sources: Arc<Mutex<Vec<grafton_ndi::Source>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let sources = Arc::new(Mutex::new(Vec::new()));
        let sources2 = sources.clone();
        std::thread::Builder::new()
            .name("ndi-discovery".into())
            .spawn(move || {
                let ndi = match NDI::new() {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!("NDI init failed in discovery: {e}");
                        return;
                    }
                };
                let finder = match Finder::new(
                    &ndi,
                    &FinderOptions::builder().show_local_sources(true).build(),
                ) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::error!("NDI Finder failed: {e}");
                        return;
                    }
                };
                loop {
                    match finder.current_sources() {
                        Ok(list) => {
                            let mut lock = sources2.lock().unwrap();
                            *lock = list;
                        }
                        Err(e) => tracing::warn!("NDI discovery error: {e}"),
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn ndi-discovery");
        Self { sources }
    }

    pub fn list(&self) -> Vec<grafton_ndi::Source> {
        self.sources.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn find_by_name(&self, name: &str) -> Option<grafton_ndi::Source> {
        self.sources
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.name == name)
            .cloned()
    }
}
