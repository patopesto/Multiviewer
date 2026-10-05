use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};

use super::build::{clip, Frame, Placement};
use super::gpu::{conv_uniform, placeholder, solid_bind_group, solid_texture, texture_bind_group};
use super::{Compositor, ConvMode, LabelKey, Pipeline};
use crate::config::{Canvas, LabelPosition, LabelVisibility, Source, SourceLabelVisibility};

pub(super) struct LabelTex {
    _tex: wgpu::Texture,
    bg: Arc<wgpu::BindGroup>,
    w: u32,
    h: u32,
    key_hash: u64,
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
    pub(super) fn push_label(
        &mut self,
        source: &Source,
        place: &Placement,
        canvas: &Canvas,
        expanded: bool,
    ) {
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
        let label_key = LabelKey {
            name: source.name.clone(),
            size: label.size,
            text_color: label.text_color,
        };
        let (tex_w, tex_h, label_bg) = self.comp.ensure_label_texture(
            self.device,
            self.queue,
            source.uuid.clone(),
            &label_key,
        );

        let padding = 4.0 * self.scale;
        let bg_w = (tex_w as f32 * self.scale + padding * 2.0).min(place.lw);
        let bg_h = (tex_h as f32 * self.scale + padding * 2.0).min(place.lh);
        let (bg_x, bg_y) = label_origin(
            label.position,
            [place.lx, place.ly, place.lw, place.lh],
            (bg_w, bg_h),
        );

        let [bx0, by0] = clip(self.panel_rect, bg_x, bg_y);
        let [bx1, by1] = clip(self.panel_rect, bg_x + bg_w, bg_y + bg_h);
        let bg = self.comp.label_bg_bg.clone();
        self.quad([bx0, by0, bx1, by1], [0.0, 0.0, 1.0, 1.0], bg, Pipeline::Text);

        let tx_x = bg_x + padding;
        let tx_y = bg_y + padding;
        let [tx0, ty0] = clip(self.panel_rect, tx_x, tx_y);
        let [tx1, ty1] = clip(
            self.panel_rect,
            tx_x + tex_w as f32 * self.scale,
            tx_y + tex_h as f32 * self.scale,
        );
        self.quad([tx0, ty0, tx1, ty1], [0.0, 0.0, 1.0, 1.0], label_bg, Pipeline::Text);
    }
}

impl Compositor {
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
        let (tex, bg, w, h) = self.rasterize_label(device, queue, key);
        self.label_textures.insert(
            uuid,
            LabelTex {
                _tex: tex,
                bg: bg.clone(),
                w,
                h,
                key_hash,
            },
        );
        return (w, h, bg);
    }

    fn rasterize_label(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &LabelKey,
    ) -> (wgpu::Texture, Arc<wgpu::BindGroup>, u32, u32) {
        let Some((width, height, pixels)) = label_pixels(&self.font, key) else {
            let bg = Arc::new(placeholder(device, queue, &self.bind_layout, &self.sampler));
            let tex = solid_texture(device, "label-empty");
            return (tex, bg, 0, 0);
        };

        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("label"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
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

        let uniform = conv_uniform(
            device,
            queue,
            ConvMode::Passthrough,
            width as f32,
            height as f32,
        );
        let bg = Arc::new(texture_bind_group(
            device,
            &self.bind_layout,
            &self.sampler,
            &view,
            &uniform,
        ));
        return (tex, bg, width, height);
    }
}

/// Rasterize `key`'s glyphs into an RGBA buffer, or `None` when it is empty.
fn label_pixels(font: &fontdue::Font, key: &LabelKey) -> Option<(u32, u32, Vec<u8>)> {
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings::default());
    layout.append(&[font], &TextStyle::new(&key.name, key.size, 0));

    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return None;
    }

    let max_x = glyphs
        .iter()
        .map(|g| g.x + g.width as f32)
        .fold(0.0_f32, f32::max)
        .ceil() as i32;
    let min_y = glyphs.iter().map(|g| g.y.floor() as i32).min().unwrap_or(0);
    let max_y = glyphs
        .iter()
        .map(|g| (g.y + g.height as f32).ceil() as i32)
        .max()
        .unwrap_or(0);
    let width = max_x.max(1) as u32;
    let height = (max_y - min_y).max(1) as u32;

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let [tr, tg, tb, ta] = key.text_color;

    for glyph in glyphs {
        let (metrics, coverage) = font.rasterize_config(glyph.key);
        let gx = glyph.x as u32;
        let gy = (glyph.y as i32 - min_y) as u32;
        for row in 0..metrics.height as u32 {
            for col in 0..metrics.width as u32 {
                let src_idx = (row * metrics.width as u32 + col) as usize;
                let dst_x = gx + col;
                let dst_y = gy + row;
                if dst_x < width && dst_y < height {
                    let dst_idx = ((dst_y * width + dst_x) * 4) as usize;
                    let cov = coverage[src_idx];
                    // alpha-blend with existing pixel (simple over)
                    let src_a = ((cov as u32 * ta as u32) / 255) as u8;
                    let inv_dst_a = 255 - src_a;
                    pixels[dst_idx] = ((tr as u32 * src_a as u32 + pixels[dst_idx] as u32 * inv_dst_a as u32) / 255) as u8;
                    pixels[dst_idx + 1] = ((tg as u32 * src_a as u32 + pixels[dst_idx + 1] as u32 * inv_dst_a as u32) / 255) as u8;
                    pixels[dst_idx + 2] = ((tb as u32 * src_a as u32 + pixels[dst_idx + 2] as u32 * inv_dst_a as u32) / 255) as u8;
                    pixels[dst_idx + 3] = (src_a as u32 + (pixels[dst_idx + 3] as u32 * inv_dst_a as u32) / 255) as u8;
                }
            }
        }
    }
    return Some((width, height, pixels));
}
