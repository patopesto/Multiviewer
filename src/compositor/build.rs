use std::sync::Arc;
use std::time::Instant;

use tracing::instrument;

use super::gpu::solid_bind_group;
use super::{Compositor, Draw, DrawCall, Pipeline, Rect, ResolvedSource, Vert};
use crate::config::{BorderVisibility, Canvas, Source, SourceBorderVisibility, TextureMode};
use crate::sources::{Frame as SourceFrame, SourceKey, SourceRegistry};

/// Resolved geometry for one source this frame, in canvas and clip space.
/// `(x0, y0)` is the top-left clip corner, `(x1, y1)` the bottom-right.
pub(super) struct Placement {
    pub lx: f32,
    pub ly: f32,
    pub lw: f32,
    pub lh: f32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Placement {
    pub(super) fn compute(
        source: &Source,
        expanded: bool,
        canvas: (f32, f32, f32, f32),
        scale: f32,
        panel: &Rect,
    ) -> Self {
        let (cx, cy, cw, ch) = canvas;
        let (lx, ly, lw, lh) = if expanded {
            (cx, cy, cw, ch)
        } else {
            (
                cx + source.x * scale,
                cy + source.y * scale,
                source.width as f32 * scale,
                source.height as f32 * scale,
            )
        };
        let [x0, y0] = clip(panel, lx, ly);
        let [x1, y1] = clip(panel, lx + lw, ly + lh);
        return Self {
            lx,
            ly,
            lw,
            lh,
            x0,
            y0,
            x1,
            y1,
        };
    }
}

/// Canvas point -> normalized clip coords.
pub(super) fn clip(panel: &Rect, x: f32, y: f32) -> [f32; 2] {
    return [
        (x - panel.x) / panel.width() * 2.0 - 1.0,
        1.0 - (y - panel.y) / panel.height() * 2.0,
    ];
}

/// Texture scale `(sx, sy)` and UV rect `[u0, v0, u1, v1]` for a `TextureMode`,
/// before per-source flips are applied.
pub(super) fn source_uv(mode: TextureMode, aspect: f32, source_aspect: f32) -> (f32, f32, [f32; 4]) {
    return match mode {
        TextureMode::Fit => {
            if aspect > source_aspect {
                (1.0, source_aspect / aspect, [0.0, 0.0, 1.0, 1.0])
            } else {
                (aspect / source_aspect, 1.0, [0.0, 0.0, 1.0, 1.0])
            }
        }
        TextureMode::Fill => {
            if aspect > source_aspect {
                let u = source_aspect / aspect;
                (1.0, 1.0, [0.5 - u / 2.0, 0.0, 0.5 + u / 2.0, 1.0])
            } else {
                let v = aspect / source_aspect;
                (1.0, 1.0, [0.0, 0.5 - v / 2.0, 1.0, 0.5 + v / 2.0])
            }
        }
        TextureMode::Stretch => (1.0, 1.0, [0.0, 0.0, 1.0, 1.0]),
    };
}

/// Accumulates vertices and draw calls while `build` walks the canvas.
pub(super) struct Frame<'a> {
    pub comp: &'a mut Compositor,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub panel_rect: &'a Rect,
    pub scale: f32,
    registry: &'a SourceRegistry,
    verts: Vec<Vert>,
    draws: Vec<DrawCall>,
    next_index: u32,
}

impl<'a> Frame<'a> {
    fn new(
        comp: &'a mut Compositor,
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        registry: &'a SourceRegistry,
        panel_rect: &'a Rect,
        scale: f32,
        source_count: usize,
    ) -> Self {
        return Self {
            comp,
            device,
            queue,
            panel_rect,
            scale,
            registry,
            // One content quad + four border edges + label background + label text.
            verts: Vec::with_capacity(source_count * 7 * 4),
            draws: Vec::with_capacity(source_count * 7),
            next_index: 0,
        };
    }

    /// Push one axis-aligned textured quad and its draw call.
    pub(super) fn quad(
        &mut self,
        rect: [f32; 4],
        uv: [f32; 4],
        bg: Arc<wgpu::BindGroup>,
        pipeline: Pipeline,
    ) {
        let [x0, y0, x1, y1] = rect;
        let [u0, v0, u1, v1] = uv;
        self.verts.extend_from_slice(&[
            Vert {
                pos: [x0, y0],
                uv: [u0, v0],
            },
            Vert {
                pos: [x1, y0],
                uv: [u1, v0],
            },
            Vert {
                pos: [x1, y1],
                uv: [u1, v1],
            },
            Vert {
                pos: [x0, y1],
                uv: [u0, v1],
            },
        ]);
        self.draws.push(DrawCall {
            first_index: self.next_index,
            bind_group: bg,
            pipeline,
        });
        self.next_index += 6;
    }

    /// Resolve the source's frame and emit its content quad.
    fn push_source(&mut self, source: &Source, place: &Placement) {
        let entry = source.source_ref.as_deref().and_then(|source_ref| {
            let key = SourceKey::new(source.protocol.clone(), source_ref.to_string());
            self.comp
                .resolve_source(&key, self.registry, self.device, self.queue)
        });
        let (bg, aspect, src_flip_h, src_flip_v) = match entry {
            Some(e) => e,
            None => (self.comp.shared.placeholder_bg.clone(), 16.0 / 9.0, false, false),
        };
        let flip_h = src_flip_h ^ source.flip_h;
        let flip_v = src_flip_v ^ source.flip_v;

        let source_aspect = if place.lh > 0.0 { place.lw / place.lh } else { 1.0 };
        let (sx, sy, [u0, v0, u1, v1]) = source_uv(source.mode, aspect, source_aspect);
        let (u0, u1) = if flip_h { (u1, u0) } else { (u0, u1) };
        let (v0, v1) = if flip_v { (v1, v0) } else { (v0, v1) };

        let cx = (place.x0 + place.x1) / 2.0;
        let cy = (place.y0 + place.y1) / 2.0;
        let hx = (place.x1 - place.x0) / 2.0 * sx;
        let hy = (place.y0 - place.y1) / 2.0 * sy;
        self.quad(
            [cx - hx, cy + hy, cx + hx, cy - hy],
            [u0, v0, u1, v1],
            bg,
            Pipeline::Main,
        );
    }

    /// Emit the four inside-edge quads when this source shows a border.
    fn push_border(&mut self, source: &Source, place: &Placement, canvas: &Canvas, expanded: bool) {
        if expanded {
            return;
        }
        let visible = match source.border_visibility {
            SourceBorderVisibility::Show => true,
            SourceBorderVisibility::Hide => false,
            SourceBorderVisibility::Inherit => {
                canvas.border.visibility == BorderVisibility::Show
            }
        };
        if !visible {
            return;
        }

        let dx = 2.0 * canvas.border.width / self.panel_rect.width();
        let dy = 2.0 * canvas.border.width / self.panel_rect.height();
        let (x0, y0, x1, y1) = (place.x0, place.y0, place.x1, place.y1);
        let edges = [
            [x0, y0, x1, y0 - dy], // top
            [x0, y1 + dy, x1, y1], // bottom
            [x0, y0, x0 + dx, y1], // left
            [x1 - dx, y0, x1, y1], // right
        ];
        let bg = self.comp.border_bg.clone();
        for rect in edges {
            self.quad(rect, [0.0, 0.0, 1.0, 1.0], bg.clone(), Pipeline::Main);
        }
    }

    fn finish(self) -> Draw {
        return Draw {
            verts: Arc::new(self.verts),
            draws: self.draws,
        };
    }
}

impl Compositor {
    /// Start a new frame: drop the resolved-source cache so the next build pulls each source again.
    pub fn begin_frame(&mut self) {
        self.pulls.clear();
    }

    /// Pull every source this frame will need before any render runs
    #[instrument(level = "debug", skip_all)]
    pub fn prefetch_sources(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &SourceRegistry,
        canvas: &Canvas,
        expanded_source: Option<&str>,
    ) {
        for source in &canvas.sources {
            if expanded_source.is_some() && expanded_source != Some(source.uuid.as_str()) {
                continue;
            }
            if let Some(source_ref) = source.source_ref.as_deref() {
                let key = SourceKey::new(source.protocol.clone(), source_ref.to_string());
                self.resolve_source(&key, registry, device, queue);
            }
        }
    }

    /// Pull a source once per frame.
    fn resolve_source(
        &mut self,
        key: &SourceKey,
        registry: &SourceRegistry,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> ResolvedSource {
        if let Some(cached) = self.pulls.get(key) {
            return cached.clone();
        }
        let resolved: ResolvedSource = (|| {
            let src = registry.get(key)?;
            let stats = src.stats();
            let latest_span = tracing::debug_span!("latest", source = %key.source_ref);
            let _latest_guard = latest_span.entered();
            let t = Instant::now();
            let frame = src.latest(device, queue)?;
            let receive_ms = t.elapsed().as_secs_f32() * 1000.0;
            {
                let mut s = stats.lock().unwrap();
                s.record_receive_time(receive_ms);
                s.record_consumed(frame.seq());
            }
            match frame {
                SourceFrame::Cpu(f) => {
                    let st = self.ensure_texture(device, queue, key, &f, Some(stats));
                    Some((st.bg.clone(), f.w as f32 / f.h as f32, false, false))
                }
                SourceFrame::Gpu(f) => {
                    Some((f.bg.clone(), f.w as f32 / f.h as f32, false, f.flip_v))
                }
            }
        })();
        self.pulls.insert(key.clone(), resolved.clone());
        return resolved;
    }

    /// Per-frame: upload changed source textures, build quads for all sources.
    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        registry: &SourceRegistry,
        panel_rect: &Rect,
        transform: (f32, f32, f32),
        expanded_source: Option<&str>,
    ) -> Draw {
        let (scale, offset_x, offset_y) = transform;
        let cx = panel_rect.x + offset_x;
        let cy = panel_rect.y + offset_y;
        let cw = canvas.width as f32 * scale;
        let ch = canvas.height as f32 * scale;

        if self.border_color != canvas.border.color {
            self.border_color = canvas.border.color;
            self.border_bg = Arc::new(solid_bind_group(
                device,
                queue,
                &self.bind_layout,
                &self.sampler,
                self.border_color,
            ));
        }

        let mut sources: Vec<_> = canvas.sources.iter().collect();
        sources.sort_by_key(|l| l.z);

        let mut frame = Frame::new(
            self,
            device,
            queue,
            registry,
            panel_rect,
            scale,
            sources.len(),
        );
        for source in sources {
            let expanded = expanded_source == Some(source.uuid.as_str());
            if expanded_source.is_some() && !expanded {
                continue;
            }
            let place = Placement::compute(source, expanded, (cx, cy, cw, ch), scale, panel_rect);
            frame.push_source(source, &place);
            frame.push_border(source, &place, canvas, expanded);
            frame.push_label(source, &place, canvas, expanded);
        }
        return frame.finish();
    }
}
