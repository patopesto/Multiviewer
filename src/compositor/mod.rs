use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::sources::SourceKey;

pub use layout::canvas_transform;

mod build;
mod canvas;
mod gpu;
mod label;
mod layout;
mod resources;
#[cfg(test)]
mod tests;

/// A simple rectangle, replacing the previous egui::Rect dependency.
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.w
    }
    pub fn height(&self) -> f32 {
        self.h
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vert {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvMode {
    Passthrough = 0,
    UyvyBt601 = 1,
    UyvyBt709 = 2,
    Yuy2Bt601 = 3,
    Yuy2Bt709 = 4,
    Nv12Bt601 = 5,
    Nv12Bt709 = 6,
}

/// Uniform block consumed by the compositor's fragment shader.
/// Must stay in sync with the `ConvUniform` struct inside `SHADER`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ConvUniform {
    pub mode: u32, // ConvMode
    pub width: f32,
    pub height: f32,
    pub _pad: f32,
}


/// Shared with the paint callback; immutable after creation.
pub struct Shared {
    pub pipeline: wgpu::RenderPipeline,
    pub text_pipeline: wgpu::RenderPipeline,
    pub placeholder_bg: Arc<wgpu::BindGroup>,
    pub vb: wgpu::Buffer,
    pub ib: wgpu::Buffer,
}

#[derive(Clone, Debug)]
struct LabelKey {
    name: String,
    size: f32,
    text_color: [u8; 4],
}

impl Hash for LabelKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.size.to_bits().hash(state);
        self.text_color.hash(state);
    }
}

/// Per-source draw state resolved once per frame: (bind group, source aspect, flip_h, flip_v).
type ResolvedSource = Option<(Arc<wgpu::BindGroup>, f32, bool, bool)>;

pub struct Compositor {
    pub shared: Arc<Shared>,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<SourceKey, resources::SourceTex>,
    pulls: HashMap<SourceKey, ResolvedSource>, // Resolved sources for the current frame
    canvas_texture: Option<wgpu::Texture>,
    canvas_view: Option<wgpu::TextureView>,
    canvas_vb: wgpu::Buffer,
    canvas_pipeline: wgpu::RenderPipeline,
    canvas_w: u32,
    canvas_h: u32,
    border_bg: Arc<wgpu::BindGroup>,
    border_color: [u8; 4],
    font: fontdue::Font,
    label_textures: HashMap<String, label::LabelTex>,
    label_bg_color: [u8; 4],
    label_bg_bg: Arc<wgpu::BindGroup>,
    text_pipeline: wgpu::RenderPipeline,
}

pub enum Pipeline {
    Main,
    Text,
}

pub struct DrawCall {
    pub first_index: u32,
    pub bind_group: Arc<wgpu::BindGroup>,
    pub pipeline: Pipeline,
}

pub struct Draw {
    pub verts: Arc<Vec<Vert>>,
    pub draws: Vec<DrawCall>,
}
