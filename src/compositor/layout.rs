use super::Rect;
use crate::config::Canvas;

/// Compute the scale and offset to letterbox the canvas inside the panel.
pub fn canvas_transform(canvas: &Canvas, panel_rect: &Rect) -> (f32, f32, f32) {
    let canvas_aspect = canvas.width as f32 / canvas.height.max(1) as f32;
    let panel_aspect = panel_rect.width() / panel_rect.height().max(0.001);
    if canvas_aspect > panel_aspect {
        let scale = panel_rect.width() / canvas.width.max(1) as f32;
        let h = canvas.height as f32 * scale;
        (scale, 0.0, (panel_rect.height() - h) / 2.0)
    } else {
        let scale = panel_rect.height() / canvas.height.max(1) as f32;
        let w = canvas.width as f32 * scale;
        (scale, (panel_rect.width() - w) / 2.0, 0.0)
    }
}

