use crate::compositor::{self, Compositor, Draw, Rect};
use crate::config::{Config, Layer, Protocol, TextureMode};
use crate::sources::Registry;
use crate::sources::ndi::Discovery as NdiDiscovery;
use crate::sources::decklink::Discovery as DecklinkDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::syphon::Discovery as SyphonDiscovery;

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct ViewState {
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl ViewState {
    pub fn new() -> Self {
        Self { zoom: 1.0, pan: egui::Vec2::ZERO }
    }
}

pub struct Engine {
    pub cfg: Config,
    pub registry: Registry,
    pub ndi: Option<NdiDiscovery>,
    pub decklink: Option<DecklinkDiscovery>,
    #[cfg(target_os = "macos")]
    pub syphon: Option<SyphonDiscovery>,
    comp: Option<Compositor>,
    pub dirty: bool,
    pub selected_layer_id: Option<String>,
    pub dragging_uuid: Option<String>,
    pub view: ViewState,
}

impl Engine {
    pub fn new() -> Self {
        let mut cfg = Config::load();
        let mut registry = Registry::new();
        let mut dirty = false;

        // Start NDI discovery before restoring sources
        let ndi = NdiDiscovery::start();

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
            view: ViewState::new(),
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
        transform: (f32, f32, f32),
    ) -> Draw {
        let comp = self.comp.as_mut().expect("compositor not initialized");
        comp.build(device, queue, &self.cfg.canvas, self.cfg.layer_borders, &self.registry, panel_rect, transform)
    }

    pub fn display_transform(&self, panel_rect: &Rect) -> (f32, f32, f32) {
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        (base_scale * self.view.zoom, base_ox + self.view.pan.x, base_oy + self.view.pan.y)
    }

    pub fn recenter_view(&mut self, panel_rect: &Rect) {
        let canvas = &self.cfg.canvas;
        let (mut min_x, mut min_y) = (0.0_f32, 0.0_f32);
        let (mut max_x, mut max_y) = (canvas.width as f32, canvas.height as f32);
        for layer in &canvas.layers {
            min_x = min_x.min(layer.x).min(layer.x + layer.width as f32);
            min_y = min_y.min(layer.y).min(layer.y + layer.height as f32);
            max_x = max_x.max(layer.x).max(layer.x + layer.width as f32);
            max_y = max_y.max(layer.y).max(layer.y + layer.height as f32);
        }
        let bbox_w = (max_x - min_x).max(1.0);
        let bbox_h = (max_y - min_y).max(1.0);
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(canvas, panel_rect);
        let target_scale = (panel_rect.width() / bbox_w).min(panel_rect.height() / bbox_h) * 0.9;
        self.view.zoom = (target_scale / base_scale).clamp(MIN_ZOOM, MAX_ZOOM);
        let display_scale = base_scale * self.view.zoom;
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.view.pan.x = panel_rect.width() / 2.0 - base_ox - cx * display_scale;
        self.view.pan.y = panel_rect.height() / 2.0 - base_oy - cy * display_scale;
    }

    pub fn shared(&self) -> Option<std::sync::Arc<crate::compositor::Shared>> {
        self.comp.as_ref().map(|c| c.shared.clone())
    }

    pub fn hit_test(&self, panel_rect: &Rect, pos: (f32, f32)) -> Option<String> {
        let canvas = &self.cfg.canvas;
        let (scale, offset_x, offset_y) = self.display_transform(panel_rect);
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
        let (scale, _, _) = self.display_transform(panel_rect);
        if let Some(layer) = self.cfg.canvas.layers.iter_mut().find(|l| &l.uuid == uuid) {
            let (dx, dy) = delta;
            layer.x += (dx / scale).round();
            layer.y += (dy / scale).round();
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
            format!("Source {num}"),
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

    pub fn move_layer(&mut self, from_index: usize, to_index: usize) {
        let len = self.cfg.canvas.layers.len();
        if from_index == to_index || from_index >= len || to_index >= len {
            return;
        }
        let layer = self.cfg.canvas.layers.remove(from_index);
        let insert_at = if to_index > from_index { to_index } else { to_index };
        self.cfg.canvas.layers.insert(insert_at, layer);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine(canvas: crate::config::Canvas) -> Engine {
        Engine {
            cfg: crate::config::Config { canvas, ..Default::default() },
            registry: Registry::new(),
            ndi: None,
            decklink: None,
            #[cfg(target_os = "macos")]
            syphon: None,
            comp: None,
            dirty: false,
            selected_layer_id: None,
            dragging_uuid: None,
            view: ViewState::new(),
        }
    }

    #[test]
    fn display_transform_is_base_at_default_zoom() {
        let engine = test_engine(crate::config::Canvas { width: 1920, height: 1080, layers: vec![] });
        let panel = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let (scale, ox, oy) = engine.display_transform(&panel);
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(&engine.cfg.canvas, &panel);
        assert!((scale - base_scale).abs() < 1e-3);
        assert!((ox - base_ox).abs() < 1e-3);
        assert!((oy - base_oy).abs() < 1e-3);
    }

    #[test]
    fn recenter_fits_canvas_with_margin() {
        let mut engine = test_engine(crate::config::Canvas { width: 1920, height: 1080, layers: vec![] });
        let panel = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        engine.recenter_view(&panel);
        let (scale, ox, oy) = engine.display_transform(&panel);
        let expected_scale = (panel.w / 1920.0).min(panel.h / 1080.0) * 0.9;
        assert!((scale - expected_scale).abs() < 1e-3);
        assert!((ox - (panel.w - 1920.0 * scale) / 2.0).abs() < 1e-3);
        assert!((oy - (panel.h - 1080.0 * scale) / 2.0).abs() < 1e-3);
    }

    #[test]
    fn recenter_expands_to_include_layers() {
        let mut canvas = crate::config::Canvas { width: 100, height: 100, layers: vec![] };
        canvas.layers.push(Layer::new_v4(
            "L1".into(),
            Protocol::Test,
            None,
            -50.0,
            -50.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        canvas.layers.push(Layer::new_v4(
            "L2".into(),
            Protocol::Test,
            None,
            200.0,
            200.0,
            100,
            100,
            1,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect { x: 0.0, y: 0.0, w: 400.0, h: 400.0 };
        engine.recenter_view(&panel);
        let (scale, _ox, _oy) = engine.display_transform(&panel);
        let bbox_w = 350.0;
        let bbox_h = 350.0;
        let expected_scale = (panel.w / bbox_w).min(panel.h / bbox_h) * 0.9;
        assert!((scale - expected_scale).abs() < 1e-3);
    }
}
