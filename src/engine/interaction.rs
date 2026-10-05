use super::{Engine, ResizeHandle, SnapGuides};
use crate::compositor::{self, Rect};

pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 20.0;
pub const SNAP_THRESHOLD: f32 = 2.0;
pub const SNAP_BREAK_THRESHOLD: f32 = 2.0;

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

pub struct SnapCandidates {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
}

// UI/canvas engine methods
impl Engine {
    pub fn display_transform(&self, panel_rect: &Rect) -> (f32, f32, f32) {
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        (
            base_scale * self.view.zoom,
            base_ox + self.view.pan.x,
            base_oy + self.view.pan.y,
        )
    }

    /// Base transform that fits the canvas into the panel without any user pan/zoom.
    pub fn default_transform(&self, panel_rect: &Rect) -> (f32, f32, f32) {
        compositor::canvas_transform(&self.cfg.canvas, panel_rect)
    }

    pub fn recenter_view(&mut self, panel_rect: &Rect) {
        let canvas = &self.cfg.canvas;
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
        let target_scale = (panel_rect.width() / bbox_w).min(panel_rect.height() / bbox_h) * 0.9;
        self.view.zoom = (target_scale / base_scale).clamp(MIN_ZOOM, MAX_ZOOM);
        let display_scale = base_scale * self.view.zoom;
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.view.pan.x = panel_rect.width() / 2.0 - base_ox - cx * display_scale;
        self.view.pan.y = panel_rect.height() / 2.0 - base_oy - cy * display_scale;
    }

    pub fn zoom_view(&mut self, panel_rect: &Rect, factor: f32) {
        let (base_scale, base_ox, base_oy) =
            compositor::canvas_transform(&self.cfg.canvas, panel_rect);
        let old_zoom = self.view.zoom;
        let new_zoom = (old_zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let center = egui::vec2(panel_rect.w / 2.0, panel_rect.h / 2.0);
        let world_c = (center
            - egui::vec2(base_ox + self.view.pan.x, base_oy + self.view.pan.y))
            / (base_scale * old_zoom);
        self.view.zoom = new_zoom;
        self.view.pan = egui::vec2(
            center.x - base_ox - world_c.x * base_scale * new_zoom,
            center.y - base_oy - world_c.y * base_scale * new_zoom,
        );
    }

    pub fn nudge_selected_source(&mut self, dx: f32, dy: f32) {
        if self.expanded_source_id.is_some() {
            return;
        }
        let Some(uuid) = self.selected_source_id.clone() else {
            return;
        };
        if let Some(source) = self.cfg.canvas.sources.iter_mut().find(|s| s.uuid == uuid) {
            source.x += dx;
            source.y += dy;
            self.dirty = true;
        }
    }

    pub fn drag_source(&mut self, uuid: &str, delta: (f32, f32), panel_rect: &Rect) {
        let (scale, _, _) = self.display_transform(panel_rect);
        let snap_threshold = SNAP_THRESHOLD / scale;
        let break_threshold = SNAP_BREAK_THRESHOLD / scale;
        let candidates = self.snap_candidates(uuid);

        if let Some(source) = self.cfg.canvas.sources.iter_mut().find(|l| l.uuid == uuid) {
            let (dx, dy) = delta;
            let proposed_x = source.x + dx / scale;
            let proposed_y = source.y + dy / scale;

            let left = proposed_x;
            let right = proposed_x + source.width as f32;
            let top = proposed_y;
            let bottom = proposed_y + source.height as f32;

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

            let (x_offset, guide_x) = Self::resolve_axis_snap(snap_left, snap_right, left, right);
            let (y_offset, guide_y) = Self::resolve_axis_snap(snap_top, snap_bottom, top, bottom);

            source.x = (proposed_x + x_offset).round();
            source.y = (proposed_y + y_offset).round();
            self.snap_guides.x = if x_offset != 0.0 { Some(guide_x) } else { None };
            self.snap_guides.y = if y_offset != 0.0 { Some(guide_y) } else { None };
            self.dirty = true;
        }
    }

    pub fn source_rect_world(&self, uuid: &str) -> Option<WorldRect> {
        self.cfg
            .canvas
            .sources
            .iter()
            .find(|l| l.uuid == uuid)
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
        for source in &canvas.sources {
            if source.uuid == exclude_uuid {
                continue;
            }
            x.push(source.x);
            x.push(source.x + source.width as f32);
            y.push(source.y);
            y.push(source.y + source.height as f32);
        }
        SnapCandidates { x, y }
    }

    /// Offset and guide for one axis: pick the closer of the two snapped edges.
    fn resolve_axis_snap(
        snap_lo: Option<f32>,
        snap_hi: Option<f32>,
        lo: f32,
        hi: f32,
    ) -> (f32, f32) {
        return match (snap_lo, snap_hi) {
            (Some(l), Some(r)) => {
                if (l - lo).abs() < (r - hi).abs() {
                    (l - lo, l)
                } else {
                    (r - hi, r)
                }
            }
            (Some(l), None) => (l - lo, l),
            (None, Some(r)) => (r - hi, r),
            (None, None) => (0.0, 0.0),
        };
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

    pub fn hit_test(&self, panel_rect: &Rect, pos: (f32, f32)) -> Option<String> {
        let canvas = &self.cfg.canvas;
        let (scale, offset_x, offset_y) = self.display_transform(panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;

        let mut sources: Vec<_> = canvas.sources.iter().collect();
        sources.sort_by_key(|l| -l.z);

        let (px, py) = pos;
        for source in sources {
            let lx = cx + source.x * scale;
            let ly = cy + source.y * scale;
            let lw = source.width as f32 * scale;
            let lh = source.height as f32 * scale;
            if px >= lx && px <= lx + lw && py >= ly && py <= ly + lh {
                return Some(source.uuid.clone());
            }
        }
        None
    }

    pub fn hit_test_resize_handle(
        &self,
        panel_rect: &Rect,
        pos: (f32, f32),
    ) -> Option<(String, ResizeHandle)> {
        let uuid = self.selected_source_id.as_ref()?;
        let source = self.cfg.canvas.sources.iter().find(|l| &l.uuid == uuid)?;
        let (scale, offset_x, offset_y) = self.display_transform(panel_rect);
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;
        let lx = cx + source.x * scale;
        let ly = cy + source.y * scale;
        let lw = source.width as f32 * scale;
        let lh = source.height as f32 * scale;
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

    pub fn resize_source(
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

        if let Some(source) = self.cfg.canvas.sources.iter_mut().find(|l| l.uuid == uuid) {
            let (x, y, w, h, guide_x, guide_y) = match handle {
                ResizeHandle::Left => {
                    let (x, w, guide) = Self::resize_leading(
                        start.x,
                        start.w,
                        dx,
                        &candidates.x,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.x,
                    );
                    (x, start.y, w, start.h, guide, None)
                }
                ResizeHandle::Right => {
                    let (_, w, guide) = Self::resize_trailing(
                        start.x,
                        start.w,
                        dx,
                        &candidates.x,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.x,
                    );
                    (start.x, start.y, w, start.h, guide, None)
                }
                ResizeHandle::Top => {
                    let (y, h, guide) = Self::resize_leading(
                        start.y,
                        start.h,
                        dy,
                        &candidates.y,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.y,
                    );
                    (start.x, y, start.w, h, None, guide)
                }
                ResizeHandle::Bottom => {
                    let (_, h, guide) = Self::resize_trailing(
                        start.y,
                        start.h,
                        dy,
                        &candidates.y,
                        snap_threshold,
                        break_threshold,
                        self.snap_guides.y,
                    );
                    (start.x, start.y, start.w, h, None, guide)
                }
                _ => Self::resize_corner(
                    handle,
                    start,
                    dx,
                    dy,
                    &candidates,
                    snap_threshold,
                    break_threshold,
                    self.snap_guides,
                ),
            };

            source.x = x.round();
            source.y = y.round();
            source.width = w.max(1.0).round() as u32;
            source.height = h.max(1.0).round() as u32;
            self.snap_guides = SnapGuides {
                x: guide_x,
                y: guide_y,
            };
            self.dirty = true;
        }
    }

    /// Resize by moving the leading edge. Returns `(position, size, guide)`.
    fn resize_leading(
        start: f32,
        size: f32,
        delta: f32,
        candidates: &[f32],
        snap_threshold: f32,
        break_threshold: f32,
        current_guide: Option<f32>,
    ) -> (f32, f32, Option<f32>) {
        if let Some(snap) = Self::snap_value(
            start + delta,
            candidates,
            snap_threshold,
            break_threshold,
            current_guide,
        ) {
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

    /// Resize by moving the trailing edge. Returns `(position, size, guide)`;
    /// position is unchanged.
    fn resize_trailing(
        start: f32,
        size: f32,
        delta: f32,
        candidates: &[f32],
        snap_threshold: f32,
        break_threshold: f32,
        current_guide: Option<f32>,
    ) -> (f32, f32, Option<f32>) {
        if let Some(snap) = Self::snap_value(
            start + size + delta,
            candidates,
            snap_threshold,
            break_threshold,
            current_guide,
        ) {
            return (start, (snap - start).max(1.0), Some(snap));
        }
        return (start, (size + delta).max(1.0), None);
    }

    /// Corner resize: project the moving corner onto the fixed diagonal,
    /// preserving aspect. Returns `(x, y, w, h, guide_x, guide_y)`.
    #[allow(clippy::too_many_arguments)]
    fn resize_corner(
        handle: ResizeHandle,
        start: WorldRect,
        dx: f32,
        dy: f32,
        candidates: &SnapCandidates,
        snap_threshold: f32,
        break_threshold: f32,
        snap_guides: SnapGuides,
    ) -> (f32, f32, f32, f32, Option<f32>, Option<f32>) {
        let (fx, fy) = match handle {
            ResizeHandle::TopLeft => (start.x + start.w, start.y + start.h),
            ResizeHandle::TopRight => (start.x, start.y + start.h),
            ResizeHandle::BottomRight => (start.x, start.y),
            ResizeHandle::BottomLeft => (start.x + start.w, start.y),
            _ => unreachable!(),
        };
        let mx0 = start.x + start.w - (fx - start.x); // start moving corner x
        let my0 = start.y + start.h - (fy - start.y); // start moving corner y
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

        // Snap the moving corner to candidates, preferring the closer axis.
        let snap_mx = Self::snap_value(
            mx,
            &candidates.x,
            snap_threshold,
            break_threshold,
            snap_guides.x,
        );
        let snap_my = Self::snap_value(
            my,
            &candidates.y,
            snap_threshold,
            break_threshold,
            snap_guides.y,
        );
        let dist_x = snap_mx.map(|v| (v - mx).abs());
        let dist_y = snap_my.map(|v| (v - my).abs());
        let mut guide_x = None;
        let mut guide_y = None;
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
}
