/// CPU reference matching the old DeckLink shim fixed-point conversion.
fn ref_uyvy_to_rgb(u: u8, y: u8, v: u8, hd: bool) -> [u8; 3] {
    let (rc, gc_u, gc_v, bc) = if hd {
        (459, 55, 136, 541)
    } else {
        (409, 100, 208, 516)
    };
    let u = u as i32 - 128;
    let y = y as i32 - 16;
    let v = v as i32 - 128;
    let r = (298 * y + rc * v + 128) >> 8;
    let g = (298 * y - gc_u * u - gc_v * v + 128) >> 8;
    let b = (298 * y + bc * u + 128) >> 8;
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    [clamp(r), clamp(g), clamp(b)]
}

/// GPU shader equivalent (floating-point) for UYVY -> RGB.
fn shader_uyvy_to_rgb(u: u8, y: u8, v: u8, hd: bool) -> [u8; 3] {
    let uf = (u as f32 - 128.0) / 255.0;
    let yf = (y as f32 - 16.0) / 255.0;
    let vf = (v as f32 - 128.0) / 255.0;
    let (r, g, b) = if hd {
        (
            1.164 * yf + 1.793 * vf,
            1.164 * yf - 0.213 * uf - 0.533 * vf,
            1.164 * yf + 2.112 * uf,
        )
    } else {
        (
            1.164 * yf + 1.596 * vf,
            1.164 * yf - 0.391 * uf - 0.813 * vf,
            1.164 * yf + 2.018 * uf,
        )
    };
    let clamp = |v: f32| (v * 255.0).clamp(0.0, 255.0).round() as u8;
    [clamp(r), clamp(g), clamp(b)]
}

#[test]
fn uyvy_to_rgb_matches_cpu_reference() {
    let test_values: &[(u8, u8, u8)] = &[
        (128, 235, 128), // white
        (128, 16, 128),  // black
        (240, 180, 128), // yellow-ish
        (128, 168, 184), // cyan-ish
        (0, 81, 240),    // red-ish
        (0, 145, 54),    // green-ish
    ];
    for &(u, y, v) in test_values {
        for &hd in &[false, true] {
            let expected = ref_uyvy_to_rgb(u, y, v, hd);
            let actual = shader_uyvy_to_rgb(u, y, v, hd);
            assert!(
                expected
                    .iter()
                    .zip(&actual)
                    .all(|(e, a)| e.abs_diff(*a) <= 1),
                "mismatch for U={u} Y={y} V={v} hd={hd}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

#[test]
fn source_layout_matches_each_format() {
    use super::layout::source_layout;
    use super::ConvMode;
    use crate::sources::PixelFormat;
    use wgpu::TextureFormat;

    assert_eq!(
        source_layout(PixelFormat::Bgra8, 1280, 720),
        (TextureFormat::Bgra8Unorm, 1280, 720, 4, ConvMode::Passthrough)
    );
    // 4:2:2 packs two pixels per texel: half width, so Rgba8 rows are w*2 bytes.
    assert_eq!(
        source_layout(PixelFormat::Uyvy422, 1280, 720),
        (TextureFormat::Rgba8Unorm, 640, 720, 2, ConvMode::UyvyBt709)
    );
    assert_eq!(
        source_layout(PixelFormat::Yuy2, 640, 480),
        (TextureFormat::Rgba8Unorm, 320, 480, 2, ConvMode::Yuy2Bt601)
    );
    // NV12 keeps full width; the texture is 1.5x tall (Y + interleaved UV).
    assert_eq!(
        source_layout(PixelFormat::Nv12, 1920, 1080),
        (TextureFormat::R8Unorm, 1920, 1620, 1, ConvMode::Nv12Bt709)
    );
}

/// The WGSL is normally only parsed/validated when the app starts; this
/// catches syntax/semantic errors without a GPU device.
#[test]
fn shader_parses_and_validates() {
    let module =
        wgpu::naga::front::wgsl::parse_str(super::gpu::SHADER).expect("WGSL failed to parse");
    let mut validator = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .expect("WGSL failed validation");
}

/// UYVY and YUY2 produce the same texture format/dimensions, so `mode` must
/// be part of the texture staleness key — otherwise switching between them
/// reuses the wrong shader decode (YUY2 bytes decoded as UYVY gives the
/// green/pink artefact).
#[test]
fn uyvy_and_yuy2_share_layout_but_differ_in_mode() {
    use super::layout::source_layout;
    use crate::sources::PixelFormat;

    let (f1, w1, h1, _, m1) = source_layout(PixelFormat::Uyvy422, 1280, 720);
    let (f2, w2, h2, _, m2) = source_layout(PixelFormat::Yuy2, 1280, 720);
    assert_eq!((f1, w1, h1), (f2, w2, h2));
    assert_ne!(m1, m2);
}

#[test]
fn source_uv_fits_within_placement() {
    use super::build::source_uv;
    use crate::config::TextureMode;

    // Wide source in a square placement: full width, scaled down vertically, full UVs.
    assert_eq!(
        source_uv(TextureMode::Fit, 2.0, 1.0),
        (1.0, 0.5, [0.0, 0.0, 1.0, 1.0])
    );
    // Tall source: scaled down horizontally.
    assert_eq!(
        source_uv(TextureMode::Fit, 1.0, 2.0),
        (0.5, 1.0, [0.0, 0.0, 1.0, 1.0])
    );
    assert_eq!(
        source_uv(TextureMode::Stretch, 2.0, 1.0),
        (1.0, 1.0, [0.0, 0.0, 1.0, 1.0])
    );
}

#[test]
fn source_uv_fill_crops_the_long_axis() {
    use super::build::source_uv;
    use crate::config::TextureMode;

    // Wide source in a square placement: crop horizontally, keep vertical UVs.
    assert_eq!(
        source_uv(TextureMode::Fill, 2.0, 1.0),
        (1.0, 1.0, [0.25, 0.0, 0.75, 1.0])
    );
    // Tall source: crop vertically.
    assert_eq!(
        source_uv(TextureMode::Fill, 1.0, 2.0),
        (1.0, 1.0, [0.0, 0.25, 1.0, 0.75])
    );
}

#[test]
fn placement_maps_canvas_rect_to_clip_space() {
    use super::build::Placement;
    use crate::config::{Source, TextureMode};
    use crate::sources::Protocol;

    let panel = super::Rect {
        x: 0.0,
        y: 0.0,
        w: 640.0,
        h: 360.0,
    };
    let source = Source::new(
        "cam".into(),
        Protocol::Test,
        None,
        100.0,
        50.0,
        200,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    );

    let place = Placement::compute(&source, false, (0.0, 0.0, 640.0, 360.0), 1.0, &panel);
    assert_eq!((place.lx, place.ly, place.lw, place.lh), (100.0, 50.0, 200.0, 100.0));
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(close(place.x0, -0.6875) && close(place.x1, -0.0625));
    assert!(close(place.y0, 0.722_222_2) && close(place.y1, 0.166_666_67));

    // Expanded sources fill the whole canvas regardless of their rect.
    let expanded = Placement::compute(&source, true, (0.0, 0.0, 640.0, 360.0), 1.0, &panel);
    assert!(close(expanded.x0, -1.0) && close(expanded.y0, 1.0));
    assert!(close(expanded.x1, 1.0) && close(expanded.y1, -1.0));
}

#[test]
fn label_origin_places_background_for_each_corner() {
    use super::label::label_origin;
    use crate::config::LabelPosition;

    let rect = [10.0, 20.0, 200.0, 100.0];
    let bg = (40.0, 20.0);
    assert_eq!(label_origin(LabelPosition::TopLeft, rect, bg), (10.0, 20.0));
    assert_eq!(label_origin(LabelPosition::TopRight, rect, bg), (170.0, 20.0));
    assert_eq!(label_origin(LabelPosition::BottomLeft, rect, bg), (10.0, 100.0));
    assert_eq!(label_origin(LabelPosition::BottomRight, rect, bg), (170.0, 100.0));
    assert_eq!(label_origin(LabelPosition::Center, rect, bg), (90.0, 60.0));
}
