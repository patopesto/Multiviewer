use crate::compositor::{self, Rect, Vert};
use crate::engine::{DragState, Engine, ResizeHandle};
use eframe::egui_wgpu;
use std::sync::Arc;

// Macro to load from the assets directory
macro_rules! asset_image {
    ($file:expr) => {
        egui::include_image!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/", $file))
    };
}

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
        engine.drag_state = DragState::None;
    } else if pressed && response.hovered() && let Some(pos) = response.interact_pointer_pos() {
        if let Some((uuid, handle)) = engine.hit_test_resize_handle(&panel_rect, (pos.x, pos.y)) {
            let start = engine.layer_rect_world(&uuid).expect("selected layer exists");
            engine.selected_layer_id = Some(uuid.clone());
            engine.drag_state = DragState::Resize {
                uuid,
                handle,
                start,
                start_screen: (pos.x, pos.y),
            };
        } else {
            let hit = engine.hit_test(&panel_rect, (pos.x, pos.y));
            engine.selected_layer_id = hit.clone();
            engine.drag_state = hit.map(|uuid| DragState::Move { uuid }).unwrap_or_default();
        }
    }

    if response.dragged() {
        match engine.drag_state.clone() {
            DragState::Move { uuid } => {
                engine.drag_layer(&uuid, (response.drag_delta().x, response.drag_delta().y), &panel_rect);
            }
            DragState::Resize { uuid, handle, start, start_screen } => {
                if let Some(pos) = response.interact_pointer_pos() {
                    let delta = (pos.x - start_screen.0, pos.y - start_screen.1);
                    engine.resize_layer(&uuid, handle, start, delta, &panel_rect);
                }
            }
            DragState::None => {
                // Primary-drag on empty canvas pans the view.
                engine.view.pan += response.drag_delta();
            }
        }
    }

    // Cursor feedback when hovering a resize handle.
    if response.hovered() && !primary_down {
        if let Some(pos) = response.hover_pos() {
            if let Some((_, handle)) = engine.hit_test_resize_handle(&panel_rect, (pos.x, pos.y)) {
                ui.output_mut(|o| o.cursor_icon = cursor_for_handle(handle));
            }
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

    let btn_icon = asset_image!("compress.svg");
    let btn_image = egui::Image::new(btn_icon).fit_to_exact_size(egui::vec2(25.0, 25.0));
    let btn_rect = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - 40.0, rect.min.y + 10.0),
        egui::vec2(30.0, 30.0),
    );
    let btn = egui::Button::image(btn_image).corner_radius(5.0);
    if ui.put(btn_rect, btn).on_hover_text("Re-center view").clicked() {
        engine.recenter_view(&panel_rect);
    }
}

fn cursor_for_handle(handle: ResizeHandle) -> egui::CursorIcon {
    match handle {
        ResizeHandle::Top | ResizeHandle::Bottom => egui::CursorIcon::ResizeVertical,
        ResizeHandle::Left | ResizeHandle::Right => egui::CursorIcon::ResizeHorizontal,
        ResizeHandle::TopLeft | ResizeHandle::BottomRight => egui::CursorIcon::ResizeNwSe,
        ResizeHandle::TopRight | ResizeHandle::BottomLeft => egui::CursorIcon::ResizeNeSw,
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

    // Draw resize handles: corners and edge midpoints.
    // const HANDLE_SIZE: f32 = 6.0;
    // let hs = egui::vec2(HANDLE_SIZE, HANDLE_SIZE);
    // let corners = [
    //     egui::pos2(lx, ly),
    //     egui::pos2(lx + lw, ly),
    //     egui::pos2(lx + lw, ly + lh),
    //     egui::pos2(lx, ly + lh),
    // ];
    // for p in corners {
    //     painter.rect_filled(
    //         egui::Rect::from_center_size(p, hs),
    //         0.0,
    //         egui::Color32::YELLOW,
    //     );
    // }
    // let edges = [
    //     egui::pos2(lx + lw / 2.0, ly),
    //     egui::pos2(lx + lw, ly + lh / 2.0),
    //     egui::pos2(lx + lw / 2.0, ly + lh),
    //     egui::pos2(lx, ly + lh / 2.0),
    // ];
    // for p in edges {
    //     painter.rect_filled(
    //         egui::Rect::from_center_size(p, hs),
    //         0.0,
    //         egui::Color32::YELLOW,
    //     );
    // }
}
