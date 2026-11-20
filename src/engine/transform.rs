use serde::{Deserialize, Serialize};

use crate::compositor::{self, Rect};
use crate::config::Canvas;

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 20.0;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ViewState {
    pub zoom: f32,
    pub pan: [f32; 2],
}

impl ViewState {
    pub fn new() -> Self {
        return Self { zoom: 1.0, pan: [0.0, 0.0] };
    }

    pub fn recenter(&mut self, canvas: &Canvas, panel_rect: &Rect) {
        let (mut min_x, mut min_y) = (0.0_f32, 0.0_f32);
        let (mut max_x, mut max_y) = (canvas.width as f32, canvas.height as f32);
        for source in &canvas.sources {
            min_x = min_x.min(source.x).min(source.x + source.width as f32);
            min_y = min_y.min(source.y).min(source.y + source.height as f32);
            max_x = max_x.max(source.x).max(source.x + source.width as f32);
            max_y = max_y.max(source.y).max(source.y + source.height as f32);
        }
        let bbox_w = (max_x - min_x).max(1.0);
        let bbox_h = (max_y - min_y).max(1.0);
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(canvas, panel_rect);
        if !base_scale.is_finite() || base_scale <= 0.0 {
            return;
        }
        let target_scale = (panel_rect.width() / bbox_w).min(panel_rect.height() / bbox_h) * 0.9;
        self.zoom = (target_scale / base_scale).clamp(MIN_ZOOM, MAX_ZOOM);
        let display_scale = base_scale * self.zoom;
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.pan = [
            panel_rect.width() / 2.0 - base_ox - cx * display_scale,
            panel_rect.height() / 2.0 - base_oy - cy * display_scale,
        ];
    }

    pub fn zoom(&mut self, canvas: &Canvas, panel_rect: &Rect, factor: f32) {
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(canvas, panel_rect);
        // A viewport that has not been laid out yet (or a zero-size panel)
        // would divide by zero and poison the view with NaN.
        if !base_scale.is_finite() || base_scale <= 0.0 {
            return;
        }
        let old_zoom = self.zoom;
        let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let center = egui::vec2(panel_rect.w / 2.0, panel_rect.h / 2.0);
        let world_c = (center - egui::vec2(base_ox + self.pan[0], base_oy + self.pan[1])) / (base_scale * old_zoom);
        self.zoom = new_zoom;
        self.pan = [
            center.x - base_ox - world_c.x * base_scale * new_zoom,
            center.y - base_oy - world_c.y * base_scale * new_zoom,
        ];
    }
}

pub fn display_transform(view: &ViewState, canvas: &Canvas, panel_rect: &Rect, ui_visible: bool) -> (f32, f32, f32) {
    let (base_scale, base_ox, base_oy) = compositor::canvas_transform(canvas, panel_rect);
    if !ui_visible {
        return (base_scale, base_ox, base_oy);
    }
    return (base_scale * view.zoom, base_ox + view.pan[0], base_oy + view.pan[1]);
}
