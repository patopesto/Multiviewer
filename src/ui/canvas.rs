use std::sync::Arc;
use std::sync::mpsc::Sender;

use eframe::egui_wgpu;

use crate::compositor::{self, Draw, Rect};
use crate::config::{Canvas, Source, SourceBorderVisibility, SourceId, SourceLabelVisibility, TextureMode};
use crate::engine::{Command, EngineSnapshot, PreviewFrame, Selector};
use crate::sources::{SourceKey, SourceStats};

macro_rules! asset_image {
    ($file:expr) => {
        egui::include_image!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/", $file))
    };
}

const HANDLE_TOLERANCE: f32 = 8.0;
const SNAP_THRESHOLD: f32 = 2.0;
const SNAP_BREAK_THRESHOLD: f32 = 2.0;
const STATS_LINE_HEIGHT: f32 = 14.0;
const STATS_INSET: f32 = 6.0;
const STATS_MIN_WIDTH: f32 = 100.0;
const STATS_MIN_HEIGHT: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Clone, Copy, Debug, Default)]
struct SnapGuides {
    x: Option<f32>,
    y: Option<f32>,
}

#[derive(Clone, Copy)]
struct WorldRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct SnapCandidates {
    x: Vec<f32>,
    y: Vec<f32>,
}

/// Local canvas interaction state. Never shared with the engine.
#[derive(Default)]
pub struct CanvasUi {
    drag: Option<CanvasDrag>,
    snap_guides: SnapGuides,
    scroll_device: ScrollDevice,
    /// Source under the pointer when the context menu opened
    context_target: Option<SourceId>,
}

enum CanvasDrag {
    Move {
        id: SourceId,
        start: egui::Pos2,
        origin: (f32, f32),
    },
    Resize {
        id: SourceId,
        handle: ResizeHandle,
        start: egui::Pos2,
        origin: (f32, f32, u32, u32),
    },
    Pan {
        start: egui::Pos2,
        pan: [f32; 2],
    },
}

/// A smooth-scrolling mouse reports `Point` exactly like a trackpad, so the
/// phase is the only difference: touch surfaces bracket a gesture with `Start`/`End`, mice never send one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ScrollDevice {
    #[default]
    Wheel,
    Trackpad,
}

fn scroll_device(previous: ScrollDevice, events: &[egui::Event]) -> ScrollDevice {
    let mut device = previous;
    for event in events {
        let egui::Event::MouseWheel { unit, phase, .. } = event else {
            continue;
        };
        device = match unit {
            egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => ScrollDevice::Wheel,
            egui::MouseWheelUnit::Point => match phase {
                egui::TouchPhase::Start => ScrollDevice::Trackpad,
                egui::TouchPhase::End | egui::TouchPhase::Cancel => ScrollDevice::Wheel,
                egui::TouchPhase::Move => device,
            },
        };
    }
    return device;
}

fn scroll_nav(zoom_delta: f32, scroll: egui::Vec2, shift: bool, device: ScrollDevice, zoom_speed: f32) -> Option<Command> {
    if zoom_delta != 1.0 {
        return Some(Command::Zoom { factor: zoom_delta });
    }
    if scroll == egui::Vec2::ZERO {
        return None;
    }
    let factor = if shift {
        ((scroll.x + scroll.y) * zoom_speed).exp()
    } else if device == ScrollDevice::Wheel {
        1.1_f32.powf(scroll.y / 50.0)
    } else {
        return Some(Command::Pan { delta: [scroll.x, scroll.y] });
    };
    if factor == 1.0 {
        return None;
    }
    return Some(Command::Zoom { factor });
}

pub struct CanvasCallback {
    rect_px: [f32; 4], // x, y, w, h (physical pixels, origin top-left)
    shared: Arc<compositor::Shared>,
    draw: Arc<Draw>,
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
        queue.write_buffer(&self.shared.vb, 0, bytemuck::cast_slice(&self.draw.verts));
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, rpass: &mut wgpu::RenderPass<'static>, _resources: &egui_wgpu::CallbackResources) {
        let [x, y, w, h] = self.rect_px;
        rpass.set_viewport(x, y, w, h, 0.0, 1.0);
        rpass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
        rpass.set_vertex_buffer(0, self.shared.vb.slice(..));
        rpass.set_index_buffer(self.shared.ib.slice(..), wgpu::IndexFormat::Uint16);
        let mut current_pipeline = None;
        for draw_call in &self.draw.draws {
            let pipeline = match draw_call.pipeline {
                compositor::Pipeline::Main => &self.shared.pipeline,
                compositor::Pipeline::Text => &self.shared.text_pipeline,
            };
            if current_pipeline != Some(pipeline) {
                rpass.set_pipeline(pipeline);
                current_pipeline = Some(pipeline);
            }
            rpass.set_bind_group(0, &*draw_call.bind_group, &[]);
            rpass.draw_indexed(draw_call.first_index..draw_call.first_index + 6, 0, 0..1);
        }
    }
}

pub fn update(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    commands: &Sender<Command>,
    state: &mut CanvasUi,
    preview: Option<&PreviewFrame>,
    ctx: &egui::Context,
) -> Rect {
    let rect = ui.available_rect_before_wrap();
    let panel_rect = Rect {
        x: rect.min.x,
        y: rect.min.y,
        w: rect.width(),
        h: rect.height(),
    };
    let (response, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());

    let doc = &snapshot.state;
    let canvas = &doc.cfg.canvas;
    let ui_visible = doc.ui_visible;
    let transform = if ui_visible {
        display_transform(canvas, doc.view.zoom, doc.view.pan, &panel_rect)
    } else {
        display_transform(canvas, 1.0, [0.0, 0.0], &panel_rect)
    };
    let (scale, offset_x, offset_y) = transform;
    let origin_x = panel_rect.x + offset_x;
    let origin_y = panel_rect.y + offset_y;

    ctx.input(|i| {
        state.scroll_device = scroll_device(state.scroll_device, &i.events);
    });

    let (primary_down, middle_down, primary_pressed, middle_pressed) = ctx.input(|i| {
        (
            i.pointer.primary_down(),
            i.pointer.middle_down(),
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_pressed(egui::PointerButton::Middle),
        )
    });
    if !primary_down && !middle_down {
        state.drag = None;
        state.snap_guides = SnapGuides::default();
    } else if ui_visible
        && (primary_pressed || middle_pressed)
        && response.hovered()
        && let Some(pos) = response.interact_pointer_pos()
    {
        response.request_focus();
        if middle_pressed || doc.expanded_source_id.is_some() {
            state.drag = Some(CanvasDrag::Pan {
                start: pos,
                pan: doc.view.pan,
            });
        } else if let Some((id, handle)) = hit_resize_handle(canvas, doc.selected_source_id.as_deref(), pos, scale, origin_x, origin_y) {
            let source = canvas.sources.iter().find(|source| source.uuid == id).expect("hit-tested source exists");
            state.drag = Some(CanvasDrag::Resize {
                id,
                handle,
                start: pos,
                origin: (source.x, source.y, source.width, source.height),
            });
        } else if let Some(id) = hit_test(canvas, pos, scale, origin_x, origin_y) {
            let _ = commands.send(Command::SelectSource(Selector::Id(id.clone())));
            let source = canvas.sources.iter().find(|source| source.uuid == id).expect("hit-tested source exists");
            state.drag = Some(CanvasDrag::Move {
                id,
                start: pos,
                origin: (source.x, source.y),
            });
        } else {
            state.drag = Some(CanvasDrag::Pan {
                start: pos,
                pan: doc.view.pan,
            });
        }
    }

    if ui_visible && response.dragged() {
        let mut guides = state.snap_guides;
        match &state.drag {
            Some(CanvasDrag::Move { id, start, origin }) => {
                let pointer = response.interact_pointer_pos().unwrap_or(*start);
                if let Some(source) = canvas.sources.iter().find(|source| source.uuid == *id) {
                    let ((x, y), next) = snap_move(
                        canvas,
                        id,
                        *origin,
                        (source.width as f32, source.height as f32),
                        (pointer.x - start.x, pointer.y - start.y),
                        scale,
                        guides,
                    );
                    guides = next;
                    let _ = commands.send(Command::SetSourceRect {
                        source: Selector::Id(id.clone()),
                        x,
                        y,
                        width: source.width,
                        height: source.height,
                    });
                }
            }
            Some(CanvasDrag::Resize { id, handle, start, origin }) => {
                let pointer = response.interact_pointer_pos().unwrap_or(*start);
                let ((x, y, width, height), next) =
                    snap_resize(canvas, id, *handle, *origin, (pointer.x - start.x, pointer.y - start.y), scale, guides);
                guides = next;
                let _ = commands.send(Command::SetSourceRect {
                    source: Selector::Id(id.clone()),
                    x,
                    y,
                    width,
                    height,
                });
            }
            Some(CanvasDrag::Pan { start, pan }) => {
                guides = SnapGuides::default();
                let _ = commands.send(Command::SetView {
                    zoom: doc.view.zoom,
                    pan: [
                        pan[0] + response.interact_pointer_pos().unwrap_or(*start).x - start.x,
                        pan[1] + response.interact_pointer_pos().unwrap_or(*start).y - start.y,
                    ],
                });
            }
            None => {}
        }
        state.snap_guides = guides;
    }

    if ui_visible
        && response.hovered()
        && !primary_down
        && !middle_down
        && let Some(pos) = response.hover_pos()
        && let Some((_, handle)) = hit_resize_handle(canvas, doc.selected_source_id.as_deref(), pos, scale, origin_x, origin_y)
    {
        ui.output_mut(|output| output.cursor_icon = cursor_for_handle(handle));
    }

    if let Some(CanvasDrag::Pan { .. }) = &state.drag {
        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::Grabbing);
    }

    if ui_visible && response.hovered() {
        let (zoom_delta, scroll, shift) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.modifiers.shift));
        let zoom_speed = ctx.options(|o| o.input_options.scroll_zoom_speed);
        let nav = scroll_nav(zoom_delta, scroll, shift, state.scroll_device, zoom_speed);
        if let Some(command) = nav {
            let _ = commands.send(command);
        }
    }

    match preview {
        // The runner can be a frame behind a resize; painting the last frame
        // stretched is less jarring than flashing the placeholder layer.
        Some(preview) => {
            let ppp = ctx.pixels_per_point();
            painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                CanvasCallback {
                    rect_px: [rect.min.x * ppp, rect.min.y * ppp, rect.width() * ppp, rect.height() * ppp],
                    shared: preview.shared.clone(),
                    draw: preview.draw.clone(),
                },
            ));
        }
        None => draw_placeholders(&painter, snapshot, &panel_rect, transform),
    }

    if ui_visible {
        draw_overlays(
            canvas,
            &panel_rect,
            &painter,
            doc.selected_source_id.as_deref(),
            doc.expanded_source_id.is_some(),
            &state.snap_guides,
            transform,
        );
        if doc.show_stats {
            draw_source_stats_overlay(&painter, snapshot, &panel_rect, transform);
        }
    }

    if ui_visible {
        if response.secondary_clicked() && doc.expanded_source_id.is_none() {
            let target = response.interact_pointer_pos().and_then(|pos| hit_test(canvas, pos, scale, origin_x, origin_y));
            if let Some(id) = &target {
                let _ = commands.send(Command::SelectSource(Selector::Id(id.clone())));
            }
            state.context_target = target;
        }
        response.context_menu(|ui| {
            ui.set_min_width(100.0);
            if doc.expanded_source_id.is_some() {
                if ui.button("Exit expanded view").clicked() {
                    let _ = commands.send(Command::ExitExpanded);
                    ui.close();
                }
            } else if let Some(id) = state.context_target.clone() {
                source_menu(ui, canvas, &id, commands);
            } else if ui.button("Add source").clicked() {
                let _ = commands.send(Command::AddSource);
                ui.close();
            }
        });
    }

    if ui_visible {
        if doc.expanded_source_id.is_some() {
            let close_icon = asset_image!("close.svg");
            let close_image = egui::Image::new(close_icon).fit_to_exact_size(egui::vec2(25.0, 25.0));
            let close_rect = egui::Rect::from_min_size(egui::pos2(rect.max.x - 80.0, rect.min.y + 10.0), egui::vec2(30.0, 30.0));
            let close_btn = egui::Button::image(close_image).corner_radius(5.0);
            if ui.put(close_rect, close_btn).on_hover_text("Exit expanded view").clicked() {
                let _ = commands.send(Command::ExitExpanded);
            }
        }

        let btn_icon = asset_image!("compress.svg");
        let btn_image = egui::Image::new(btn_icon).fit_to_exact_size(egui::vec2(25.0, 25.0));
        let btn_rect = egui::Rect::from_min_size(egui::pos2(rect.max.x - 40.0, rect.min.y + 10.0), egui::vec2(30.0, 30.0));
        let btn = egui::Button::image(btn_image).corner_radius(5.0);
        if ui.put(btn_rect, btn).on_hover_text("Re-center view").clicked() {
            let _ = commands.send(Command::RecenterView);
        }
    }

    return panel_rect;
}

/// Actions on the source the menu was opened on. Every item targets `id`, never
/// the selection, so the menu cannot drift from what was right-clicked.
fn source_menu(ui: &mut egui::Ui, canvas: &Canvas, id: &SourceId, commands: &Sender<Command>) {
    let Some(source) = canvas.sources.iter().find(|source| source.uuid == *id) else {
        return;
    };
    let selector = Selector::Id(id.clone());
    let (back, front) = canvas.z_bounds();

    if ui.button("Expand to full canvas").clicked() {
        let _ = commands.send(Command::ExpandSource(selector.clone()));
        ui.close();
    }

    ui.separator();

    if ui.button("Bring to front").clicked() {
        let _ = commands.send(Command::SetSourceZ {
            source: selector.clone(),
            value: front.saturating_add(1),
        });
        ui.close();
    }
    if ui.button("Send to back").clicked() {
        let _ = commands.send(Command::SetSourceZ {
            source: selector.clone(),
            value: back.saturating_sub(1),
        });
        ui.close();
    }
    if ui.button("Fit to canvas").clicked() {
        let _ = commands.send(Command::SetSourceRect {
            source: selector.clone(),
            x: 0.0,
            y: 0.0,
            width: canvas.width,
            height: canvas.height,
        });
        ui.close();
    }

    ui.separator();

    ui.menu_button("Mode", |ui| {
        for mode in [TextureMode::Fit, TextureMode::Fill, TextureMode::Stretch] {
            if ui.selectable_label(source.mode == mode, mode.label()).clicked() {
                let _ = commands.send(Command::SetSourceMode {
                    source: selector.clone(),
                    mode,
                });
                ui.close();
            }
        }
    });
    let mut flip_h = source.flip_h;
    if ui.checkbox(&mut flip_h, "Flip horizontal").changed() {
        let _ = commands.send(Command::SetSourceFlipH {
            source: selector.clone(),
            value: flip_h,
        });
        ui.close();
    }
    let mut flip_v = source.flip_v;
    if ui.checkbox(&mut flip_v, "Flip vertical").changed() {
        let _ = commands.send(Command::SetSourceFlipV {
            source: selector.clone(),
            value: flip_v,
        });
        ui.close();
    }

    ui.separator();

    ui.menu_button("Label", |ui| {
        for visibility in [SourceLabelVisibility::Inherit, SourceLabelVisibility::Show, SourceLabelVisibility::Hide] {
            let label = visibility.label();
            if ui.selectable_label(source.label_visibility == visibility, label).clicked() {
                let _ = commands.send(Command::SetSourceLabelVisibility {
                    source: selector.clone(),
                    value: visibility,
                });
                ui.close();
            }
        }
    });
    ui.menu_button("Border", |ui| {
        for visibility in [
            SourceBorderVisibility::Inherit,
            SourceBorderVisibility::Show,
            SourceBorderVisibility::Hide,
        ] {
            let label = visibility.label();
            if ui.selectable_label(source.border_visibility == visibility, label).clicked() {
                let _ = commands.send(Command::SetSourceBorderVisibility {
                    source: selector.clone(),
                    value: visibility,
                });
                ui.close();
            }
        }
    });

    ui.separator();

    if ui.button("Duplicate source").clicked() {
        let _ = commands.send(Command::DuplicateSource(selector.clone()));
        ui.close();
    }
    if ui.button("Delete source").clicked() {
        let _ = commands.send(Command::RemoveSource(selector));
        ui.close();
    }
}

fn display_transform(canvas: &Canvas, zoom: f32, pan: [f32; 2], panel: &Rect) -> (f32, f32, f32) {
    let (base_scale, base_ox, base_oy) = compositor::canvas_transform(canvas, panel);
    return (base_scale * zoom, base_ox + pan[0], base_oy + pan[1]);
}

fn hit_test(canvas: &Canvas, pos: egui::Pos2, scale: f32, origin_x: f32, origin_y: f32) -> Option<SourceId> {
    let cx = (pos.x - origin_x) / scale;
    let cy = (pos.y - origin_y) / scale;
    let mut sources: Vec<&Source> = canvas.sources.iter().collect();
    sources.sort_by_key(|source| source.z);
    for source in sources.into_iter().rev() {
        let x0 = source.x;
        let y0 = source.y;
        let x1 = source.x + source.width as f32;
        let y1 = source.y + source.height as f32;
        if cx >= x0 && cx < x1 && cy >= y0 && cy < y1 {
            return Some(source.uuid.clone());
        }
    }
    return None;
}

fn hit_resize_handle(
    canvas: &Canvas,
    selected: Option<&str>,
    pos: egui::Pos2,
    scale: f32,
    origin_x: f32,
    origin_y: f32,
) -> Option<(SourceId, ResizeHandle)> {
    let selected = selected?;
    let source = canvas.sources.iter().find(|source| source.uuid == selected)?;
    let x0 = origin_x + source.x * scale;
    let y0 = origin_y + source.y * scale;
    let x1 = x0 + source.width as f32 * scale;
    let y1 = y0 + source.height as f32 * scale;
    let near = |a: f32, b: f32| (a - b).abs() <= HANDLE_TOLERANCE;
    let left = near(pos.x, x0);
    let right = near(pos.x, x1);
    let top = near(pos.y, y0);
    let bottom = near(pos.y, y1);
    let handle = match (left, right, top, bottom) {
        (true, _, true, _) => ResizeHandle::TopLeft,
        (true, _, _, true) => ResizeHandle::BottomLeft,
        (_, true, true, _) => ResizeHandle::TopRight,
        (_, true, _, true) => ResizeHandle::BottomRight,
        (true, false, false, false) => ResizeHandle::Left,
        (false, true, false, false) => ResizeHandle::Right,
        (false, false, true, false) => ResizeHandle::Top,
        (false, false, false, true) => ResizeHandle::Bottom,
        _ => return None,
    };
    return Some((source.uuid.clone(), handle));
}

fn snap_candidates(canvas: &Canvas, exclude_uuid: &str) -> SnapCandidates {
    let mut x = vec![0.0, canvas.width as f32];
    let mut y = vec![0.0, canvas.height as f32];
    for source in &canvas.sources {
        if source.uuid == exclude_uuid {
            continue;
        }
        x.push(source.x);
        x.push(source.x + source.width as f32);
        y.push(source.y);
        y.push(source.y + source.height as f32);
    }
    return SnapCandidates { x, y };
}

fn snap_value(value: f32, candidates: &[f32], snap_threshold: f32, break_threshold: f32, current: Option<f32>) -> Option<f32> {
    let mut best = None;
    let mut best_dist = f32::INFINITY;
    for &candidate in candidates {
        let dist = (candidate - value).abs();
        if dist < best_dist {
            best_dist = dist;
            best = Some(candidate);
        }
    }
    if best_dist < snap_threshold {
        return best;
    }
    // Hysteresis: stay snapped to the current guide until the pointer moves
    // past the larger break threshold.
    if let Some(current) = current {
        let dist = (current - value).abs();
        if dist < break_threshold {
            return Some(current);
        }
    }
    return None;
}

fn resolve_axis_snap(snap_lo: Option<f32>, snap_hi: Option<f32>, lo: f32, hi: f32) -> (f32, f32) {
    return match (snap_lo, snap_hi) {
        (Some(lo_snap), Some(hi_snap)) => {
            if (lo_snap - lo).abs() < (hi_snap - hi).abs() {
                (lo_snap - lo, lo_snap)
            } else {
                (hi_snap - hi, hi_snap)
            }
        }
        (Some(lo_snap), None) => (lo_snap - lo, lo_snap),
        (None, Some(hi_snap)) => (hi_snap - hi, hi_snap),
        (None, None) => (0.0, 0.0),
    };
}

fn snap_move(
    canvas: &Canvas,
    id: &SourceId,
    origin: (f32, f32),
    size: (f32, f32),
    delta: (f32, f32),
    scale: f32,
    guides: SnapGuides,
) -> ((f32, f32), SnapGuides) {
    let candidates = snap_candidates(canvas, id);
    let snap_threshold = SNAP_THRESHOLD / scale;
    let break_threshold = SNAP_BREAK_THRESHOLD / scale;

    let proposed_x = origin.0 + delta.0 / scale;
    let proposed_y = origin.1 + delta.1 / scale;
    let left = proposed_x;
    let right = proposed_x + size.0;
    let top = proposed_y;
    let bottom = proposed_y + size.1;

    let snap_left = snap_value(left, &candidates.x, snap_threshold, break_threshold, guides.x);
    let snap_right = snap_value(right, &candidates.x, snap_threshold, break_threshold, guides.x);
    let snap_top = snap_value(top, &candidates.y, snap_threshold, break_threshold, guides.y);
    let snap_bottom = snap_value(bottom, &candidates.y, snap_threshold, break_threshold, guides.y);

    let (x_offset, guide_x) = resolve_axis_snap(snap_left, snap_right, left, right);
    let (y_offset, guide_y) = resolve_axis_snap(snap_top, snap_bottom, top, bottom);

    return (
        ((proposed_x + x_offset).round(), (proposed_y + y_offset).round()),
        SnapGuides {
            x: if x_offset != 0.0 { Some(guide_x) } else { None },
            y: if y_offset != 0.0 { Some(guide_y) } else { None },
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn snap_resize(
    canvas: &Canvas,
    id: &SourceId,
    handle: ResizeHandle,
    origin: (f32, f32, u32, u32),
    delta: (f32, f32),
    scale: f32,
    guides: SnapGuides,
) -> ((f32, f32, u32, u32), SnapGuides) {
    let candidates = snap_candidates(canvas, id);
    let snap_threshold = SNAP_THRESHOLD / scale;
    let break_threshold = SNAP_BREAK_THRESHOLD / scale;
    let start = WorldRect {
        x: origin.0,
        y: origin.1,
        w: origin.2 as f32,
        h: origin.3 as f32,
    };
    let dx = delta.0 / scale;
    let dy = delta.1 / scale;

    let (x, y, w, h, guide_x, guide_y) = match handle {
        ResizeHandle::Left => {
            let (x, w, guide) = resize_leading(start.x, start.w, dx, &candidates.x, snap_threshold, break_threshold, guides.x);
            (x, start.y, w, start.h, guide, None)
        }
        ResizeHandle::Right => {
            let (_, w, guide) = resize_trailing(start.x, start.w, dx, &candidates.x, snap_threshold, break_threshold, guides.x);
            (start.x, start.y, w, start.h, guide, None)
        }
        ResizeHandle::Top => {
            let (y, h, guide) = resize_leading(start.y, start.h, dy, &candidates.y, snap_threshold, break_threshold, guides.y);
            (start.x, y, start.w, h, None, guide)
        }
        ResizeHandle::Bottom => {
            let (_, h, guide) = resize_trailing(start.y, start.h, dy, &candidates.y, snap_threshold, break_threshold, guides.y);
            (start.x, start.y, start.w, h, None, guide)
        }
        _ => resize_corner(handle, start, dx, dy, &candidates, snap_threshold, break_threshold, guides),
    };

    return (
        (x.round(), y.round(), w.max(1.0).round() as u32, h.max(1.0).round() as u32),
        SnapGuides { x: guide_x, y: guide_y },
    );
}

fn resize_leading(
    start: f32,
    size: f32,
    delta: f32,
    candidates: &[f32],
    snap_threshold: f32,
    break_threshold: f32,
    current_guide: Option<f32>,
) -> (f32, f32, Option<f32>) {
    if let Some(snap) = snap_value(start + delta, candidates, snap_threshold, break_threshold, current_guide) {
        let pos = snap.min(start + size - 1.0);
        return (pos, (start + size) - pos, Some(snap));
    }
    let new_pos = start + delta;
    let new_size = (start + size) - new_pos;
    if new_size >= 1.0 {
        return (new_pos, new_size, None);
    }
    return (start + size - 1.0, 1.0, None);
}

fn resize_trailing(
    start: f32,
    size: f32,
    delta: f32,
    candidates: &[f32],
    snap_threshold: f32,
    break_threshold: f32,
    current_guide: Option<f32>,
) -> (f32, f32, Option<f32>) {
    if let Some(snap) = snap_value(start + size + delta, candidates, snap_threshold, break_threshold, current_guide) {
        return (start, (snap - start).max(1.0), Some(snap));
    }
    return (start, (size + delta).max(1.0), None);
}

#[allow(clippy::too_many_arguments)]
fn resize_corner(
    handle: ResizeHandle,
    start: WorldRect,
    dx: f32,
    dy: f32,
    candidates: &SnapCandidates,
    snap_threshold: f32,
    break_threshold: f32,
    guides: SnapGuides,
) -> (f32, f32, f32, f32, Option<f32>, Option<f32>) {
    let (fx, fy) = match handle {
        ResizeHandle::TopLeft => (start.x + start.w, start.y + start.h),
        ResizeHandle::TopRight => (start.x, start.y + start.h),
        ResizeHandle::BottomRight => (start.x, start.y),
        ResizeHandle::BottomLeft => (start.x + start.w, start.y),
        _ => unreachable!(),
    };
    let mx0 = start.x + start.w - (fx - start.x);
    let my0 = start.y + start.h - (fy - start.y);
    let diag_x = mx0 - fx;
    let diag_y = my0 - fy;
    let denom = diag_x * diag_x + diag_y * diag_y;
    if denom <= 0.0 {
        return (start.x, start.y, start.w, start.h, None, None);
    }

    let t = ((mx0 + dx - fx) * diag_x + (my0 + dy - fy) * diag_y) / denom;
    let min_t = (1.0 / start.w).max(1.0 / start.h);
    let t = t.max(min_t);
    let mut mx = fx + t * diag_x;
    let mut my = fy + t * diag_y;

    let snap_mx = snap_value(mx, &candidates.x, snap_threshold, break_threshold, guides.x);
    let snap_my = snap_value(my, &candidates.y, snap_threshold, break_threshold, guides.y);
    let dist_x = snap_mx.map(|value| (value - mx).abs());
    let dist_y = snap_my.map(|value| (value - my).abs());
    let mut guide_x = None;
    let mut guide_y = None;
    match (dist_x, dist_y) {
        (Some(dist_x), Some(dist_y)) => {
            if dist_x < dist_y {
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
    let x = match handle {
        ResizeHandle::TopLeft | ResizeHandle::BottomLeft => fx - new_w,
        _ => fx,
    };
    let y = match handle {
        ResizeHandle::TopLeft | ResizeHandle::TopRight => fy - new_h,
        _ => fy,
    };
    return (x, y, new_w, new_h, guide_x, guide_y);
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
    canvas: &Canvas,
    panel_rect: &Rect,
    painter: &egui::Painter,
    selected_source_id: Option<&str>,
    expanded: bool,
    snap_guides: &SnapGuides,
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

    if expanded {
        return;
    }

    let Some(selected_uuid) = selected_source_id else {
        return;
    };
    let Some(source) = canvas.sources.iter().find(|source| source.uuid == selected_uuid) else {
        return;
    };
    let lx = cx + source.x * scale;
    let ly = cy + source.y * scale;
    let lw = source.width as f32 * scale;
    let lh = source.height as f32 * scale;
    painter.rect_stroke(
        egui::Rect::from_min_size(egui::pos2(lx, ly), egui::vec2(lw, lh)),
        0.0,
        egui::Stroke::new(canvas.border.width, egui::Color32::YELLOW),
        egui::StrokeKind::Inside,
    );

    let guide_stroke = egui::Stroke::new(1.0, egui::Color32::CYAN);
    if let Some(gx) = snap_guides.x {
        let sx = cx + gx * scale;
        painter.vline(sx, panel_rect.y..=panel_rect.y + panel_rect.h, guide_stroke);
    }
    if let Some(gy) = snap_guides.y {
        let sy = cy + gy * scale;
        painter.hline(panel_rect.x..=panel_rect.x + panel_rect.w, sy, guide_stroke);
    }
}

fn draw_placeholders(painter: &egui::Painter, snapshot: &EngineSnapshot, panel_rect: &Rect, transform: (f32, f32, f32)) {
    let doc = &snapshot.state;
    let canvas = &doc.cfg.canvas;
    let (scale, offset_x, offset_y) = transform;
    let cx = panel_rect.x + offset_x;
    let cy = panel_rect.y + offset_y;
    let cw = canvas.width as f32 * scale;
    let ch = canvas.height as f32 * scale;
    let expanded = doc.expanded_source_id.is_some();

    let clip = egui::Rect::from_min_size(egui::pos2(panel_rect.x, panel_rect.y), egui::vec2(panel_rect.w.max(0.0), panel_rect.h.max(0.0)));
    let painter = painter.with_clip_rect(clip);

    let mut sources: Vec<&Source> = canvas.sources.iter().collect();
    sources.sort_by_key(|source| source.z);
    for source in sources {
        if expanded && doc.expanded_source_id.as_deref() != Some(source.uuid.as_str()) {
            continue;
        }
        let (x, y, w, h) = if expanded {
            (cx, cy, cw, ch)
        } else {
            (
                cx + source.x * scale,
                cy + source.y * scale,
                source.width as f32 * scale,
                source.height as f32 * scale,
            )
        };
        let source_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w.max(1.0), h.max(1.0)));
        let color = source_color(&source.uuid);
        painter.rect_filled(source_rect, 2.0, egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 45));
        painter.rect_stroke(source_rect, 2.0, egui::Stroke::new(1.0, color), egui::StrokeKind::Inside);

        if w < 40.0 || h < 24.0 {
            continue;
        }
        let panel_clip = egui::Rect::from_min_size(egui::pos2(panel_rect.x, panel_rect.y), egui::vec2(panel_rect.w.max(0.0), panel_rect.h.max(0.0)));
        let text_painter = painter.with_clip_rect(panel_clip.intersect(source_rect));
        text_painter.text(
            egui::pos2(x + 8.0, y + 6.0),
            egui::Align2::LEFT_TOP,
            &source.name,
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
        let mut line_y = y + 24.0;
        let mut source_shown = false;
        if w >= 60.0 && h >= 44.0 {
            text_painter.text(
                egui::pos2(x + 8.0, line_y),
                egui::Align2::LEFT_TOP,
                source_heading(source),
                egui::FontId::monospace(11.0),
                egui::Color32::from_gray(200),
            );
            line_y += 16.0;
            source_shown = true;
        }
        let stats = placeholder_stats(snapshot, source);
        let stats_min_height = if source_shown { 62.0 } else { 44.0 };
        if !stats.is_empty() && h >= stats_min_height {
            text_painter.text(
                egui::pos2(x + 8.0, line_y),
                egui::Align2::LEFT_TOP,
                stats,
                egui::FontId::monospace(11.0),
                egui::Color32::from_gray(180),
            );
        }
        if let Some(source_ref) = &source.source_ref {
            let key = SourceKey::new(source.protocol.clone(), source_ref.clone());
            if let Some(runtime) = snapshot.runtime.source(&key) {
                let live = runtime.stats.frames_received > 0 && !runtime.restart_pending;
                painter.circle_filled(
                    egui::pos2(source_rect.right() - 12.0, source_rect.top() + 12.0),
                    4.0,
                    if live {
                        egui::Color32::from_rgb(110, 220, 120)
                    } else {
                        egui::Color32::from_rgb(220, 110, 110)
                    },
                );
            }
        }
    }
}

/// The protocol and raw source reference a placement is bound to.
fn source_heading(source: &Source) -> String {
    return match &source.source_ref {
        Some(source_ref) => format!("{} · {source_ref}", source.protocol.label()),
        None => source.protocol.label().to_string(),
    };
}

fn placeholder_stats(snapshot: &EngineSnapshot, source: &Source) -> String {
    let Some(source_ref) = &source.source_ref else {
        return String::new();
    };
    let key = SourceKey::new(source.protocol.clone(), source_ref.clone());
    let Some(runtime) = snapshot.runtime.source(&key) else {
        return "not connected".to_string();
    };
    let stats = &runtime.stats;
    if stats.width == 0 || stats.height == 0 {
        return "connecting".to_string();
    }
    return format!("{}x{} · {:.0} fps", stats.width, stats.height, stats.computed_fps);
}

/// Extended stats rows for a placed source; empty when it is not bound.
fn source_stats_lines(snapshot: &EngineSnapshot, source: &Source) -> Vec<String> {
    let Some(source_ref) = &source.source_ref else {
        return Vec::new();
    };
    let key = SourceKey::new(source.protocol.clone(), source_ref.clone());
    let runtime = snapshot.runtime.source(&key);
    return stats_lines(runtime.map(|runtime| &runtime.stats), runtime.is_some_and(|runtime| runtime.restart_pending));
}

fn stats_lines(stats: Option<&SourceStats>, restarting: bool) -> Vec<String> {
    if restarting {
        return vec!["restarting...".to_string()];
    }
    let Some(stats) = stats else {
        return vec!["not connected".to_string()];
    };
    if stats.width == 0 || stats.height == 0 {
        return vec!["connecting".to_string()];
    }

    let mut lines = vec![format!("{}x{} {}", stats.width, stats.height, stats.pixel_format)];
    if stats.nominal_fps > 0.0 {
        lines.push(format!("{:.2} / {:.2} fps", stats.computed_fps, stats.nominal_fps));
    } else {
        lines.push(format!("{:.2} fps", stats.computed_fps));
    }
    lines.push(format!(
        "{} recv · {} shown · {} dropped",
        stats.frames_received, stats.frames_consumed, stats.frames_dropped
    ));
    if stats.off_screen {
        lines.push("offscreen: holding last frame".to_string());
    }
    let mut io = vec![format!("recv {:.2} ms", stats.receive_time_ms)];
    if stats.gpu_direct {
        if stats.network_mbps > 0.0 {
            io.push(format!("net {:.1} MB/s", stats.network_mbps));
        }
        if stats.decode_ms > 0.0 {
            io.push(format!("decode {:.2} ms", stats.decode_ms));
        }
    } else {
        io.push(format!("copy {:.2} ms", stats.copy_time_ms));
        io.push(format!("upload {:.2} ms", stats.upload_time_ms));
        if stats.upload_mbps > 0.0 {
            io.push(format!("{:.1} MB/s", stats.upload_mbps));
        }
    }
    lines.push(io.join(" · "));
    return lines;
}

/// Draw the stats block over each visible placement. Viewer-only: never part of
/// the compositor draw, so it cannot reach an output.
fn draw_source_stats_overlay(painter: &egui::Painter, snapshot: &EngineSnapshot, panel_rect: &Rect, transform: (f32, f32, f32)) {
    let doc = &snapshot.state;
    let canvas = &doc.cfg.canvas;
    let (scale, offset_x, offset_y) = transform;
    let cx = panel_rect.x + offset_x;
    let cy = panel_rect.y + offset_y;
    let expanded = doc.expanded_source_id.as_deref();
    let clip = egui::Rect::from_min_size(egui::pos2(panel_rect.x, panel_rect.y), egui::vec2(panel_rect.w.max(0.0), panel_rect.h.max(0.0)));
    let painter = painter.with_clip_rect(clip);
    let font = egui::FontId::monospace(11.0);

    let mut sources: Vec<&Source> = canvas.sources.iter().collect();
    sources.sort_by_key(|source| source.z);
    for source in sources {
        if expanded.is_some_and(|uuid| uuid != source.uuid) {
            continue;
        }
        let (x, y, w, h) = if expanded.is_some() {
            (cx, cy, canvas.width as f32 * scale, canvas.height as f32 * scale)
        } else {
            (
                cx + source.x * scale,
                cy + source.y * scale,
                source.width as f32 * scale,
                source.height as f32 * scale,
            )
        };
        if w < STATS_MIN_WIDTH || h < STATS_MIN_HEIGHT {
            continue;
        }

        let mut lines = Vec::new();
        if !source.name.is_empty() {
            lines.push(source.name.clone());
        }
        lines.push(source_heading(source));
        lines.extend(source_stats_lines(snapshot, source));

        let mut galleys = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            let color = if index == 0 && !source.name.is_empty() {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_gray(200)
            };
            galleys.push(painter.layout_no_wrap(line.clone(), font.clone(), color));
        }
        let max_lines = ((h - STATS_INSET * 2.0) / STATS_LINE_HEIGHT).floor().max(0.0) as usize;
        let used = galleys.len().min(max_lines);
        if used == 0 {
            continue;
        }

        let text_w = galleys[..used].iter().map(|galley| galley.size().x).fold(0.0_f32, f32::max);
        let text_x = (x + STATS_INSET).min(x + w);
        let text_y = (y + h - STATS_INSET - used as f32 * STATS_LINE_HEIGHT).max(y + STATS_INSET);
        let box_rect = egui::Rect::from_min_size(
            egui::pos2(text_x - 4.0, text_y - 3.0),
            egui::vec2((text_w + 8.0).min(x + w - text_x + 4.0), used as f32 * STATS_LINE_HEIGHT + 6.0),
        );
        let text_painter = painter.with_clip_rect(clip.intersect(egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))));
        text_painter.rect_filled(box_rect, 2.0, egui::Color32::from_black_alpha(160));
        let mut line_y = text_y;
        for (index, galley) in galleys.into_iter().take(used).enumerate() {
            let color = if index == 0 && !source.name.is_empty() {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_gray(200)
            };
            text_painter.galley(egui::pos2(text_x, line_y), galley, color);
            line_y += STATS_LINE_HEIGHT;
        }
    }
}

fn source_color(uuid: &str) -> egui::Color32 {
    const COLORS: [egui::Color32; 8] = [
        egui::Color32::from_rgb(92, 156, 232),
        egui::Color32::from_rgb(226, 160, 92),
        egui::Color32::from_rgb(130, 200, 130),
        egui::Color32::from_rgb(202, 130, 202),
        egui::Color32::from_rgb(226, 118, 124),
        egui::Color32::from_rgb(120, 196, 200),
        egui::Color32::from_rgb(190, 190, 110),
        egui::Color32::from_rgb(160, 150, 230),
    ];
    let hash = uuid.bytes().fold(0u32, |hash, byte| hash.wrapping_mul(31).wrapping_add(byte as u32));
    return COLORS[hash as usize % COLORS.len()];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Canvas, Source, TextureMode};
    use crate::engine::Engine;
    use crate::sources::Protocol;

    fn source(uuid: &str, x: f32, y: f32, width: u32, height: u32, z: i32) -> Source {
        let mut source = Source::new(uuid.into(), Protocol::Test, None, x, y, width, height, z, TextureMode::Fit, false, false);
        source.uuid = uuid.into();
        return source;
    }

    fn canvas(sources: Vec<Source>) -> Canvas {
        return Canvas {
            sources,
            ..Canvas::default()
        };
    }

    #[test]
    fn source_heading_shows_the_protocol_and_source_ref() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        let snapshot = engine.snapshot();
        let source = snapshot.state.cfg.canvas.sources.iter().find(|source| source.uuid == uuid).unwrap();
        assert_eq!(source_heading(source), format!("Test · {}", source.source_ref.clone().unwrap()));
    }

    #[test]
    fn source_heading_hides_the_ref_when_unbound_and_stats_report_the_key() {
        let mut bound = source("bound", 0.0, 0.0, 640, 360, 0);
        bound.source_ref = Some("missing".to_string());
        let unbound = source("unbound", 0.0, 0.0, 640, 360, 0);
        let engine = Engine::test_instance(canvas(vec![bound, unbound]));
        let snapshot = engine.snapshot();

        assert_eq!(source_heading(&snapshot.state.cfg.canvas.sources[0]), "Test · missing");
        assert_eq!(placeholder_stats(&snapshot, &snapshot.state.cfg.canvas.sources[0]), "not connected");
        assert_eq!(source_heading(&snapshot.state.cfg.canvas.sources[1]), "Test");
        assert_eq!(placeholder_stats(&snapshot, &snapshot.state.cfg.canvas.sources[1]), "");
    }

    #[test]
    fn hit_test_uses_canvas_coordinates_and_topmost_z() {
        let canvas = canvas(vec![source("a", 0.0, 0.0, 640, 360, 0), source("b", 100.0, 100.0, 640, 360, 1)]);

        assert_eq!(hit_test(&canvas, egui::Pos2::new(200.0, 200.0), 1.0, 0.0, 0.0).as_deref(), Some("b"));
        assert_eq!(hit_test(&canvas, egui::Pos2::new(500.0, 50.0), 1.0, 0.0, 0.0).as_deref(), Some("a"));
        assert!(hit_test(&canvas, egui::Pos2::new(900.0, 900.0), 1.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn hit_test_converts_screen_offsets_and_scale() {
        let canvas = canvas(vec![source("a", 100.0, 50.0, 640, 360, 0)]);

        assert_eq!(hit_test(&canvas, egui::Pos2::new(200.0, 100.0), 2.0, 0.0, 0.0).as_deref(), Some("a"));
        assert!(hit_test(&canvas, egui::Pos2::new(0.0, 0.0), 2.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn resize_handle_is_found_on_selected_source_edges() {
        let canvas = canvas(vec![source("a", 100.0, 50.0, 640, 360, 0)]);

        assert_eq!(
            hit_resize_handle(&canvas, Some("a"), egui::Pos2::new(100.0, 50.0), 1.0, 0.0, 0.0),
            Some(("a".into(), ResizeHandle::TopLeft))
        );
        assert_eq!(
            hit_resize_handle(&canvas, Some("a"), egui::Pos2::new(740.0, 50.0), 1.0, 0.0, 0.0),
            Some(("a".into(), ResizeHandle::TopRight))
        );
        assert!(hit_resize_handle(&canvas, Some("a"), egui::Pos2::new(400.0, 200.0), 1.0, 0.0, 0.0).is_none());
        assert!(hit_resize_handle(&canvas, None, egui::Pos2::new(100.0, 50.0), 1.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn hit_test_uses_the_panel_origin() {
        let canvas = canvas(vec![source("a", 100.0, 50.0, 640, 360, 0)]);

        // World (100, 50) is screen (380, 90) with a panel origin of (280, 40).
        assert_eq!(hit_test(&canvas, egui::Pos2::new(400.0, 100.0), 1.0, 280.0, 40.0).as_deref(), Some("a"));
        assert!(hit_test(&canvas, egui::Pos2::new(120.0, 60.0), 1.0, 280.0, 40.0).is_none());
    }

    #[test]
    fn hit_test_with_panel_origin_picks_the_topmost_source() {
        let canvas = canvas(vec![source("a", 100.0, 50.0, 640, 360, 0), source("b", 200.0, 100.0, 640, 360, 1)]);

        // World (250, 150) is screen (530, 190) with a panel origin of (280, 40).
        assert_eq!(hit_test(&canvas, egui::Pos2::new(530.0, 190.0), 1.0, 280.0, 40.0).as_deref(), Some("b"));
        // World (150, 75) is screen (430, 115); only the lower source contains it.
        assert_eq!(hit_test(&canvas, egui::Pos2::new(430.0, 115.0), 1.0, 280.0, 40.0).as_deref(), Some("a"));
    }

    #[test]
    fn hit_resize_handle_uses_the_panel_origin() {
        let canvas = canvas(vec![source("a", 100.0, 50.0, 640, 360, 0)]);

        assert_eq!(
            hit_resize_handle(&canvas, Some("a"), egui::Pos2::new(380.0, 90.0), 1.0, 280.0, 40.0),
            Some(("a".into(), ResizeHandle::TopLeft))
        );
        assert!(hit_resize_handle(&canvas, Some("a"), egui::Pos2::new(100.0, 50.0), 1.0, 280.0, 40.0).is_none());
    }

    #[test]
    fn snap_move_snaps_to_the_canvas_edge() {
        let canvas = canvas(vec![source("a", 15.0, 100.0, 100, 100, 0)]);

        let ((x, y), guides) = snap_move(&canvas, &"a".into(), (15.0, 100.0), (100.0, 100.0), (-14.0, 0.0), 1.0, SnapGuides::default());

        assert_eq!((x, y), (0.0, 100.0));
        assert!(guides.x.is_some());
    }

    #[test]
    fn snap_move_snaps_to_another_source_edge() {
        let canvas = canvas(vec![source("a", 0.0, 0.0, 100, 100, 0), source("b", 300.0, 0.0, 100, 100, 1)]);

        let ((x, _), guides) = snap_move(&canvas, &"a".into(), (0.0, 0.0), (100.0, 100.0), (199.0, 0.0), 1.0, SnapGuides::default());

        assert_eq!(x, 200.0);
        assert!(guides.x.is_some());
    }

    #[test]
    fn snap_value_keeps_the_current_guide_until_the_break_threshold() {
        assert_eq!(snap_value(1.5, &[0.0], 2.0, 2.0, Some(0.0)), Some(0.0));
        assert_eq!(snap_value(2.5, &[0.0], 2.0, 2.0, Some(0.0)), None);
    }

    #[test]
    fn snap_resize_snaps_the_right_edge_to_a_neighbour() {
        let canvas = canvas(vec![source("a", 100.0, 100.0, 100, 100, 0), source("b", 300.0, 100.0, 100, 100, 1)]);

        let ((x, y, width, height), guides) = snap_resize(
            &canvas,
            &"a".into(),
            ResizeHandle::Right,
            (100.0, 100.0, 100, 100),
            (99.0, 0.0),
            1.0,
            SnapGuides::default(),
        );

        assert_eq!((x, y, width, height), (100.0, 100.0, 200, 100));
        assert!(guides.x.is_some());
    }

    #[test]
    fn snap_resize_keeps_the_corner_aspect() {
        let canvas = canvas(vec![source("a", 100.0, 100.0, 200, 100, 0)]);

        let ((x, y, width, height), _guides) = snap_resize(
            &canvas,
            &"a".into(),
            ResizeHandle::BottomRight,
            (100.0, 100.0, 200, 100),
            (50.0, 25.0),
            1.0,
            SnapGuides::default(),
        );

        assert_eq!((x, y), (100.0, 100.0));
        let aspect = width as f32 / height as f32;
        assert!((aspect - 2.0).abs() < 0.02, "aspect should be preserved, got {aspect}");
    }

    #[test]
    fn display_transform_applies_local_zoom_and_pan() {
        let canvas = Canvas::default();
        let panel = Rect {
            x: 0.0,
            y: 0.0,
            w: 960.0,
            h: 540.0,
        };
        let (base_scale, base_ox, base_oy) = compositor::canvas_transform(&canvas, &panel);
        let (scale, ox, oy) = display_transform(&canvas, 2.0, [10.0, 20.0], &panel);
        assert_eq!(scale, base_scale * 2.0);
        assert_eq!(ox, base_ox + 10.0);
        assert_eq!(oy, base_oy + 20.0);
    }

    #[test]
    fn scroll_nav_splits_wheel_zoom_from_trackpad_pan() {
        let delta = egui::vec2(3.0, 40.0);
        let speed = 1.0 / 200.0;

        assert_eq!(
            scroll_nav(1.0, delta, false, ScrollDevice::Wheel, speed),
            Some(Command::Zoom {
                factor: 1.1_f32.powf(40.0 / 50.0)
            })
        );
        assert_eq!(
            scroll_nav(1.0, delta, false, ScrollDevice::Trackpad, speed),
            Some(Command::Pan { delta: [3.0, 40.0] })
        );
        assert_eq!(
            scroll_nav(1.0, delta, true, ScrollDevice::Trackpad, speed),
            Some(Command::Zoom {
                factor: (43.0 * speed).exp()
            })
        );
        assert_eq!(scroll_nav(1.2, delta, false, ScrollDevice::Trackpad, speed), Some(Command::Zoom { factor: 1.2 }));
        assert_eq!(scroll_nav(1.0, egui::Vec2::ZERO, false, ScrollDevice::Trackpad, speed), None);
    }

    #[test]
    fn scroll_nav_ignores_a_zoom_with_no_change() {
        let speed = 1.0 / 200.0;

        assert_eq!(scroll_nav(1.0, egui::vec2(30.0, 0.0), false, ScrollDevice::Wheel, speed), None);
        assert_eq!(scroll_nav(1.0, egui::vec2(30.0, -30.0), true, ScrollDevice::Trackpad, speed), None);
        assert_eq!(
            scroll_nav(1.0, egui::vec2(30.0, 0.0), false, ScrollDevice::Trackpad, speed),
            Some(Command::Pan { delta: [30.0, 0.0] })
        );
    }

    #[test]
    fn scroll_device_follows_touch_phases() {
        let wheel = |unit, phase| egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(0.0, 10.0),
            phase,
            modifiers: egui::Modifiers::NONE,
        };
        let point = |phase| wheel(egui::MouseWheelUnit::Point, phase);

        let mut device = scroll_device(ScrollDevice::Wheel, &[point(egui::TouchPhase::Move)]);
        assert_eq!(device, ScrollDevice::Wheel);

        device = scroll_device(device, &[point(egui::TouchPhase::Start)]);
        assert_eq!(device, ScrollDevice::Trackpad);

        device = scroll_device(device, &[point(egui::TouchPhase::Move)]);
        assert_eq!(device, ScrollDevice::Trackpad);

        device = scroll_device(device, &[point(egui::TouchPhase::End)]);
        assert_eq!(device, ScrollDevice::Wheel);

        device = scroll_device(ScrollDevice::Trackpad, &[wheel(egui::MouseWheelUnit::Line, egui::TouchPhase::Move)]);
        assert_eq!(device, ScrollDevice::Wheel);
    }

    #[test]
    fn stats_lines_reports_connection_and_frame_state() {
        assert_eq!(stats_lines(None, false), vec!["not connected"]);
        assert_eq!(stats_lines(None, true), vec!["restarting..."]);

        let mut stats = SourceStats::new();
        assert_eq!(stats_lines(Some(&stats), false), vec!["connecting"]);

        stats.record_frame(1920, 1080, "BGRA8", 60.0);
        let lines = stats_lines(Some(&stats), false);
        assert_eq!(lines[0], "1920x1080 BGRA8");
        assert_eq!(lines[1], "0.00 / 60.00 fps");
        assert_eq!(lines[2], "1 recv · 0 shown · 0 dropped");
        assert!(lines[3].contains("recv 0.00 ms") && lines[3].contains("copy"));
    }

    #[test]
    fn stats_lines_reports_network_and_decode_for_gpu_direct_sources() {
        let mut stats = SourceStats::new();
        stats.record_frame(1920, 1080, "BGRA8", 60.0);
        stats.set_gpu_direct();
        stats.network_mbps = 12.5;
        stats.decode_ms = 1.5;
        stats.off_screen = true;

        let lines = stats_lines(Some(&stats), false);
        assert!(lines.iter().any(|line| line.contains("offscreen")));
        let io = lines.last().unwrap();
        assert!(io.contains("net 12.5 MB/s") && io.contains("decode 1.50 ms"));
        assert!(!io.contains("copy"));
    }
}
