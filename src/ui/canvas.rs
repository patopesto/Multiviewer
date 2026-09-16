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
    ui_visible: bool,
) {
    let rect = ui.available_rect_before_wrap();
    let panel_rect = Rect {
        x: rect.min.x,
        y: rect.min.y,
        w: rect.width(),
        h: rect.height(),
    };
    let (response, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());

    // Only start a drag if the press actually happened on the canvas.
    let pressed = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    let primary_down = ctx.input(|i| i.pointer.primary_down());
    if !primary_down {
        engine.drag_state = DragState::None;
        engine.snap_guides = crate::engine::SnapGuides::default();
    } else if ui_visible
        && pressed
        && response.hovered()
        && let Some(pos) = response.interact_pointer_pos()
    {
        response.request_focus();
        if engine.expanded_source_id().is_some() {
            // While a source is expanded, the canvas is used for panning only.
            engine.drag_state = DragState::None;
        } else if let Some((uuid, handle)) = engine.hit_test_resize_handle(&panel_rect, (pos.x, pos.y)) {
            let start = engine
                .layer_rect_world(&uuid)
                .expect("selected source exists");
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

    if ui_visible && response.dragged() {
        match engine.drag_state.clone() {
            DragState::Move { uuid } => {
                engine.drag_layer(
                    &uuid,
                    (response.drag_delta().x, response.drag_delta().y),
                    &panel_rect,
                );
            }
            DragState::Resize {
                uuid,
                handle,
                start,
                start_screen,
            } => {
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
    if ui_visible
        && response.hovered()
        && !primary_down
        && engine.expanded_source_id().is_none()
        && let Some(pos) = response.hover_pos()
        && let Some((_, handle)) = engine.hit_test_resize_handle(&panel_rect, (pos.x, pos.y))
    {
        ui.output_mut(|o| o.cursor_icon = cursor_for_handle(handle));
    }

    // Zoom toward the center of the view.
    if ui_visible && response.hovered() {
        let scroll = ctx.input(|i| i.smooth_scroll_delta).y;
        if scroll != 0.0 {
            let factor = 1.1_f32.powf(scroll / 50.0);
            engine.zoom_view(&panel_rect, factor);
        }
    }

    let Some(rs) = frame.wgpu_render_state() else {
        return;
    };
    engine.ensure_compositor(&rs.device, &rs.queue, rs.target_format);

    let transform = if ui_visible {
        engine.display_transform(&panel_rect)
    } else {
        engine.default_transform(&panel_rect)
    };
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

    if ui_visible {
        draw_overlays(&engine.cfg.canvas, &panel_rect, &painter, engine, transform);
    }

    // Context menu on sources
    if ui_visible {
        response.context_menu(|ui| {
            ui.set_min_width(100.0);
            if engine.expanded_source_id().is_some() {
                if ui.button("Exit expanded view").clicked() {
                    engine.clear_expanded_source();
                    ui.close();
                }
            } else if let Some(pos) = ui.input(|i| i.pointer.latest_pos())
                && let Some(uuid) = engine.hit_test(&panel_rect, (pos.x, pos.y))
            {
                engine.selected_layer_id = Some(uuid.clone());
                if ui.button("Expand to full canvas").clicked() {
                    engine.expand_source(uuid);
                    ui.close();
                }
            }
        });
    }

    // Buttons
    if ui_visible {
        if engine.expanded_source_id().is_some() {
            let close_icon = asset_image!("close.svg");
            let close_image = egui::Image::new(close_icon).fit_to_exact_size(egui::vec2(25.0, 25.0));
            let close_rect = egui::Rect::from_min_size(
                egui::pos2(rect.max.x - 80.0, rect.min.y + 10.0),
                egui::vec2(30.0, 30.0),
            );
            let close_btn = egui::Button::image(close_image).corner_radius(5.0);
            if ui.put(close_rect, close_btn).on_hover_text("Exit expanded view").clicked() {
                engine.clear_expanded_source();
            }
        }

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
    engine: &Engine,
    transform: (f32, f32, f32),
) {
    let (scale, offset_x, offset_y) = transform;
    let cx = panel_rect.x + offset_x;
    let cy = panel_rect.y + offset_y;
    let cw = canvas.width as f32 * scale;
    let ch = canvas.height as f32 * scale;

    // Canvas borders
    painter.rect_stroke(
        egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, ch)),
        0.0,
        egui::Stroke::new(
            2.0_f32,
            egui::Color32::from_rgba_unmultiplied(60, 60, 60, 120),
        ),
        egui::StrokeKind::Inside,
    );

    // Hide selection overlays while a source is expanded.
    if engine.expanded_source_id().is_some() {
        return;
    }

    // Selected source borders
    let selected_uuid = match &engine.selected_layer_id {
        Some(uuid) => uuid,
        None => return,
    };

    let Some(source) = canvas.sources.iter().find(|l| &l.uuid == selected_uuid) else {
        return;
    };
    let lx = cx + source.x * scale;
    let ly = cy + source.y * scale;
    let lw = source.width as f32 * scale;
    let lh = source.height as f32 * scale;
    painter.rect_stroke(
        egui::Rect::from_min_size(egui::pos2(lx, ly), egui::vec2(lw, lh)),
        0.0,
        egui::Stroke::new(canvas.border_width, egui::Color32::YELLOW),
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

    // Snap guides.
    let guide_stroke = egui::Stroke::new(1.0, egui::Color32::CYAN);
    if let Some(gx) = engine.snap_guides.x {
        let sx = cx + gx * scale;
        painter.vline(sx, panel_rect.y..=panel_rect.y + panel_rect.h, guide_stroke);
    }
    if let Some(gy) = engine.snap_guides.y {
        let sy = cy + gy * scale;
        painter.hline(panel_rect.x..=panel_rect.x + panel_rect.w, sy, guide_stroke);
    }
}
