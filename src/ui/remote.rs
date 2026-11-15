use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use egui::epaint::ImageData;
use tokio::sync::broadcast;

use crate::control::{
    ApiHandles, Command, Frame, HttpControlConfig, InputEvent, PointerButton, Server, Status,
    TextureUpload, encode_frame, texture_key,
};
use crate::engine::{Engine, FocusHandle, PreviewFrame, SnapshotSlot, publish_state};
use crate::ui::viewer::{HostView, UiMode, Viewer};

const SESSION_FPS: f64 = 30.0;
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);
const RETIRE_GRACE: Duration = Duration::from_millis(1000);
const RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_TEXTURE_SIDE: usize = 4096;

/// Owns the HTTP listener and the remote egui session. Both start and stop
/// from the project's `control.http` config, so the native window and
/// `--headless` share the same lifecycle.
pub struct RemoteHost {
    engine: Arc<Mutex<Engine>>,
    snapshots: SnapshotSlot,
    commands: Sender<Command>,
    focus: FocusHandle,
    running: Option<Running>,
    /// Servers shutting down in the background, keyed by their port.
    retiring: Vec<(u16, Receiver<()>)>,
    applied: Option<HttpControlConfig>,
    error: Option<String>,
    retry_at: Option<Instant>,
}

struct Running {
    server: Server,
    /// Dropped with the server; kept alive for its thread.
    _session: Session,
}

impl RemoteHost {
    pub fn new(
        engine: Arc<Mutex<Engine>>,
        snapshots: SnapshotSlot,
        commands: Sender<Command>,
        focus: FocusHandle,
    ) -> Self {
        return Self {
            engine,
            snapshots,
            commands,
            focus,
            running: None,
            retiring: Vec::new(),
            applied: None,
            error: None,
            retry_at: None,
        };
    }

    /// Start, stop or restart the listener to match the current project.
    pub fn sync(&mut self) {
        let desired = {
            let slot = self.snapshots.lock().unwrap();
            let Some(snapshot) = slot.as_ref() else {
                return;
            };
            snapshot.state.cfg.control.http.clone()
        };
        if self.applied.as_ref() == Some(&desired) {
            if self.running.is_some() == desired.enabled {
                return;
            }
            if self.retry_at.is_some_and(|retry_at| Instant::now() < retry_at) {
                return;
            }
        }

        self.retiring.retain(|(_, done)| {
            matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty))
        });
        let previous = self.applied.clone();
        let wait_for_port = desired.enabled
            && self.running.is_some()
            && previous
                .as_ref()
                .is_some_and(|old| old.port == desired.port);
        if let Some(running) = self.running.take() {
            if wait_for_port {
                drop(running);
            } else if let Some(done) = shutdown_in_background(running)
                && let Some(previous) = &previous
            {
                self.retiring.push((previous.port, done));
            }
        }
        self.error = None;
        self.retry_at = None;
        self.applied = Some(desired.clone());
        if !desired.enabled {
            return;
        }
        self.wait_for_retiring(desired.port);
        match self.start(&desired) {
            Ok(running) => self.running = Some(running),
            Err(error) => {
                tracing::warn!("HTTP control failed to start: {error}");
                self.error = Some(error);
                self.retry_at = Some(Instant::now() + RETRY_DELAY);
            }
        }
    }

    fn start(&self, config: &HttpControlConfig) -> Result<Running, String> {
        let (frames, _) = broadcast::channel(8);
        let (inputs, input_rx) = mpsc::channel();
        let clients = Arc::new(AtomicUsize::new(0));

        let handles = ApiHandles {
            engine: self.engine.clone(),
            snapshots: self.snapshots.clone(),
            frames: frames.clone(),
            inputs,
            clients: clients.clone(),
        };
        let server = Server::start(config, handles)?;
        let status = server.status_handle();
        let session = Session::start(SessionConfig {
            engine: self.engine.clone(),
            snapshots: self.snapshots.clone(),
            commands: self.commands.clone(),
            focus: self.focus.clone(),
            frames,
            inputs: input_rx,
            clients,
            status,
        });
        return Ok(Running {
            server,
            _session: session,
        });
    }

    pub fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = shutdown_in_background(running);
        }
        self.applied = None;
    }

    /// Wait for an older server on the same port to finish closing before a
    /// new one binds. This closes the off/on and A -> B -> A restart races.
    fn wait_for_retiring(&mut self, port: u16) {
        let mut index = 0;
        while index < self.retiring.len() {
            if self.retiring[index].0 == port {
                let (_, done) = self.retiring.remove(index);
                let _ = done.recv_timeout(RETIRE_GRACE);
            } else {
                index += 1;
            }
        }
    }

    /// Listener state for the Control settings window.
    pub fn status_label(&self) -> Option<String> {
        if let Some(running) = &self.running {
            return Some(running.server.status_label());
        }
        return self.error.as_ref().map(|error| format!("HTTP error: {error}"));
    }
}

impl Drop for RemoteHost {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = shutdown_in_background(running);
        }
    }
}

/// The server's graceful shutdown can wait for an in-flight request, and the
/// session join can wait for an engine lock. Neither belongs on the paint
/// thread, so the handles are dropped on a short-lived thread. The receiver
/// signals when the old listener is fully gone.
fn shutdown_in_background(running: Running) -> Option<Receiver<()>> {
    let (done_tx, done) = mpsc::channel();
    match thread::Builder::new()
        .name("http-shutdown".to_string())
        .spawn(move || {
            drop(running);
            let _ = done_tx.send(());
        }) {
        Ok(_) => Some(done),
        Err(error) => {
            tracing::warn!("HTTP shutdown thread: {error}");
            None
        }
    }
}

struct SessionConfig {
    engine: Arc<Mutex<Engine>>,
    snapshots: SnapshotSlot,
    commands: Sender<Command>,
    focus: FocusHandle,
    frames: broadcast::Sender<Frame>,
    inputs: Receiver<InputEvent>,
    clients: Arc<AtomicUsize>,
    status: Arc<Mutex<Status>>,
}

struct Session {
    stop: Arc<AtomicBool>,
    done: Receiver<()>,
}

impl Session {
    fn start(config: SessionConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = mpsc::channel();
        let thread_stop = stop.clone();
        let spawned = thread::Builder::new()
            .name("web-session".to_string())
            .spawn(move || {
                run_session(config, thread_stop);
                let _ = done_tx.send(());
            });
        if let Err(error) = spawned {
            tracing::error!("web session thread: {error}");
        }
        return Self { stop, done };
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.done.recv_timeout(SHUTDOWN_GRACE);
    }
}

struct InputState {
    size: egui::Vec2,
    dpr: f32,
    focused: bool,
    pointer: egui::Pos2,
    modifiers: egui::Modifiers,
    events: Vec<egui::Event>,
    buttons: [bool; 3],
    keyframe: bool,
}

impl InputState {
    fn new() -> Self {
        return Self {
            size: egui::vec2(1600.0, 900.0),
            dpr: 1.0,
            focused: true,
            pointer: egui::Pos2::ZERO,
            modifiers: egui::Modifiers::default(),
            events: Vec::new(),
            buttons: [false; 3],
            keyframe: true,
        };
    }

    fn handle_client_gone(&mut self) {
        for (index, down) in self.buttons.iter_mut().enumerate() {
            if *down {
                *down = false;
                self.events.push(egui::Event::PointerButton {
                    pos: self.pointer,
                    button: index_button(index),
                    pressed: false,
                    modifiers: self.modifiers,
                });
            }
        }
        self.events.push(egui::Event::PointerGone);
    }

    fn handle(&mut self, event: InputEvent) {
        match event {
            InputEvent::Hello {
                width,
                height,
                dpr,
            }
            | InputEvent::Resize {
                width,
                height,
                dpr,
            } => {
                self.size = egui::vec2(width.max(1.0), height.max(1.0));
                self.dpr = dpr.clamp(0.5, 4.0);
            }
            InputEvent::PointerMove { x, y } => {
                self.pointer = egui::pos2(x, y);
                self.events.push(egui::Event::PointerMoved(self.pointer));
            }
            InputEvent::PointerButton {
                button,
                pressed,
                x,
                y,
            } => {
                self.pointer = egui::pos2(x, y);
                let index = match button {
                    PointerButton::Primary => 0,
                    PointerButton::Secondary => 2,
                    PointerButton::Middle => 1,
                };
                self.buttons[index] = pressed;
                self.events.push(egui::Event::PointerButton {
                    pos: self.pointer,
                    button: button.to_egui(),
                    pressed,
                    modifiers: self.modifiers,
                });
            }
            InputEvent::Scroll {
                dx,
                dy,
                alt,
                ctrl,
                shift,
                command,
            } => {
                self.modifiers = egui::Modifiers {
                    alt,
                    ctrl,
                    shift,
                    mac_cmd: command,
                    command,
                };
                self.events
                    .push(egui::Event::ModifiersChanged(self.modifiers));
                self.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(dx, dy),
                    phase: egui::TouchPhase::Move,
                    modifiers: self.modifiers,
                });
            }
            InputEvent::Key {
                key,
                pressed,
                repeat,
                alt,
                ctrl,
                shift,
                command,
            } => {
                self.modifiers = egui::Modifiers {
                    alt,
                    ctrl,
                    shift,
                    mac_cmd: command,
                    command,
                };
                if let Some(key) = egui::Key::from_name(&key) {
                    self.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat,
                        modifiers: self.modifiers,
                    });
                }
            }
            InputEvent::Text { text } => {
                if !text.is_empty() {
                    self.events.push(egui::Event::Text(text));
                }
            }
            InputEvent::Focus { focused } => {
                self.focused = focused;
                self.events.push(egui::Event::WindowFocused(focused));
            }
            InputEvent::ClientGone => self.handle_client_gone(),
            InputEvent::Keyframe => self.keyframe = true,
        }
    }
}

/// A client leaving releases the shared pointer only when nobody else is left.
fn feed(input: &mut InputState, event: InputEvent, clients: usize) {
    if matches!(event, InputEvent::ClientGone) && clients > 0 {
        return;
    }
    input.handle(event);
}

fn index_button(index: usize) -> egui::PointerButton {
    return match index {
        1 => egui::PointerButton::Middle,
        2 => egui::PointerButton::Secondary,
        _ => egui::PointerButton::Primary,
    };
}

fn run_session(config: SessionConfig, stop: Arc<AtomicBool>) {
    let ctx = egui::Context::default();
    super::configure(&ctx);
    let mut viewer = Viewer::new(UiMode::Remote);
    let mut input = InputState::new();
    let mut textures = TextureCache::default();
    let started = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / SESSION_FPS);
    let mut next_frame = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        match config.inputs.recv_timeout(frame_interval) {
            Ok(event) => {
                feed(&mut input, event, config.clients.load(Ordering::Relaxed));
                while let Ok(event) = config.inputs.try_recv() {
                    feed(&mut input, event, config.clients.load(Ordering::Relaxed));
                    if input.events.len() > 256 {
                        break;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        if now < next_frame {
            continue;
        }
        next_frame = now + frame_interval;

        let connected = config.clients.load(Ordering::Relaxed) > 0;
        if !connected {
            if input.buttons.iter().any(|down| *down) {
                input.handle_client_gone();
            }
            input.events.clear();
            continue;
        }

        let snapshot = {
            let slot = config.snapshots.lock().unwrap();
            slot.clone()
        };
        let Some(snapshot) = snapshot else {
            continue;
        };

        let events = std::mem::take(&mut input.events);
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(input.size.x.max(1.0), input.size.y.max(1.0)),
        );
        let mut viewports = egui::ViewportIdMap::default();
        viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(input.dpr),
                inner_rect: Some(screen),
                focused: Some(input.focused),
                ..Default::default()
            },
        );
        let raw_input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(started.elapsed().as_secs_f64()),
            events,
            focused: input.focused,
            max_texture_side: Some(MAX_TEXTURE_SIDE),
            system_theme: Some(egui::Theme::Dark),
            viewports,
            ..Default::default()
        };
        ctx.set_pixels_per_point(input.dpr);

        let keyframe = input.keyframe;
        input.keyframe = false;
        let web_status = config
            .status
            .lock()
            .unwrap()
            .label(config.clients.load(Ordering::Relaxed));
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(raw_input, |ui| {
            let host = HostView {
                preview: None::<&PreviewFrame>,
                focus: Some(&config.focus),
                status: "remote",
                status_tooltip: "This interface runs on the Multiviewer host and is streamed to the browser",
                web_status: Some(&web_status),
            };
            let out = viewer.draw(ui, &snapshot, &config.commands, host);
            actions = out.actions;
        });

        if !actions.is_empty() {
            let mut engine = config.engine.lock().unwrap();
            for action in &actions {
                if let Err(error) = viewer.run_action(&mut engine, action) {
                    viewer.last_error = Some(error);
                }
            }
            publish_state(&engine, &config.snapshots);
        }

        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        // epaint panics if a delta is dropped unapplied, so take it and clear it.
        let mut textures_delta = std::mem::take(&mut output.textures_delta);
        let mut uploads = Vec::new();
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                textures.apply(*id, delta, &mut uploads);
            }
        }
        for id in &textures_delta.free {
            textures.free(*id);
        }
        if keyframe {
            uploads = textures.full_uploads();
        }
        let free: Vec<u32> = textures_delta
            .free
            .iter()
            .map(|id| texture_key(*id))
            .collect();
        textures_delta.clear();
        let frame = encode_frame(&primitives, &uploads, &free);
        let _ = config.frames.send(frame);
    }
}

/// Keep every texture so a new browser can receive a keyframe.
#[derive(Default)]
struct TextureCache {
    textures: HashMap<u32, CachedTexture>,
}

struct CachedTexture {
    size: [u16; 2],
    pixels: Vec<u8>,
}

impl TextureCache {
    fn apply(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta, uploads: &mut Vec<TextureUpload>) {
        let ImageData::Color(image) = &delta.image;
        let size = [image.size[0] as u16, image.size[1] as u16];
        let pixels: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|color| [color.r(), color.g(), color.b(), color.a()])
            .collect();
        let key = texture_key(id);
        match delta.pos {
            None => {
                self.textures.insert(
                    key,
                    CachedTexture {
                        size,
                        pixels: pixels.clone(),
                    },
                );
                uploads.push(TextureUpload {
                    id: key,
                    full: true,
                    pos: [0, 0],
                    size,
                    pixels,
                });
            }
            Some(pos) => {
                let Some(cached) = self.textures.get_mut(&key) else {
                    return;
                };
                let width = cached.size[0] as usize;
                let patch_width = size[0] as usize;
                for row in 0..size[1] as usize {
                    let source = row * patch_width * 4;
                    let destination = ((pos[1] + row) * width + pos[0]) * 4;
                    let end = destination + patch_width * 4;
                    if end <= cached.pixels.len() && source + patch_width * 4 <= pixels.len() {
                        cached.pixels[destination..end]
                            .copy_from_slice(&pixels[source..source + patch_width * 4]);
                    }
                }
                uploads.push(TextureUpload {
                    id: key,
                    full: false,
                    pos: [pos[0] as u16, pos[1] as u16],
                    size,
                    pixels,
                });
            }
        }
    }

    fn free(&mut self, id: egui::TextureId) {
        self.textures.remove(&texture_key(id));
    }

    fn full_uploads(&self) -> Vec<TextureUpload> {
        return self
            .textures
            .iter()
            .map(|(id, texture)| TextureUpload {
                id: *id,
                full: true,
                pos: [0, 0],
                size: texture.size,
                pixels: texture.pixels.clone(),
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Canvas;

    #[test]
    fn texture_cache_patches_and_rebuilds_a_keyframe() {
        let mut cache = TextureCache::default();
        let mut uploads = Vec::new();
        let full = egui::epaint::ImageDelta::full(
            egui::epaint::ImageData::Color(std::sync::Arc::new(egui::ColorImage::new(
                [2, 2],
                vec![egui::Color32::RED; 4],
            ))),
            egui::TextureOptions::LINEAR,
        );
        cache.apply(egui::TextureId::Managed(0), &full, &mut uploads);
        assert_eq!(uploads.len(), 1);
        assert!(uploads[0].full);
        assert_eq!(uploads[0].size, [2, 2]);

        uploads.clear();
        let patch = egui::epaint::ImageDelta {
            image: egui::epaint::ImageData::Color(std::sync::Arc::new(egui::ColorImage::new(
                [1, 1],
                vec![egui::Color32::BLUE],
            ))),
            options: egui::TextureOptions::LINEAR,
            pos: Some([1, 0]),
        };
        cache.apply(egui::TextureId::Managed(0), &patch, &mut uploads);
        assert_eq!(uploads.len(), 1);
        assert!(!uploads[0].full);
        assert_eq!(uploads[0].pos, [1, 0]);
        assert_eq!(&uploads[0].pixels, &[0, 0, 255, 255]);

        let all = cache.full_uploads();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].size, [2, 2]);
        assert_eq!(&all[0].pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&all[0].pixels[4..8], &[0, 0, 255, 255]);

        cache.free(egui::TextureId::Managed(0));
        assert!(cache.full_uploads().is_empty());
    }

    #[test]
    fn the_remote_viewer_draws_a_tessellatable_frame() {
        let mut engine = Engine::test_instance(Canvas::default());
        engine.add_source();
        let snapshot = engine.snapshot();
        let (commands, _) = mpsc::channel();
        let ctx = egui::Context::default();
        let mut viewer = Viewer::new(UiMode::Remote);

        for http in [false, true] {
            if http {
                viewer.open_http_settings(&snapshot);
            } else {
                viewer.open_osc_settings(&snapshot);
            }
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let out = viewer.draw(
                        ui,
                        &snapshot,
                        &commands,
                        HostView {
                            preview: None,
                            focus: None,
                            status: "test",
                            status_tooltip: "",
                            web_status: None,
                        },
                    );
                    actions = out.actions;
                },
            );

            let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
            let mut textures = std::mem::take(&mut output.textures_delta);
            assert!(
                primitives.iter().any(|primitive| matches!(
                    primitive.primitive,
                    egui::epaint::Primitive::Mesh(_)
                )),
                "the remote viewer produced no meshes"
            );
            let frame = encode_frame(&primitives, &[], &[]);
            assert!(frame.len() > 28);
            textures.clear();
            assert!(actions.is_empty());
        }
    }
}
