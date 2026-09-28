use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Background Spout sender discovery.
///
/// Mirrors the Syphon discovery thread: a loop enumerating the sender registry
/// every two seconds. The `Receiver` is `!Send`, so it is created on (and
/// dropped on) this thread and never leaves it.
pub struct Discovery {
    senders: Arc<Mutex<Vec<String>>>,
}

impl Discovery {
    pub fn start() -> Self {
        let senders = Arc::new(Mutex::new(Vec::new()));
        let senders2 = senders.clone();
        std::thread::Builder::new()
            .name("spout-discovery".into())
            .spawn(move || {
                // Enumeration only touches shared memory; no device is opened
                // until the first receive call, which we never make here.
                let Ok(receiver) = spout2::dx12::Receiver::new(None) else {
                    tracing::error!("Spout discovery: failed to create receiver");
                    return;
                };
                loop {
                    let names = receiver.sender_list();
                    *senders2.lock().unwrap() = names;
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn spout-discovery");
        Self { senders }
    }

    pub fn list(&self) -> Vec<String> {
        self.senders.lock().unwrap().clone()
    }
}
