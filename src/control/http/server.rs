use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::api::{self, ApiHandles};
use super::config::HttpControlConfig;

const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub enum ListenerState {
    #[default]
    Stopped,
    Listening(SocketAddr),
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: ListenerState,
}

impl Status {
    pub fn label(&self, clients: usize) -> String {
        return match &self.state {
            ListenerState::Stopped => "HTTP stopped".to_string(),
            ListenerState::Listening(addr) => {
                let noun = if clients == 1 { "client" } else { "clients" };
                format!("HTTP listening on {addr} · {clients} {noun}")
            }
        };
    }
}

/// The HTTP API and web UI listener. Binds on the caller's thread so a bad
/// address or a taken port is reported before the app claims to be serving.
pub struct Server {
    status: Arc<Mutex<Status>>,
    clients: Arc<AtomicUsize>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    done: Option<mpsc::Receiver<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(config: &HttpControlConfig, handles: ApiHandles) -> Result<Self, String> {
        let listener =
            TcpListener::bind((config.bind.trim(), config.port)).map_err(|error| format!("bind {}:{}: {error}", config.bind.trim(), config.port))?;
        let addr = listener.local_addr().map_err(|error| format!("listener address: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| format!("listener nonblocking: {error}"))?;

        let clients = handles.clients.clone();
        let status = Arc::new(Mutex::new(Status {
            state: ListenerState::Listening(addr),
        }));
        let router = api::router(handles, status.clone(), config.bind.clone());
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("http-control".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!("HTTP control runtime: {error}");
                        let _ = done_tx.send(());
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(error) => {
                            tracing::error!("HTTP control listener: {error}");
                            let _ = done_tx.send(());
                            return;
                        }
                    };
                    let served = axum::serve(listener, router)
                        .with_graceful_shutdown(async {
                            let _ = stop_rx.await;
                        })
                        .await;
                    if let Err(error) = served {
                        tracing::error!("HTTP control stopped: {error}");
                    }
                    let _ = done_tx.send(());
                });
            })
            .map_err(|error| format!("spawn HTTP control: {error}"))?;

        tracing::info!("HTTP control listening on {addr}");
        return Ok(Self {
            status,
            clients,
            stop: Some(stop_tx),
            done: Some(done_rx),
            thread: Some(thread),
        });
    }

    #[cfg(test)]
    pub fn status(&self) -> Status {
        return self.status.lock().unwrap().clone();
    }

    /// The Arc the session reads for the web settings status line.
    pub fn status_handle(&self) -> Arc<Mutex<Status>> {
        return self.status.clone();
    }

    pub fn status_label(&self) -> String {
        let clients = self.clients.load(Ordering::Relaxed);
        return self.status.lock().unwrap().label(clients);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Bounded: a handler can be waiting on the engine lock or a blocking
        // SD card write. The runtime is already shutting down; detach.
        if let Some(done) = self.done.take() {
            let _ = done.recv_timeout(SHUTDOWN_GRACE);
        }
        if let Some(thread) = self.thread.take() {
            drop(thread);
        }
    }
}
