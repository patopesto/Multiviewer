use crate::compositor::{self, Rect, Vert};
use crate::engine::Engine;
use eframe::egui_wgpu;
use std::sync::Arc;

pub struct CanvasCallback {
    rect_px: [f32; 4], // x, y, w, h (physical pixels, origin top-left)
    shared: Arc<compositor::Shared>,
    verts: Arc<Vec<Vert>>,
    draws: Vec<(u32, Arc<wgpu::BindGroup>)>,
}

impl egui_wgpu::CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        queue.write_buffer(&self.shared.vb, 0, bytemuck::cast_slice(&self.verts));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        rpass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let [x, y, w, h] = self.rect_px;
        rpass.set_viewport(x, y, w, h, 0.0, 1.0);
        rpass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
        rpass.set_pipeline(&self.shared.pipeline);
        rpass.set_vertex_buffer(0, self.shared.vb.slice(..));
        rpass.set_index_buffer(self.shared.ib.slice(..), wgpu::IndexFormat::Uint16);
        for (first, bg) in &self.draws {
            rpass.set_bind_group(0, &**bg, &[]);
            rpass.draw_indexed(*first..*first + 6, 0, 0..1);
        }
    }
}

pub fn update(
    ui: &mut egui::Ui,
    engine: &mut Engine,
    ctx: &egui::Context,
    frame: &mut eframe::Frame,
) {
    let rect = ui.available_rect_before_wrap();
    let panel_rect = Rect { x: rect.min.x, y: rect.min.y, w: rect.width(), h: rect.height() };
    let (response, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());

    // Only start a drag if the press actually happened on the canvas.
    let pressed = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    let primary_down = ctx.input(|i| i.pointer.primary_down());
    if !primary_down {
        engine.dragging_uuid = None;
    } else if pressed && response.hovered() && let Some(pos) = response.interact_pointer_pos() {
        let hit = engine.hit_test(&panel_rect, (pos.x, pos.y));
        engine.selected_layer_id = hit.clone();
        engine.dragging_uuid = hit;
    }

    if response.dragged() {
        if let Some(uuid) = engine.dragging_uuid.clone() {
            engine.drag_layer(&uuid, (response.drag_delta().x, response.drag_delta().y), &panel_rect);
        } else {
            // Primary-drag on empty canvas pans the view.
            engine.view.pan += response.drag_delta();
        }
    }

    // Zoom toward the center of the view.
    if response.hovered() {
        let scroll = ctx.input(|i| i.smooth_scroll_delta).y;
        if scroll != 0.0 {
            let (base_scale, base_ox, base_oy) =
                compositor::canvas_transform(&engine.cfg.canvas, &panel_rect);
            let old_zoom = engine.view.zoom;
            let factor = 1.1_f32.powf(scroll / 50.0);
            let new_zoom = (old_zoom * factor).clamp(crate::engine::MIN_ZOOM, crate::engine::MAX_ZOOM);
            let center = egui::vec2(panel_rect.w / 2.0, panel_rect.h / 2.0);
            let world_c = (center
                - egui::vec2(base_ox + engine.view.pan.x, base_oy + engine.view.pan.y))
                / (base_scale * old_zoom);
            engine.view.zoom = new_zoom;
            engine.view.pan = egui::vec2(
                center.x - base_ox - world_c.x * base_scale * new_zoom,
                center.y - base_oy - world_c.y * base_scale * new_zoom,
            );
        }
    }

    let Some(rs) = frame.wgpu_render_state() else { return };
    engine.ensure_compositor(&rs.device, &rs.queue, rs.target_format);

    let transform = engine.display_transform(&panel_rect);
    let draw = engine.build_frame(&rs.device, &rs.queue, &panel_rect, transform);
    let ppp = ctx.pixels_per_point();

    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        CanvasCallback {
            rect_px: [
                rect.min.x * ppp,
                rect.min.y * ppp,
                rect.width() * ppp,
                rect.height() * ppp,
            ],
            shared: engine.shared().expect("compositor initialized"),
            verts: draw.verts,
            draws: draw.draws,
        },
    ));

    draw_overlays(&engine.cfg.canvas, &panel_rect, &painter, &engine.selected_layer_id, transform);

    let btn_rect = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - 100.0, rect.min.y + 10.0),
        egui::vec2(90.0, 24.0),
    );
    if ui.put(btn_rect, egui::Button::new("Recenter")).clicked() {
        engine.recenter_view(&panel_rect);
    }
}

fn draw_overlays(
    canvas: &crate::config::Canvas,
    panel_rect: &Rect,
    painter: &egui::Painter,
    selected: &Option<String>,
    transform: (f32, f32, f32),
) {
    let (scale, offset_x, offset_y) = transform;
    let cx = panel_rect.x + offset_x;
    let cy = panel_rect.y + offset_y;
    let cw = canvas.width as f32 * scale;
    let ch = canvas.height as f32 * scale;

    painter.rect_stroke(
        egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, ch)),
        0.0,
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgba_unmultiplied(60, 60, 60, 120)),
        egui::StrokeKind::Inside,
    );

    let selected_uuid = match selected {
        Some(uuid) => uuid,
        None => return,
    };

    let Some(layer) = canvas.layers.iter().find(|l| &l.uuid == selected_uuid) else { return };
    let lx = cx + layer.x * scale;
    let ly = cy + layer.y * scale;
    let lw = layer.width as f32 * scale;
    let lh = layer.height as f32 * scale;
    painter.rect_stroke(
        egui::Rect::from_min_size(egui::pos2(lx, ly), egui::vec2(lw, lh)),
        0.0,
        egui::Stroke::new(2.0_f32, egui::Color32::YELLOW),
        egui::StrokeKind::Inside,
    );
}
