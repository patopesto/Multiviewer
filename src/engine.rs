use crate::compositor::{self, Compositor, Draw, Rect};
use crate::config::{Config, Layer, Protocol, TextureMode};
use crate::sources::decklink::Discovery as DecklinkDiscovery;
use crate::sources::ndi::Discovery;
use crate::sources::Registry;

#[cfg(target_os = "macos")]
use crate::sources::syphon::Discovery as SyphonDiscovery;

pub struct Engine {
    pub cfg: Config,
    pub registry: Registry,
    pub ndi: Option<Discovery>,
    pub decklink: Option<DecklinkDiscovery>,
    #[cfg(target_os = "macos")]
    pub syphon: Option<SyphonDiscovery>,
    comp: Option<Compositor>,
    pub dirty: bool,
    pub selected_layer_id: Option<String>,
    pub dragging_uuid: Option<String>,
}

impl Engine {
    pub fn new() -> Self {
        let mut cfg = Config::load();
        let mut registry = Registry::new();
        let mut dirty = false;

        // Start NDI discovery before restoring sources
        let ndi = Discovery::start();

        // Start DeckLink discovery
        let decklink = DecklinkDiscovery::start();

        // Start Syphon discovery on macOS
        #[cfg(target_os = "macos")]
        let syphon = Some(SyphonDiscovery::start());

        // Restore Test sources for all Test layers in the loaded config.
        // Each Test layer gets a fresh dedicated test source.
        for (i, layer) in cfg.canvas.layers.iter_mut().enumerate() {
            if layer.uuid.is_empty() {
                layer.uuid = uuid::Uuid::new_v4().to_string();
                dirty = true;
            }
            if layer.name.is_empty() {
                layer.name = format!("Layer {}", i + 1);
                dirty = true;
            }
            if layer.protocol == Protocol::Test {
                let sid = registry.add_test();
                layer.source_id = Some(sid);
                dirty = true;
            }
            // NDI and Syphon layers keep their source_id; auto-connect happens in update()
        }

        // Seed demo layout if nothing was loaded
        if cfg.canvas.layers.is_empty() {
            let w = cfg.canvas.width as f32;
            let h = cfg.canvas.height as f32;
            for i in 0..2 {
                let sid = registry.add_test();
                let col = i % 2;
                let row = i / 2;
                cfg.canvas.layers.push(Layer::new_v4(
                    format!("Layer {}", i + 1),
                    Protocol::Test,
                    Some(sid),
                    col as f32 * w / 2.0,
                    row as f32 * h / 2.0,
                    (w / 2.0) as u32,
                    (h / 2.0) as u32,
                    i,
                    TextureMode::Fit,
                    false,
                    false,
                ));
            }
            dirty = true;
        }

        Self {
            cfg,
            registry,
            ndi: Some(ndi),
            decklink: Some(decklink),
            #[cfg(target_os = "macos")]
            syphon,
            comp: None,
            dirty,
            selected_layer_id: None,
            dragging_uuid: None,
        }
    }

    pub fn update(&mut self) {
        // Auto-connect pending NDI sources when they appear in discovery
        if let Some(ref ndi) = self.ndi {
            let discovered = ndi.list();
            for layer in &self.cfg.canvas.layers {
                if layer.protocol == Protocol::Ndi {
                    if let Some(ref name) = layer.source_id {
                        if self.registry.get(name).is_none() {
                            if let Some(src) = discovered.iter().find(|s| &s.name == name) {
                                self.registry.add_ndi(name.clone(), src.clone());
                                self.dirty = true;
                            }
                        }
                    }
                }
            }
        }

        // Auto-connect pending DeckLink sources when they appear in discovery
        if let Some(ref decklink) = self.decklink {
            let discovered = decklink.list();
            for layer in &self.cfg.canvas.layers {
                if layer.protocol == Protocol::Decklink {
                    if let Some(ref name) = layer.source_id {
                        if self.registry.get(name).is_none() {
                            if let Some(port) = discovered.iter().find(|p| &p.name == name) {
                                self.registry.add_decklink(name.clone(), name.clone(), Some(port.connections.clone()));
                                self.dirty = true;
                            }
                        }
                    }
                }
            }
        }

        // Auto-connect pending Syphon sources on macOS
        #[cfg(target_os = "macos")]
        {
            if let Some(ref syphon) = self.syphon {
                let discovered = syphon.list();
                for layer in &self.cfg.canvas.layers {
                    if layer.protocol == Protocol::Syphon {
                        if let Some(ref name) = layer.source_id {
                            if self.registry.get(name).is_none() {
                                if discovered.iter().any(|s| s == name) {
                                    self.registry.add_syphon(name.clone(), name.clone());
                                    self.dirty = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn ensure_compositor(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) {
        if self.comp.is_none() {
            self.comp = Some(Compositor::new(device, queue, target_format));
        }
    }

    pub fn build_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        panel_rect: &Rect,
    ) -> Draw {
        let comp = self.comp.as_mut().expect("compositor not initialized");
        comp.build(device, queue, &self.cfg.canvas, &self.registry, panel_rect)
    }

    pub fn shared(&self) -> Option<std::sync::Arc<crate::compositor::Shared>> {
        self.comp.as_ref().map(|c| c.shared.clone())
    }

    pub fn hit_test(&self, panel_rect: &Rect, pos: (f32, f32)) -> Option<String> {
        let canvas = &self.cfg.canvas;
        let (scale, offset_x, offset_y) = compositor::canvas_transform(canvas, panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;

        let mut layers: Vec<_> = canvas.layers.iter().collect();
        layers.sort_by_key(|l| -l.z);

        let (px, py) = pos;
        for layer in layers {
            let lx = cx + layer.x * scale;
            let ly = cy + layer.y * scale;
            let lw = layer.width as f32 * scale;
            let lh = layer.height as f32 * scale;
            if px >= lx && px <= lx + lw && py >= ly && py <= ly + lh {
                return Some(layer.uuid.clone());
            }
        }
        None
    }

    pub fn drag_layer(&mut self, uuid: &str, delta: (f32, f32), panel_rect: &Rect) {
        let (scale, _, _) = compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        if let Some(layer) = self.cfg.canvas.layers.iter_mut().find(|l| &l.uuid == uuid) {
            let (dx, dy) = delta;
            layer.x += dx / scale;
            layer.y += dy / scale;
            self.dirty = true;
        }
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            self.cfg.save();
            self.dirty = false;
        }
    }

    pub fn cleanup_orphaned_sources(&mut self) {
        let active_ids: Vec<&str> = self.cfg.canvas.layers.iter()
            .filter_map(|l| l.source_id.as_deref())
            .collect();
        self.registry.cleanup_orphaned_sources(&active_ids);
    }

    pub fn add_layer(&mut self) -> String {
        let sid = self.registry.add_test();
        let num = self.cfg.canvas.layers.len() + 1;
        let layer = Layer::new_v4(
            format!("Layer {num}"),
            Protocol::Test,
            Some(sid),
            self.cfg.canvas.width as f32 * 0.25,
            self.cfg.canvas.height as f32 * 0.25,
            self.cfg.canvas.width / 2,
            self.cfg.canvas.height / 2,
            self.cfg.canvas.layers.len() as i32,
            TextureMode::Fit,
            false,
            false,
        );
        let uuid = layer.uuid.clone();
        self.cfg.canvas.layers.push(layer);
        self.dirty = true;
        uuid
    }

    pub fn remove_layer(&mut self, uuid: &str) {
        self.cfg.canvas.layers.retain(|l| l.uuid != uuid);
        if self.selected_layer_id.as_deref() == Some(uuid) {
            self.selected_layer_id = None;
        }
        self.dirty = true;
    }

    pub fn connect_ndi(&mut self, name: &str) {
        if let Some(ref ndi) = self.ndi {
            if let Some(src) = ndi.find_by_name(name) {
                self.registry.add_ndi(name.to_string(), src);
            }
        }
    }

    pub fn connect_decklink(&mut self, name: &str) {
        let connections = self.decklink.as_ref()
            .and_then(|d| d.find_by_name(name))
            .map(|p| p.connections);
        self.registry.add_decklink(name.to_string(), name.to_string(), connections);
    }

    #[cfg(target_os = "macos")]
    pub fn connect_syphon(&mut self, name: &str) {
        self.registry.add_syphon(name.to_string(), name.to_string());
    }
}
