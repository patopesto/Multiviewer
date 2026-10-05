use super::{ConvMode, Rect};
use crate::config::Canvas;
use crate::sources::PixelFormat;

/// Texture layout for a CPU frame
pub(super) fn source_layout(
    fmt: PixelFormat,
    w: u32,
    h: u32,
) -> (wgpu::TextureFormat, u32, u32, u32, ConvMode) {
    let hd = h > 576;
    return match fmt {
        PixelFormat::Rgba8 => (wgpu::TextureFormat::Rgba8Unorm, w, h, 4, ConvMode::Passthrough),
        PixelFormat::Bgra8 => (wgpu::TextureFormat::Bgra8Unorm, w, h, 4, ConvMode::Passthrough),
        // 4:2:2 packed as Rgba8 at half width (two pixels per texel).
        PixelFormat::Uyvy422 => {
            let mode = if hd { ConvMode::UyvyBt709 } else { ConvMode::UyvyBt601 };
            (wgpu::TextureFormat::Rgba8Unorm, w / 2, h, 2, mode)
        }
        PixelFormat::Yuy2 => {
            let mode = if hd { ConvMode::Yuy2Bt709 } else { ConvMode::Yuy2Bt601 };
            (wgpu::TextureFormat::Rgba8Unorm, w / 2, h, 2, mode)
        }
        // 4:2:0: one R8 texture, Y plane then interleaved UV plane (height 1.5h).
        PixelFormat::Nv12 => {
            let mode = if hd { ConvMode::Nv12Bt709 } else { ConvMode::Nv12Bt601 };
            (wgpu::TextureFormat::R8Unorm, w, h + h / 2, 1, mode)
        }
    };
}

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

