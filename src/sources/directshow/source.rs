use std::ffi::c_void;
use std::mem::{size_of, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use windows::Win32::Foundation::{FALSE, RPC_E_CHANGED_MODE, TRUE};
use windows::Win32::Media::DirectShow::{
    IAMStreamConfig, IBaseFilter, ICaptureGraphBuilder2, IFilterGraph2, IMediaControl,
    VIDEO_STREAM_CONFIG_CAPS,
};
use windows::Win32::Media::MediaFoundation::{
    AM_MEDIA_TYPE, CLSID_CaptureGraphBuilder2, CLSID_FilterGraph, FORMAT_VideoInfo,
    FORMAT_VideoInfo2, MEDIASUBTYPE_MJPG, MEDIASUBTYPE_NV12, MEDIASUBTYPE_RGB24,
    MEDIASUBTYPE_RGB32, MEDIASUBTYPE_UYVY, MEDIASUBTYPE_YUY2, MEDIASUBTYPE_v210, MEDIATYPE_Video,
    PIN_CATEGORY_CAPTURE, VIDEOINFOHEADER, VIDEOINFOHEADER2,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::core::{w, GUID, Interface};

use super::super::{Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};
use super::discovery::find_capture_filter;
use super::ffi::{Grabber, ISampleGrabber, ISampleGrabberCB, CLSID_NULL_RENDERER, CLSID_SAMPLE_GRABBER};

/// One mode the capture device advertises (from `IAMStreamConfig::GetStreamCaps`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CaptureMode {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl CaptureMode {
    /// Human-readable `WxH @ fps` label (`fps` shown as a fraction when non-integral).
    pub fn label(&self) -> String {
        let fps = if self.fps_num == 0 || self.fps_den == 0 {
            "?".to_string()
        } else if self.fps_den == 1 {
            format!("{}", self.fps_num)
        } else {
            format!("{:.2}", self.fps_num as f64 / self.fps_den as f64)
        };
        return format!("{}x{} @ {}", self.width, self.height, fps);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DirectShowSourceConfig {
    /// Device-interface path of the capture device; empty uses the first found.
    pub device_id: String,
    /// Requested frame width in pixels; 0 lets the device pick its default.
    #[serde(default)]
    pub width: u32,
    /// Requested frame height in pixels; 0 lets the device pick its default.
    #[serde(default)]
    pub height: u32,
    /// Requested frame-rate numerator; 0 lets the device pick its default.
    #[serde(default)]
    pub fps_num: u32,
    /// Requested frame-rate denominator (stored exactly, e.g. 1001 for 29.97).
    #[serde(default)]
    pub fps_den: u32,
    /// Requested output pixel format; `None` = Auto (RGB32).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_format: Option<PixelFormat>,
}

/// Negotiated frame geometry and pixel layout, read once from the connected
/// SampleGrabber media type.
struct ConnectedFormat {
    width: u32,
    height: u32,
    pitch: u32,
    rows: u32,
    pixel_format: PixelFormat,
    flip: bool,
    nominal_fps: f64,
}

/// The capture graph kept alive for the source's lifetime.
struct Graph {
    _graph: IFilterGraph2,
    media_control: IMediaControl,
    grabber: ISampleGrabber,
    _capture: IBaseFilter,
    _grabber_filter: IBaseFilter,
    _null_renderer: IBaseFilter,
}

/// Active DirectShow capture source on its own thread.
pub struct DirectShowSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DirectShowSource {
    /// `modes` is filled with the device's advertised modes on open; it is read
    /// by the settings UI.
    pub fn spawn(
        source_ref: SourceRef,
        cfg: &DirectShowSourceConfig,
        modes: Arc<Mutex<Vec<CaptureMode>>>,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let running2 = running.clone();
        let trace_ref = source_ref.clone();
        let config = cfg.clone();

        let thread = std::thread::Builder::new()
            .name(format!("directshow-in-{source_ref}"))
            .spawn(move || {
                run_capture(trace_ref, config, slot2, stats2, running2, modes);
            })
            .expect("spawn directshow capture thread");

        Self {
            source_ref,
            slot,
            stats,
            running,
            thread: Some(thread),
        }
    }
}

impl Drop for DirectShowSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source = self.source_ref, "DirectShow thread join failed: {:?}", e);
        }
    }
}

impl VideoSource for DirectShowSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}

/// The full capture loop; owns the thread's COM apartment for its lifetime.
fn run_capture(
    source_ref: SourceRef,
    cfg: DirectShowSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    modes: Arc<Mutex<Vec<CaptureMode>>>,
) {
    tracing::info!(source = source_ref, device_id = cfg.device_id, "DirectShow capture thread starting");
    unsafe {
        let coinit = CoInitializeEx(None, COINIT_MULTITHREADED);
        if coinit.is_err() && coinit != RPC_E_CHANGED_MODE {
            tracing::error!(source = source_ref, "DirectShow: CoInitializeEx failed");
            return;
        }
    }

    let (graph, fmt, mode_list) = match unsafe { build_graph(&source_ref, &cfg) } {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(source = source_ref, "DirectShow open failed: {e}");
            unsafe { CoUninitialize() };
            return;
        }
    };
    tracing::info!(
        source = source_ref,
        "DirectShow opened: {}x{} {} pitch={} flip={}",
        fmt.width,
        fmt.height,
        fmt.pixel_format.label(),
        fmt.pitch,
        fmt.flip
    );
    *modes.lock().unwrap() = mode_list;

    let callback: ISampleGrabberCB = Grabber {
        slot,
        stats,
        pool: Mutex::new(FramePool::new()),
        seq: std::sync::atomic::AtomicU64::new(0),
        width: fmt.width,
        height: fmt.height,
        pitch: fmt.pitch,
        rows: fmt.rows,
        pixel_format: fmt.pixel_format,
        flip: fmt.flip,
        nominal_fps: fmt.nominal_fps,
    }
    .into();

    if let Err(e) = unsafe { graph.grabber.SetCallback(Interface::as_raw(&callback), 1) } {
        tracing::error!(source = source_ref, "DirectShow SetCallback failed: {e}");
        drop(callback);
        unsafe { CoUninitialize() };
        return;
    }
    if let Err(e) = unsafe { graph.media_control.Run() } {
        tracing::error!(source = source_ref, "DirectShow Run failed: {e}");
        drop(callback);
        unsafe { CoUninitialize() };
        return;
    }

    while running.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = unsafe { graph.media_control.Stop() };
    // The graph holds the callback's raw pointer; stop and drop the graph
    // before releasing our owning reference.
    drop(graph);
    drop(callback);
    unsafe { CoUninitialize() };
}

/// Build and start the capture graph, returning it with the negotiated format
/// and the device's advertised modes.
unsafe fn build_graph(
    source_ref: &str,
    cfg: &DirectShowSourceConfig,
) -> Result<(Graph, ConnectedFormat, Vec<CaptureMode>), String> {
    let graph: IFilterGraph2 =
        unsafe { CoCreateInstance(&CLSID_FilterGraph, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("CoCreateInstance(FilterGraph): {e}"))?;
    let builder: ICaptureGraphBuilder2 = unsafe {
        CoCreateInstance(&CLSID_CaptureGraphBuilder2, None, CLSCTX_INPROC_SERVER)
    }
    .map_err(|e| format!("CoCreateInstance(CaptureGraphBuilder2): {e}"))?;
    unsafe { builder.SetFiltergraph(&graph) }.map_err(|e| format!("SetFiltergraph: {e}"))?;

    let capture = unsafe { find_capture_filter(&cfg.device_id) }?;
    unsafe { graph.AddFilter(&capture, w!("capture")) }
        .map_err(|e| format!("AddFilter(capture): {e}"))?;

    let caps = unsafe { apply_and_enumerate_modes(source_ref, &builder, &capture, cfg) };

    let grabber_filter: IBaseFilter = unsafe {
        CoCreateInstance(&CLSID_SAMPLE_GRABBER, None, CLSCTX_INPROC_SERVER)
    }
    .map_err(|e| format!("CoCreateInstance(SampleGrabber): {e}"))?;
    let grabber: ISampleGrabber = grabber_filter
        .cast()
        .map_err(|e| format!("cast ISampleGrabber: {e}"))?;
    let target_subtype = caps
        .selected
        .map(|s| s.subtype)
        .unwrap_or_else(|| subtype_for(cfg.pixel_format));
    tracing::debug!(
        source = source_ref,
        subtype = %subtype_name(&target_subtype),
        native = caps.selected.is_some(),
        "DirectShow: SampleGrabber SetMediaType"
    );
    let media_type = partial_video_type(&target_subtype);
    unsafe { grabber.SetMediaType(&media_type) }.map_err(|e| format!("SetMediaType: {e}"))?;
    unsafe { graph.AddFilter(&grabber_filter, w!("grabber")) }
        .map_err(|e| format!("AddFilter(grabber): {e}"))?;

    let null_renderer: IBaseFilter = unsafe {
        CoCreateInstance(&CLSID_NULL_RENDERER, None, CLSCTX_INPROC_SERVER)
    }
    .map_err(|e| format!("CoCreateInstance(NullRenderer): {e}"))?;
    unsafe { graph.AddFilter(&null_renderer, w!("null")) }
        .map_err(|e| format!("AddFilter(null): {e}"))?;

    let offered = if caps.offered.is_empty() {
        "none".to_string()
    } else {
        caps.offered.join(", ")
    };
    if let Err(e) = unsafe {
        builder.RenderStream(
            Some(&PIN_CATEGORY_CAPTURE),
            &MEDIATYPE_Video,
            &capture,
            &grabber_filter,
            &null_renderer,
        )
    } {
        // The graph could not connect capture -> SampleGrabber -> null. The
        // offered subtypes usually explain why (e.g. 10-bit/VideoInfo2).
        tracing::error!(source = source_ref, "DirectShow: RenderStream failed: {e}; device offers [{offered}]");
        return Err(format!("RenderStream: {e}; device offers [{offered}]"));
    }

    let mut connected = AM_MEDIA_TYPE::default();
    unsafe { grabber.GetConnectedMediaType(&mut connected) }
        .map_err(|e| format!("GetConnectedMediaType: {e}"))?;
    tracing::debug!(
        source = source_ref,
        subtype = %subtype_name(&connected.subtype),
        format_type = %format_type_name(connected.formattype),
        "DirectShow: connected media type"
    );
    let fmt = unsafe { interpret_connected(&connected) };
    unsafe { free_media_type(&mut connected) };
    let fmt = fmt?;

    let media_control: IMediaControl =
        graph.cast().map_err(|e| format!("cast IMediaControl: {e}"))?;

    Ok((
        Graph {
            _graph: graph,
            media_control,
            grabber,
            _capture: capture,
            _grabber_filter: grabber_filter,
            _null_renderer: null_renderer,
        },
        fmt,
        caps.modes,
    ))
}

/// Advertised modes, the distinct subtype names seen, and the native format
/// chosen for the capture pin (for diagnostics and the SampleGrabber).
struct ModeCaps {
    modes: Vec<CaptureMode>,
    offered: Vec<String>,
    selected: Option<SelectedFormat>,
}

/// A device-native output format chosen from `IAMStreamConfig` caps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectedFormat {
    subtype: GUID,
    pixel_format: PixelFormat,
}

/// Enumerate the capture pin's advertised modes and, when `cfg` requests a
/// size/rate, apply it. Best-effort: a device that refuses the request falls
/// back to its default. Every cap is logged for diagnostics.
unsafe fn apply_and_enumerate_modes(
    source_ref: &str,
    builder: &ICaptureGraphBuilder2,
    capture: &IBaseFilter,
    cfg: &DirectShowSourceConfig,
) -> ModeCaps {
    let pin = match unsafe {
        builder.FindPin(
            capture,
            windows::Win32::Media::DirectShow::PINDIR_OUTPUT,
            Some(&PIN_CATEGORY_CAPTURE),
            Some(&MEDIATYPE_Video),
            true,
            0,
        )
    } {
        Ok(pin) => pin,
        Err(e) => {
            tracing::warn!(source = source_ref, "DirectShow: no capture pin for IAMStreamConfig: {e}");
            return ModeCaps { modes: Vec::new(), offered: Vec::new(), selected: None };
        }
    };
    let config = match pin.cast::<IAMStreamConfig>() {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!(source = source_ref, "DirectShow: capture pin has no IAMStreamConfig: {e}");
            return ModeCaps { modes: Vec::new(), offered: Vec::new(), selected: None };
        }
    };

    let mut count = 0i32;
    let mut caps_size = 0i32;
    if let Err(e) = unsafe { config.GetNumberOfCapabilities(&mut count, &mut caps_size) } {
        tracing::warn!(source = source_ref, "DirectShow: GetNumberOfCapabilities failed: {e}");
        return ModeCaps { modes: Vec::new(), offered: Vec::new(), selected: None };
    }
    tracing::debug!(source = source_ref, caps = count, "DirectShow: IAMStreamConfig caps");

    let want = cfg.width > 0 && cfg.height > 0;
    let preference = preference_for(cfg.pixel_format);
    let mut best_rank: Option<usize> = None;
    let mut selected: Option<SelectedFormat> = None;
    let mut modes = Vec::new();
    let mut offered = Vec::new();
    let buf_len = (caps_size.max(size_of::<VIDEO_STREAM_CONFIG_CAPS>() as i32)).max(0) as usize;

    for i in 0..count {
        let mut media_type: *mut AM_MEDIA_TYPE = ptr::null_mut();
        let mut caps = vec![0u8; buf_len];
        if unsafe { config.GetStreamCaps(i, &mut media_type, caps.as_mut_ptr()) }.is_err()
            || media_type.is_null()
        {
            continue;
        }
        let mt = unsafe { &*media_type };
        let subtype = subtype_name(&mt.subtype);
        offered.push(subtype.clone());

        // Read geometry from either header variant; SampleGrabber only accepts
        // FORMAT_VideoInfo, but VideoInfo2 is worth logging verbatim.
        let mut w = 0u32;
        let mut h = 0u32;
        let mut fps_num = 0u32;
        let mut fps_den = 0u32;
        let mut bits = 0u16;
        let mut compression = 0u32;
        if !mt.pbFormat.is_null() && mt.formattype == FORMAT_VideoInfo {
            let vih = unsafe { &*(mt.pbFormat as *const VIDEOINFOHEADER) };
            w = vih.bmiHeader.biWidth.unsigned_abs();
            h = vih.bmiHeader.biHeight.unsigned_abs();
            bits = vih.bmiHeader.biBitCount;
            compression = vih.bmiHeader.biCompression;
            if vih.AvgTimePerFrame > 0 {
                fps_num = 10_000_000;
                fps_den = vih.AvgTimePerFrame as u32;
            }
        } else if !mt.pbFormat.is_null() && mt.formattype == FORMAT_VideoInfo2 {
            let vih = unsafe { &*(mt.pbFormat as *const VIDEOINFOHEADER2) };
            w = vih.bmiHeader.biWidth.unsigned_abs();
            h = vih.bmiHeader.biHeight.unsigned_abs();
            bits = vih.bmiHeader.biBitCount;
            compression = vih.bmiHeader.biCompression;
            if vih.AvgTimePerFrame > 0 {
                fps_num = 10_000_000;
                fps_den = vih.AvgTimePerFrame as u32;
            }
        }
        tracing::debug!(
            source = source_ref,
            index = i,
            subtype = %subtype,
            format_type = %format_type_name(mt.formattype),
            width = w,
            height = h,
            fps = %if fps_den == 0 { "?".to_string() } else { format!("{:.2}", fps_num as f64 / fps_den as f64) },
            bits,
            compression = %fourcc(compression),
            cb_format = mt.cbFormat,
            "DirectShow: cap"
        );

        if mt.formattype == FORMAT_VideoInfo {
            if w > 0 && h > 0 {
                modes.push(CaptureMode { width: w, height: h, fps_num, fps_den });
            }
            if let Some(rank) = preference.iter().position(|g| *g == mt.subtype) {
                let size_ok = !want || (w == cfg.width && h == cfg.height);
                let fps_ok =
                    cfg.fps_num == 0 || (fps_num == cfg.fps_num && fps_den == cfg.fps_den);
                if size_ok && fps_ok && best_rank.is_none_or(|b| rank < b) {
                    match unsafe { config.SetFormat(media_type) } {
                        Ok(()) => {
                            best_rank = Some(rank);
                            selected = Some(SelectedFormat {
                                subtype: mt.subtype,
                                pixel_format: pixel_format_for_subtype(&mt.subtype)
                                    .map(|(f, _)| f)
                                    .unwrap_or(PixelFormat::Bgra8),
                            });
                            tracing::debug!(source = source_ref, subtype = %subtype, width = w, height = h, "DirectShow: SetFormat applied");
                        }
                        Err(e) => {
                            tracing::warn!(source = source_ref, subtype = %subtype, width = w, height = h, "DirectShow: SetFormat failed: {e}");
                        }
                    }
                }
            }
        }
        unsafe { free_media_type(&mut *media_type) };
        unsafe { CoTaskMemFree(Some(media_type as *const c_void)) };
    }

    modes.sort_unstable();
    modes.dedup();
    offered.sort_unstable();
    offered.dedup();
    match selected {
        Some(s) => tracing::info!(
            source = source_ref,
            subtype = %subtype_name(&s.subtype),
            pixel_format = s.pixel_format.label(),
            "DirectShow: selected device format"
        ),
        None if !offered.is_empty() => tracing::warn!(
            source = source_ref,
            offered = %offered.join(", "),
            "DirectShow: no compositor-supported native format in caps"
        ),
        None => {}
    }
    return ModeCaps { modes, offered, selected };
}

/// Human-readable subtype name for diagnostics; falls back to a FOURCC decode
/// (e.g. `HDYC`) then the raw GUID.
fn subtype_name(subtype: &GUID) -> String {
    if *subtype == MEDIASUBTYPE_RGB32 {
        return "RGB32".to_string();
    }
    if *subtype == MEDIASUBTYPE_RGB24 {
        return "RGB24".to_string();
    }
    if *subtype == MEDIASUBTYPE_YUY2 {
        return "YUY2".to_string();
    }
    if *subtype == MEDIASUBTYPE_UYVY {
        return "UYVY".to_string();
    }
    if *subtype == MEDIASUBTYPE_NV12 {
        return "NV12".to_string();
    }
    if *subtype == MEDIASUBTYPE_v210 {
        return "v210".to_string();
    }
    if *subtype == MEDIASUBTYPE_MJPG {
        return "MJPG".to_string();
    }
    // Uncompressed video subtypes carry a FOURCC in `data1` with the standard
    // FIFO template; decode it so HDYC and friends are named in the log.
    const TEMPLATE: [u8; 8] = [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71];
    if subtype.data2 == 0 && subtype.data3 == 0x0010 && subtype.data4 == TEMPLATE {
        let cc = subtype.data1.to_le_bytes();
        if cc.iter().all(|b| b.is_ascii_graphic()) {
            return String::from_utf8_lossy(&cc).to_string();
        }
    }
    return format!("{subtype:?}");
}

/// `FORMAT_VideoInfo` / `FORMAT_VideoInfo2` / other, for diagnostics.
fn format_type_name(format_type: GUID) -> String {
    if format_type == FORMAT_VideoInfo {
        return "VideoInfo".to_string();
    }
    if format_type == FORMAT_VideoInfo2 {
        return "VideoInfo2".to_string();
    }
    return format!("{format_type:?}");
}

/// Render a `biCompression` value: a four-character code when printable, else hex.
fn fourcc(value: u32) -> String {
    let bytes = value.to_le_bytes();
    if value != 0 && bytes.iter().all(|b| b.is_ascii_graphic()) {
        return String::from_utf8_lossy(&bytes).to_string();
    }
    return format!("0x{value:08X}");
}

/// Read dimensions, stride, pixel layout and orientation from the negotiated
/// media type.
unsafe fn interpret_connected(mt: &AM_MEDIA_TYPE) -> Result<ConnectedFormat, String> {
    if mt.formattype == FORMAT_VideoInfo2 {
        return Err(format!(
            "negotiated FORMAT_VideoInfo2 with subtype {} (SampleGrabber cannot accept it)",
            subtype_name(&mt.subtype)
        ));
    }
    if mt.formattype != FORMAT_VideoInfo || mt.pbFormat.is_null() {
        return Err(format!(
            "negotiated {} with subtype {} has no VIDEOINFOHEADER",
            format_type_name(mt.formattype),
            subtype_name(&mt.subtype)
        ));
    }
    let vih = unsafe { &*(mt.pbFormat as *const VIDEOINFOHEADER) };
    let width = vih.bmiHeader.biWidth.unsigned_abs();
    let height = vih.bmiHeader.biHeight.unsigned_abs();
    if width == 0 || height == 0 {
        return Err("negotiated type has no frame size".to_string());
    }
    let (pixel_format, bits) = match pixel_format_for_subtype(&mt.subtype) {
        Some(pf) => pf,
        None => {
            return Err(format!("unsupported subtype {}", subtype_name(&mt.subtype)));
        }
    };
    let pitch = (width * bits).div_ceil(32) * 4;
    let rows = if pixel_format == PixelFormat::Nv12 { height + height / 2 } else { height };
    // DirectShow RGB/YUV is bottom-up when biHeight is positive; flip so the
    // compositor sees a top-down image.
    let flip = vih.bmiHeader.biHeight > 0;
    let nominal_fps = if vih.AvgTimePerFrame > 0 {
        10_000_000.0 / vih.AvgTimePerFrame as f64
    } else {
        0.0
    };
    Ok(ConnectedFormat {
        width,
        height,
        pitch,
        rows,
        pixel_format,
        flip,
        nominal_fps,
    })
}

/// An `AM_MEDIA_TYPE` constraining only the major type and subtype, for
/// `ISampleGrabber::SetMediaType` (the graph inserts a converter if needed).
fn partial_video_type(subtype: &GUID) -> AM_MEDIA_TYPE {
    AM_MEDIA_TYPE {
        majortype: MEDIATYPE_Video,
        subtype: *subtype,
        bFixedSizeSamples: TRUE,
        bTemporalCompression: FALSE,
        lSampleSize: 0,
        formattype: FORMAT_VideoInfo,
        pUnk: ManuallyDrop::new(None),
        cbFormat: 0,
        pbFormat: ptr::null_mut(),
    }
}

/// The DirectShow subtype requested for a pixel-format preference.
fn subtype_for(format: Option<PixelFormat>) -> GUID {
    return match format {
        Some(PixelFormat::Yuy2) => MEDIASUBTYPE_YUY2,
        Some(PixelFormat::Uyvy422) => MEDIASUBTYPE_UYVY,
        Some(PixelFormat::Nv12) => MEDIASUBTYPE_NV12,
        // RGB32 is BGRX in memory (see mediafoundation); it is the DS default.
        _ => MEDIASUBTYPE_RGB32,
    };
}

/// `MEDIASUBTYPE_HDYC`: 8-bit 4:2:2 Rec.709, not in the SDK headers. Its byte
/// layout is identical to UYVY, so it maps to the compositor's existing UYVY path.
const MEDIASUBTYPE_HDYC: GUID = GUID::from_u128(0x43594448_0000_0010_8000_00aa00389b71);

/// Output subtypes to try, in preference order, for a pixel-format request.
/// `None` (Auto) prefers an uncompressed format the compositor renders; an
/// explicit choice restricts to that one subtype.
fn preference_for(format: Option<PixelFormat>) -> Vec<GUID> {
    return match format {
        Some(PixelFormat::Bgra8) | Some(PixelFormat::Rgba8) => vec![MEDIASUBTYPE_RGB32],
        Some(PixelFormat::Uyvy422) => vec![MEDIASUBTYPE_UYVY],
        Some(PixelFormat::Yuy2) => vec![MEDIASUBTYPE_YUY2],
        Some(PixelFormat::Nv12) => vec![MEDIASUBTYPE_NV12],
        None => vec![
            MEDIASUBTYPE_RGB32,
            MEDIASUBTYPE_UYVY,
            MEDIASUBTYPE_YUY2,
            MEDIASUBTYPE_NV12,
        ],
    };
}

/// Internal pixel format and bit depth for a negotiated DS subtype.
fn pixel_format_for_subtype(subtype: &GUID) -> Option<(PixelFormat, u32)> {
    if *subtype == MEDIASUBTYPE_RGB32 {
        return Some((PixelFormat::Bgra8, 32));
    }
    if *subtype == MEDIASUBTYPE_UYVY || *subtype == MEDIASUBTYPE_HDYC {
        return Some((PixelFormat::Uyvy422, 16));
    }
    if *subtype == MEDIASUBTYPE_YUY2 {
        return Some((PixelFormat::Yuy2, 16));
    }
    if *subtype == MEDIASUBTYPE_NV12 {
        return Some((PixelFormat::Nv12, 12));
    }
    return None;
}

/// Free a DirectShow-allocated media type (format block + `pUnk`); the struct
/// itself is freed by the caller.
unsafe fn free_media_type(mt: &mut AM_MEDIA_TYPE) {
    if !mt.pbFormat.is_null() {
        unsafe { CoTaskMemFree(Some(mt.pbFormat as *const c_void)) };
        mt.pbFormat = ptr::null_mut();
    }
    let unk = unsafe { ManuallyDrop::take(&mut mt.pUnk) };
    drop(unk);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb32_is_the_auto_subtype() {
        assert_eq!(subtype_for(None), MEDIASUBTYPE_RGB32);
        assert_eq!(subtype_for(Some(PixelFormat::Yuy2)), MEDIASUBTYPE_YUY2);
        assert_eq!(subtype_for(Some(PixelFormat::Nv12)), MEDIASUBTYPE_NV12);
    }

    #[test]
    fn known_subtypes_are_named() {
        assert_eq!(subtype_name(&MEDIASUBTYPE_RGB32), "RGB32");
        assert_eq!(subtype_name(&MEDIASUBTYPE_UYVY), "UYVY");
        assert_eq!(subtype_name(&MEDIASUBTYPE_v210), "v210");
    }

    /// Blackmagic-style uncompressed subtypes carry a FOURCC; it must decode to
    /// a readable name instead of a raw GUID.
    #[test]
    fn fourcc_subtype_is_decoded() {
        let hdyc = GUID::from_u128(0x43594448_0000_0010_8000_00aa00389b71);
        assert_eq!(subtype_name(&hdyc), "HDYC");
    }

    #[test]
    fn fourcc_helper_formats() {
        assert_eq!(fourcc(0), "0x00000000");
        assert_eq!(fourcc(0x43594448), "HDYC");
    }

    #[test]
    fn auto_prefers_rgb32_then_uyvy() {
        assert_eq!(
            preference_for(None),
            vec![
                MEDIASUBTYPE_RGB32,
                MEDIASUBTYPE_UYVY,
                MEDIASUBTYPE_YUY2,
                MEDIASUBTYPE_NV12,
            ]
        );
    }

    #[test]
    fn explicit_format_restricts_to_one_subtype() {
        assert_eq!(preference_for(Some(PixelFormat::Uyvy422)), vec![MEDIASUBTYPE_UYVY]);
        assert_eq!(preference_for(Some(PixelFormat::Yuy2)), vec![MEDIASUBTYPE_YUY2]);
    }

    #[test]
    fn supported_subtypes_map_and_ten_bit_does_not() {
        assert_eq!(pixel_format_for_subtype(&MEDIASUBTYPE_UYVY), Some((PixelFormat::Uyvy422, 16)));
        assert_eq!(pixel_format_for_subtype(&MEDIASUBTYPE_HDYC), Some((PixelFormat::Uyvy422, 16)));
        assert_eq!(pixel_format_for_subtype(&MEDIASUBTYPE_v210), None);
    }
}
