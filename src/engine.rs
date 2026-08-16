use crate::compositor::{self, Compositor, Draw, Rect};
use crate::config::{Config, Layer, Protocol, TextureMode};
use crate::sources::Registry;
use crate::sources::decklink::Discovery as DecklinkDiscovery;
use crate::sources::ndi::Discovery as NdiDiscovery;
#[cfg(target_os = "macos")]
use crate::sources::syphon::Discovery as SyphonDiscovery;

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 10.0;
pub const SNAP_THRESHOLD: f32 = 2.0;
pub const SNAP_BREAK_THRESHOLD: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct ViewState {
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, Default)]
pub enum DragState {
    #[default]
    None,
    Move {
        uuid: String,
    },
    Resize {
        uuid: String,
        handle: ResizeHandle,
        start: WorldRect,
        start_screen: (f32, f32),
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapGuides {
    pub x: Option<f32>,
    pub y: Option<f32>,
}

pub struct SnapCandidates {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
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
    pub drag_state: DragState,
    pub snap_guides: SnapGuides,
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
            drag_state: DragState::None,
            snap_guides: SnapGuides::default(),
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
                                self.registry.add_decklink(
                                    name.clone(),
                                    name.clone(),
                                    Some(port.connections),
                                );
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

        self.registry.apply_pending_restarts();
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
        comp.build(
            device,
            queue,
            &self.cfg.canvas,
            self.cfg.layer_borders,
            &self.registry,
            panel_rect,
            transform,
        )
    }

    pub fn display_transform(&self, panel_rect: &Rect) -> (f32, f32, f32) {
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        (
            base_scale * self.view.zoom,
            base_ox + self.view.pan.x,
            base_oy + self.view.pan.y,
        )
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
        let snap_threshold = SNAP_THRESHOLD / scale;
        let break_threshold = SNAP_BREAK_THRESHOLD / scale;
        let candidates = self.snap_candidates(uuid);

        if let Some(layer) = self.cfg.canvas.layers.iter_mut().find(|l| &l.uuid == uuid) {
            let (dx, dy) = delta;
            let proposed_x = layer.x + dx / scale;
            let proposed_y = layer.y + dy / scale;

            let left = proposed_x;
            let right = proposed_x + layer.width as f32;
            let top = proposed_y;
            let bottom = proposed_y + layer.height as f32;

            let current_x = self.snap_guides.x;
            let current_y = self.snap_guides.y;
            let snap_left = Self::snap_value(
                left,
                &candidates.x,
                snap_threshold,
                break_threshold,
                current_x,
            );
            let snap_right = Self::snap_value(
                right,
                &candidates.x,
                snap_threshold,
                break_threshold,
                current_x,
            );
            let snap_top = Self::snap_value(
                top,
                &candidates.y,
                snap_threshold,
                break_threshold,
                current_y,
            );
            let snap_bottom = Self::snap_value(
                bottom,
                &candidates.y,
                snap_threshold,
                break_threshold,
                current_y,
            );

            let (x_offset, guide_x) = match (snap_left, snap_right) {
                (Some(l), Some(r)) => {
                    if (l - left).abs() < (r - right).abs() {
                        (l - left, l)
                    } else {
                        (r - right, r)
                    }
                }
                (Some(l), None) => (l - left, l),
                (None, Some(r)) => (r - right, r),
                (None, None) => (0.0, 0.0),
            };
            let (y_offset, guide_y) = match (snap_top, snap_bottom) {
                (Some(t), Some(b)) => {
                    if (t - top).abs() < (b - bottom).abs() {
                        (t - top, t)
                    } else {
                        (b - bottom, b)
                    }
                }
                (Some(t), None) => (t - top, t),
                (None, Some(b)) => (b - bottom, b),
                (None, None) => (0.0, 0.0),
            };

            layer.x = (proposed_x + x_offset).round();
            layer.y = (proposed_y + y_offset).round();
            self.snap_guides.x = if x_offset != 0.0 { Some(guide_x) } else { None };
            self.snap_guides.y = if y_offset != 0.0 { Some(guide_y) } else { None };
            self.dirty = true;
        }
    }

    pub fn layer_rect_world(&self, uuid: &str) -> Option<WorldRect> {
        self.cfg
            .canvas
            .layers
            .iter()
            .find(|l| &l.uuid == uuid)
            .map(|l| WorldRect {
                x: l.x,
                y: l.y,
                w: l.width as f32,
                h: l.height as f32,
            })
    }

    pub fn snap_candidates(&self, exclude_uuid: &str) -> SnapCandidates {
        let canvas = &self.cfg.canvas;
        let mut x = vec![0.0, canvas.width as f32];
        let mut y = vec![0.0, canvas.height as f32];
        for layer in &canvas.layers {
            if layer.uuid == exclude_uuid {
                continue;
            }
            x.push(layer.x);
            x.push(layer.x + layer.width as f32);
            y.push(layer.y);
            y.push(layer.y + layer.height as f32);
        }
        SnapCandidates { x, y }
    }

    fn snap_value(
        value: f32,
        candidates: &[f32],
        snap_threshold: f32,
        break_threshold: f32,
        current: Option<f32>,
    ) -> Option<f32> {
        let mut best = None;
        let mut best_dist = f32::INFINITY;
        for &c in candidates {
            let dist = (c - value).abs();
            if dist < best_dist {
                best_dist = dist;
                best = Some(c);
            }
        }
        if best_dist < snap_threshold {
            return best;
        }
        // Hysteresis: stay snapped to the current guide until the mouse moves
        // past the larger break threshold.
        if let Some(curr) = current {
            let dist = (curr - value).abs();
            if dist < break_threshold {
                return Some(curr);
            }
        }
        None
    }

    pub fn hit_test_resize_handle(
        &self,
        panel_rect: &Rect,
        pos: (f32, f32),
    ) -> Option<(String, ResizeHandle)> {
        let uuid = self.selected_layer_id.as_ref()?;
        let layer = self.cfg.canvas.layers.iter().find(|l| &l.uuid == uuid)?;
        let (scale, offset_x, offset_y) = self.display_transform(panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;
        let lx = cx + layer.x * scale;
        let ly = cy + layer.y * scale;
        let lw = layer.width as f32 * scale;
        let lh = layer.height as f32 * scale;
        let right = lx + lw;
        let bottom = ly + lh;
        let (px, py) = pos;
        const H: f32 = 8.0; // hit radius in screen points

        // Corners take priority over edges.
        if (px - lx).abs() <= H && (py - ly).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::TopLeft));
        }
        if (px - right).abs() <= H && (py - ly).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::TopRight));
        }
        if (px - lx).abs() <= H && (py - bottom).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::BottomLeft));
        }
        if (px - right).abs() <= H && (py - bottom).abs() <= H {
            return Some((uuid.clone(), ResizeHandle::BottomRight));
        }

        // Edges.
        if (py - ly).abs() <= H && px >= lx && px <= right {
            return Some((uuid.clone(), ResizeHandle::Top));
        }
        if (py - bottom).abs() <= H && px >= lx && px <= right {
            return Some((uuid.clone(), ResizeHandle::Bottom));
        }
        if (px - lx).abs() <= H && py >= ly && py <= bottom {
            return Some((uuid.clone(), ResizeHandle::Left));
        }
        if (px - right).abs() <= H && py >= ly && py <= bottom {
            return Some((uuid.clone(), ResizeHandle::Right));
        }

        None
    }

    pub fn resize_layer(
        &mut self,
        uuid: &str,
        handle: ResizeHandle,
        start: WorldRect,
        delta_screen: (f32, f32),
        panel_rect: &Rect,
    ) {
        let (scale, _, _) = self.display_transform(panel_rect);
        let dx = delta_screen.0 / scale;
        let dy = delta_screen.1 / scale;
        let snap_threshold = SNAP_THRESHOLD / scale;
        let break_threshold = SNAP_BREAK_THRESHOLD / scale;
        let candidates = self.snap_candidates(uuid);

        if let Some(layer) = self.cfg.canvas.layers.iter_mut().find(|l| &l.uuid == uuid) {
            let mut x = start.x;
            let mut y = start.y;
            let mut w = start.w;
            let mut h = start.h;
            let mut guide_x = None;
            let mut guide_y = None;

            match handle {
                ResizeHandle::Left => {
                    let proposed = x + dx;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.x,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.x,
                    ) {
                        guide_x = Some(snap);
                        x = snap.min(start.x + start.w - 1.0);
                        w = (start.x + start.w) - x;
                    } else {
                        let new_x = x + dx;
                        let new_w = (x + w) - new_x;
                        if new_w >= 1.0 {
                            x = new_x;
                            w = new_w;
                        } else {
                            x = x + w - 1.0;
                            w = 1.0;
                        }
                    }
                }
                ResizeHandle::Right => {
                    let proposed = x + w + dx;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.x,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.x,
                    ) {
                        guide_x = Some(snap);
                        w = (snap - x).max(1.0);
                    } else {
                        w = (w + dx).max(1.0);
                    }
                }
                ResizeHandle::Top => {
                    let proposed = y + dy;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.y,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.y,
                    ) {
                        guide_y = Some(snap);
                        y = snap.min(start.y + start.h - 1.0);
                        h = (start.y + start.h) - y;
                    } else {
                        let new_y = y + dy;
                        let new_h = (y + h) - new_y;
                        if new_h >= 1.0 {
                            y = new_y;
                            h = new_h;
                        } else {
                            y = y + h - 1.0;
                            h = 1.0;
                        }
                    }
                }
                ResizeHandle::Bottom => {
                    let proposed = y + h + dy;
                    if let Some(snap) = Self::snap_value(
                        proposed,
                        &candidates.y,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.y,
                    ) {
                        guide_y = Some(snap);
                        h = (snap - y).max(1.0);
                    } else {
                        h = (h + dy).max(1.0);
                    }
                }
                _ => {
                    // Corner drag: preserve aspect ratio by projecting the moving
                    // corner onto the diagonal from the fixed opposite corner, then
                    // round width and derive height from the original aspect so the
                    // integer dimensions stay proportional.
                    let (fx, fy) = match handle {
                        ResizeHandle::TopLeft => (x + w, y + h),
                        ResizeHandle::TopRight => (x, y + h),
                        ResizeHandle::BottomRight => (x, y),
                        ResizeHandle::BottomLeft => (x + w, y),
                        _ => unreachable!(),
                    };
                    let mx0 = start.x + start.w - (fx - start.x); // start moving corner x
                    let my0 = start.y + start.h - (fy - start.y); // start moving corner y
                    let diag_x = mx0 - fx;
                    let diag_y = my0 - fy;
                    let denom = diag_x * diag_x + diag_y * diag_y;
                    if denom > 0.0 {
                        let t = ((mx0 + dx - fx) * diag_x + (my0 + dy - fy) * diag_y) / denom;
                        let min_t = (1.0 / start.w).max(1.0 / start.h);
                        let t = t.max(min_t);
                        let mut mx = fx + t * diag_x;
                        let mut my = fy + t * diag_y;

                        // Snap the moving corner to candidates, preferring the closer axis.
                        let snap_mx = Self::snap_value(
                            mx,
                            &candidates.x,
                            snap_threshold,
                            break_threshold,
                            self.snap_guides.x,
                        );
                        let snap_my = Self::snap_value(
                            my,
                            &candidates.y,
                            snap_threshold,
                            break_threshold,
                            self.snap_guides.y,
                        );
                        let dist_x = snap_mx.map(|v| (v - mx).abs());
                        let dist_y = snap_my.map(|v| (v - my).abs());
                        match (dist_x, dist_y) {
                            (Some(dx_), Some(dy_)) => {
                                if dx_ < dy_ {
                                    mx = snap_mx.unwrap();
                                    guide_x = Some(mx);
                                } else {
                                    my = snap_my.unwrap();
                                    mx = fx + (my - fy) * diag_x / diag_y;
                                    guide_y = Some(my);
                                }
                            }
                            (Some(_), None) => {
                                mx = snap_mx.unwrap();
                                guide_x = Some(mx);
                            }
                            (None, Some(_)) => {
                                my = snap_my.unwrap();
                                mx = fx + (my - fy) * diag_x / diag_y;
                                guide_y = Some(my);
                            }
                            (None, None) => {}
                        }

                        let new_w = (mx - fx).abs().round().max(1.0);
                        let new_h = (new_w * start.h / start.w).round().max(1.0);
                        x = match handle {
                            ResizeHandle::TopLeft | ResizeHandle::BottomLeft => fx - new_w,
                            _ => fx,
                        };
                        y = match handle {
                            ResizeHandle::TopLeft | ResizeHandle::TopRight => fy - new_h,
                            _ => fy,
                        };
                        w = new_w;
                        h = new_h;
                    }
                }
            }

            layer.x = x.round();
            layer.y = y.round();
            layer.width = w.max(1.0).round() as u32;
            layer.height = h.max(1.0).round() as u32;
            self.snap_guides = SnapGuides {
                x: guide_x,
                y: guide_y,
            };
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
        let active_ids: Vec<&str> = self
            .cfg
            .canvas
            .layers
            .iter()
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
        let insert_at = if to_index > from_index {
            to_index
        } else {
            to_index
        };
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
        let connections = self
            .decklink
            .as_ref()
            .and_then(|d| d.find_by_name(name))
            .map(|p| p.connections);
        self.registry
            .add_decklink(name.to_string(), name.to_string(), connections);
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
            cfg: crate::config::Config {
                canvas,
                ..Default::default()
            },
            registry: Registry::new(),
            ndi: None,
            decklink: None,
            #[cfg(target_os = "macos")]
            syphon: None,
            comp: None,
            dirty: false,
            selected_layer_id: None,
            drag_state: DragState::None,
            snap_guides: SnapGuides::default(),
            view: ViewState::new(),
        }
    }

    #[test]
    fn display_transform_is_base_at_default_zoom() {
        let engine = test_engine(crate::config::Canvas {
            width: 1920,
            height: 1080,
            layers: vec![],
        });
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        let (scale, ox, oy) = engine.display_transform(&panel);
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&engine.cfg.canvas, &panel);
        assert!((scale - base_scale).abs() < 1e-3);
        assert!((ox - base_ox).abs() < 1e-3);
        assert!((oy - base_oy).abs() < 1e-3);
    }

    #[test]
    fn recenter_fits_canvas_with_margin() {
        let mut engine = test_engine(crate::config::Canvas {
            width: 1920,
            height: 1080,
            layers: vec![],
        });
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        engine.recenter_view(&panel);
        let (scale, ox, oy) = engine.display_transform(&panel);
        let expected_scale = (panel.w / 1920.0).min(panel.h / 1080.0) * 0.9;
        assert!((scale - expected_scale).abs() < 1e-3);
        assert!((ox - (panel.w - 1920.0 * scale) / 2.0).abs() < 1e-3);
        assert!((oy - (panel.h - 1080.0 * scale) / 2.0).abs() < 1e-3);
    }

    #[test]
    fn recenter_expands_to_include_layers() {
        let mut canvas = crate::config::Canvas {
            width: 100,
            height: 100,
            layers: vec![],
        };
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
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 400.0,
        };
        engine.recenter_view(&panel);
        let (scale, _ox, _oy) = engine.display_transform(&panel);
        let bbox_w = 350.0;
        let bbox_h = 350.0;
        let expected_scale = (panel.w / bbox_w).min(panel.h / bbox_h) * 0.9;
        assert!((scale - expected_scale).abs() < 1e-3);
    }

    #[test]
    fn resize_layer_handles_corners_and_edges() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            layers: vec![],
        };
        canvas.layers.push(Layer::new_v4(
            "L1".into(),
            Protocol::Test,
            None,
            100.0,
            100.0,
            200,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.layers[0].uuid.clone();
        let start = engine.layer_rect_world(&uuid).unwrap();

        // Edge resize: drag right edge 30 px to the right.
        engine.resize_layer(&uuid, ResizeHandle::Right, start, (30.0, 0.0), &panel);
        let layer = &engine.cfg.canvas.layers[0];
        assert_eq!(layer.x, 100.0);
        assert_eq!(layer.y, 100.0);
        assert_eq!(layer.width, 230);
        assert_eq!(layer.height, 100);

        // Edge resize: drag top edge 20 px up.
        let start = engine.layer_rect_world(&uuid).unwrap();
        engine.resize_layer(&uuid, ResizeHandle::Top, start, (0.0, -20.0), &panel);
        let layer = &engine.cfg.canvas.layers[0];
        assert_eq!(layer.x, 100.0);
        assert_eq!(layer.y, 80.0);
        assert_eq!(layer.width, 230);
        assert_eq!(layer.height, 120);

        // Corner resize: drag bottom-right along the diagonal.
        let start = engine.layer_rect_world(&uuid).unwrap();
        engine.resize_layer(
            &uuid,
            ResizeHandle::BottomRight,
            start,
            (50.0, 50.0 * 120.0 / 230.0),
            &panel,
        );
        let layer = &engine.cfg.canvas.layers[0];
        let aspect = layer.width as f32 / layer.height as f32;
        // Integer dimensions can't match the exact float aspect; allow ~1% rounding error.
        assert!(
            (aspect - 230.0 / 120.0).abs() < 0.02,
            "aspect should be preserved, got {aspect}"
        );
        assert_eq!(layer.x, 100.0);
        assert_eq!(layer.y, 80.0);
    }

    #[test]
    fn drag_layer_snaps_to_canvas_edge() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            layers: vec![],
        };
        canvas.layers.push(Layer::new_v4(
            "L1".into(),
            Protocol::Test,
            None,
            15.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.layers[0].uuid.clone();
        // Drag left by 14 px: left edge moves from 15 to 1, within the 2 px snap threshold of 0.
        engine.drag_layer(&uuid, (-14.0, 0.0), &panel);
        let layer = &engine.cfg.canvas.layers[0];
        assert_eq!(layer.x, 0.0);
        assert!(engine.snap_guides.x.is_some());
    }

    #[test]
    fn resize_layer_snaps_to_other_layer_edge() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            layers: vec![],
        };
        canvas.layers.push(Layer::new_v4(
            "L1".into(),
            Protocol::Test,
            None,
            100.0,
            100.0,
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
            300.0,
            100.0,
            100,
            100,
            1,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.layers[0].uuid.clone();
        let start = engine.layer_rect_world(&uuid).unwrap();
        // Drag L1's right edge to 299: should snap to L2's left edge at 300.
        engine.resize_layer(&uuid, ResizeHandle::Right, start, (99.0, 0.0), &panel);
        let layer = &engine.cfg.canvas.layers[0];
        assert_eq!(layer.width, 200);
        assert!(engine.snap_guides.x.is_some());
    }

    #[test]
    fn drag_layer_hysteresis_releases_after_break_threshold() {
        let mut canvas = crate::config::Canvas {
            width: 1920,
            height: 1080,
            layers: vec![],
        };
        canvas.layers.push(Layer::new_v4(
            "L1".into(),
            Protocol::Test,
            None,
            15.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
        let mut engine = test_engine(canvas);
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let uuid = engine.cfg.canvas.layers[0].uuid.clone();

        // Snap left edge to the canvas edge at 0.
        engine.drag_layer(&uuid, (-14.0, 0.0), &panel);
        assert_eq!(engine.cfg.canvas.layers[0].x, 0.0);
        assert!(engine.snap_guides.x.is_some());

        // Move 1 px back: stays snapped within the 20 px break threshold.
        engine.drag_layer(&uuid, (1.0, 0.0), &panel);
        assert_eq!(engine.cfg.canvas.layers[0].x, 0.0);
        assert!(engine.snap_guides.x.is_some());

        // Move 25 px past the snap point: breaks free.
        engine.drag_layer(&uuid, (25.0, 0.0), &panel);
        assert_eq!(engine.cfg.canvas.layers[0].x, 25.0);
        assert!(engine.snap_guides.x.is_none());
    }
}
