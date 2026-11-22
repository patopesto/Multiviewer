use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::build::{clip, Frame, Placement};
use super::gpu::{conv_uniform, placeholder, solid_bind_group, solid_texture, texture_bind_group};
use super::{Compositor, ConvMode, LabelKey, Pipeline};
use crate::config::{Canvas, LabelPosition, LabelVisibility, Source, SourceLabelVisibility};
use crate::sources::{rasterize_text, PixelFormat, TextureTarget, UPLOAD_TARGET_USAGE};

pub(super) struct LabelTex {
    _target: TextureTarget,
    bg: Arc<wgpu::BindGroup>,
    w: u32,
    h: u32,
    key_hash: u64,
}

const LABEL_MIN_RASTER: f32 = 0.25;
const LABEL_MAX_RASTER: f32 = 8.0;
/// Cap on the rasterized em size, so maximum zoom cannot ask for an oversized texture.
const LABEL_MAX_EM: f32 = 256.0;

/// Raster pixels per canvas pixel for a label drawn at `display_scale`,
/// bucketed to a power of two so zooming re-rasterizes on crossings only.
fn raster_scale(display_scale: f32, size: f32) -> f32 {
    if !display_scale.is_finite() || display_scale <= 0.0 {
        return 1.0;
    }
    let cap = (LABEL_MAX_EM / size.max(1.0)).clamp(1.0, LABEL_MAX_RASTER);
    let bucketed = 2.0_f32.powf(display_scale.min(cap).log2().ceil());
    return bucketed.clamp(LABEL_MIN_RASTER, cap);
}

/// Top-left corner of the label background within a source rect, for a `LabelPosition`.
pub(super) fn label_origin(pos: LabelPosition, rect: [f32; 4], bg: (f32, f32)) -> (f32, f32) {
    let [lx, ly, lw, lh] = rect;
    let (bg_w, bg_h) = bg;
    return match pos {
        LabelPosition::TopLeft => (lx, ly),
        LabelPosition::TopCenter => (lx + (lw - bg_w) / 2.0, ly),
        LabelPosition::TopRight => (lx + lw - bg_w, ly),
        LabelPosition::CenterLeft => (lx, ly + (lh - bg_h) / 2.0),
        LabelPosition::Center => (lx + (lw - bg_w) / 2.0, ly + (lh - bg_h) / 2.0),
        LabelPosition::CenterRight => (lx + lw - bg_w, ly + (lh - bg_h) / 2.0),
        LabelPosition::BottomLeft => (lx, ly + lh - bg_h),
        LabelPosition::BottomCenter => (lx + (lw - bg_w) / 2.0, ly + lh - bg_h),
        LabelPosition::BottomRight => (lx + lw - bg_w, ly + lh - bg_h),
    };
}

impl Frame<'_> {
    /// Emit the label background and text quads when this source shows a label.
    pub(super) fn push_label(&mut self, source: &Source, place: &Placement, canvas: &Canvas, expanded: bool) {
        if expanded {
            return;
        }
        let visible = match source.label_visibility {
            SourceLabelVisibility::Show => true,
            SourceLabelVisibility::Hide => false,
            SourceLabelVisibility::Inherit => canvas.label.visibility == LabelVisibility::Show,
        };
        if !visible || source.name.is_empty() {
            return;
        }

        if self.comp.label_bg_color != canvas.label.background_color {
            self.comp.label_bg_color = canvas.label.background_color;
            self.comp.label_bg_bg = Arc::new(solid_bind_group(
                self.device,
                self.queue,
                &self.comp.bind_layout,
                &self.comp.sampler,
                self.comp.label_bg_color,
            ));
        }

        let label = &canvas.label;
        let raster = raster_scale(self.label_scale, label.size);
        let label_key = LabelKey {
            name: source.name.clone(),
            size: label.size,
            raster,
            text_color: label.text_color,
        };
        let (tex_w, tex_h, label_bg) = self.comp.ensure_label_texture(self.device, self.queue, source.uuid.clone(), &label_key);

        let text_w = tex_w as f32 * self.scale / raster;
        let text_h = tex_h as f32 * self.scale / raster;
        let padding = 4.0 * self.scale;
        let bg_w = (text_w + padding * 2.0).min(place.lw);
        let bg_h = (text_h + padding * 2.0).min(place.lh);
        let (bg_x, bg_y) = label_origin(label.position, [place.lx, place.ly, place.lw, place.lh], (bg_w, bg_h));

        let [bx0, by0] = clip(self.panel_rect, bg_x, bg_y);
        let [bx1, by1] = clip(self.panel_rect, bg_x + bg_w, bg_y + bg_h);
        let bg = self.comp.label_bg_bg.clone();
        self.quad([bx0, by0, bx1, by1], [0.0, 0.0, 1.0, 1.0], bg, Pipeline::Text);

        let tx_x = bg_x + padding;
        let tx_y = bg_y + padding;
        let [tx0, ty0] = clip(self.panel_rect, tx_x, tx_y);
        let [tx1, ty1] = clip(self.panel_rect, tx_x + text_w, tx_y + text_h);
        self.quad([tx0, ty0, tx1, ty1], [0.0, 0.0, 1.0, 1.0], label_bg, Pipeline::Text);
    }
}

impl Compositor {
    /// Drop the cached label of a placement that left the canvas.
    pub fn forget_placement(&mut self, uuid: &str) {
        self.label_textures.remove(uuid);
    }

    /// Cached label texture for `uuid`; re-rasterizes when the key hash changes.
    pub(super) fn ensure_label_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uuid: String,
        key: &LabelKey,
    ) -> (u32, u32, Arc<wgpu::BindGroup>) {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let key_hash = hasher.finish();

        if let Some(t) = self.label_textures.get(&uuid).filter(|t| t.key_hash == key_hash) {
            return (t.w, t.h, t.bg.clone());
        }
        let (target, bg, w, h) = self.rasterize_label(device, queue, key);
        self.label_textures.insert(
            uuid,
            LabelTex {
                _target: target,
                bg: bg.clone(),
                w,
                h,
                key_hash,
            },
        );
        return (w, h, bg);
    }

    fn rasterize_label(&self, device: &wgpu::Device, queue: &wgpu::Queue, key: &LabelKey) -> (TextureTarget, Arc<wgpu::BindGroup>, u32, u32) {
        let Some((width, height, pixels)) = rasterize_text(&key.name, key.size * key.raster, key.text_color) else {
            let bg = Arc::new(placeholder(device, queue, &self.bind_layout, &self.sampler));
            let texture = solid_texture(device, "label-empty");
            let view = texture.create_view(&Default::default());
            let layout = PixelFormat::Rgba8.texture_layout(1, 1);
            let target = TextureTarget::from_parts(texture, view, layout, UPLOAD_TARGET_USAGE);
            return (target, bg, 0, 0);
        };

        let layout = PixelFormat::Rgba8.texture_layout(width, height);
        let mut target = None;
        TextureTarget::ensure(&mut target, device, layout, UPLOAD_TARGET_USAGE, "label");
        let target = target.unwrap();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let uniform = conv_uniform(device, queue, ConvMode::Passthrough, width as f32, height as f32);
        let bg = Arc::new(texture_bind_group(device, &self.bind_layout, &self.sampler, &target.view, &uniform));
        return (target, bg, width, height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_scale_buckets_to_powers_of_two() {
        assert_eq!(raster_scale(1.0, 24.0), 1.0);
        assert_eq!(raster_scale(1.01, 24.0), 2.0);
        assert_eq!(raster_scale(2.0, 24.0), 2.0);
        assert_eq!(raster_scale(2.01, 24.0), 4.0);
        assert_eq!(raster_scale(0.25, 24.0), 0.25);
        assert_eq!(raster_scale(0.0, 24.0), 1.0);
    }

    /// A label at maximum zoom must not ask the GPU for a bigger em than the cap.
    #[test]
    fn raster_scale_caps_the_rasterized_em_size() {
        assert_eq!(raster_scale(8.0, 64.0), 4.0);
        assert!(raster_scale(64.0, 24.0) * 24.0 <= LABEL_MAX_EM);
        assert!(raster_scale(64.0, 256.0) * 256.0 <= LABEL_MAX_EM);
    }
}
