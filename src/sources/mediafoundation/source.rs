use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{E_ACCESSDENIED, RPC_E_CHANGED_MODE};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaSource, IMFMediaType, IMFSourceReader, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources,
    MFStartup, MFSTARTUP_LITE, MF_VERSION, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_E_HW_MFT_FAILED_START_STREAMING,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_RATE_RANGE_MAX,
    MF_MT_FRAME_RATE_RANGE_MIN, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MFMediaType_Video, MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_RGB32,
    MFVideoFormat_UYVY, MFVideoFormat_YUY2, MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::core::{Error, GUID, PWSTR};

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};
use super::discovery::activate_array;

/// One mode the device advertises natively (`IMFSourceReader::GetNativeMediaType`).
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
pub struct MediaFoundationSourceConfig {
    /// Symbolic link of the capture device; empty uses the first discovered one.
    pub device_id: String,
    /// Requested frame width in pixels; 0 lets the device pick its default.
    pub width: u32,
    /// Requested frame height in pixels; 0 lets the device pick its default.
    pub height: u32,
    /// Requested frame-rate numerator; 0 lets the device pick its default.
    pub fps_num: u32,
    /// Requested frame-rate denominator (stored exactly, e.g. 1001 for 29.97).
    pub fps_den: u32,
    /// Requested output pixel format; `None` = Auto (try the fallback chain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_format: Option<PixelFormat>,
}

/// Justifies the `unsafe impl Send`: the reader is created and driven on the
/// capture thread inside an MTA apartment. It is shared only so `Drop` can call
/// `Flush` from another thread to unblock a synchronous `ReadSample`; MF
/// objects are free-threaded.
struct SendReader(IMFSourceReader);
unsafe impl Send for SendReader {}

/// Active Media Foundation capture source on its own thread.
pub struct MediaFoundationSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    reader: Arc<Mutex<Option<SendReader>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MediaFoundationSource {
    /// `modes` is filled with the device's advertised modes on open; it is read
    /// by the settings UI.
    pub fn spawn(
        source_ref: SourceRef,
        cfg: &MediaFoundationSourceConfig,
        modes: Arc<Mutex<Vec<CaptureMode>>>,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));
        let reader = Arc::new(Mutex::new(None));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let running2 = running.clone();
        let reader2 = reader.clone();
        let trace_ref = source_ref.clone();
        let config = cfg.clone();

        let thread = std::thread::Builder::new()
            .name(format!("mediafoundation-in-{source_ref}"))
            .spawn(move || {
                run_capture(trace_ref, config, slot2, stats2, running2, reader2, modes);
            })
            .expect("spawn mediafoundation capture thread");

        Self {
            source_ref,
            slot,
            stats,
            running,
            reader,
            thread: Some(thread),
        }
    }
}

impl Drop for MediaFoundationSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        // A synchronous ReadSample can block indefinitely (e.g. an unplugged
        // device), so signal the reader to flush before joining.
        if let Some(reader) = self.reader.lock().unwrap().as_ref() {
            unsafe {
                let _ = reader.0.Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32);
            }
        }
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source = self.source_ref, "MediaFoundation thread join failed: {:?}", e);
        }
        *self.reader.lock().unwrap() = None;
    }
}

impl VideoSource for MediaFoundationSource {
    fn latest(&self, _device: &wgpu::Device, _queue: &wgpu::Queue) -> Option<Frame> {
        self.slot.lock().unwrap().clone()
    }

    fn stats(&self) -> Arc<Mutex<SourceStats>> {
        self.stats.clone()
    }
}

/// The full capture loop; owns the thread's COM/MF apartment for its lifetime.
// Each argument is a distinct shared handle the loop needs; bundling them into
// a struct would add indirection for no behavior change.
#[allow(clippy::too_many_arguments)]
fn run_capture(
    source_ref: SourceRef,
    cfg: MediaFoundationSourceConfig,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    reader_slot: Arc<Mutex<Option<SendReader>>>,
    modes: Arc<Mutex<Vec<CaptureMode>>>,
) {
    tracing::info!(source = source_ref, device_id = cfg.device_id, "MediaFoundation capture thread starting");
    unsafe {
        let coinit = CoInitializeEx(None, COINIT_MULTITHREADED);
        if coinit.is_err() && coinit != RPC_E_CHANGED_MODE {
            tracing::error!(source = source_ref, "MediaFoundation: CoInitializeEx failed");
            return;
        }
        if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
            tracing::error!(source = source_ref, "MediaFoundation: MFStartup failed");
            CoUninitialize();
            return;
        }
    }

    tracing::debug!(
        source = source_ref,
        width = cfg.width,
        height = cfg.height,
        fps = format!("{}/{}", cfg.fps_num, cfg.fps_den),
        "MediaFoundation requested format"
    );
    let reader = match unsafe { open_reader(&cfg) } {
        Ok(reader) => reader,
        Err(e) => {
            tracing::error!(source = source_ref, "MediaFoundation open failed: {e}");
            unsafe { shutdown() };
            return;
        }
    };
    // Share a reference so `Drop` can flush a blocked ReadSample.
    *reader_slot.lock().unwrap() = Some(SendReader(reader.clone()));

    log_negotiated_format(&source_ref, &reader);
    *modes.lock().unwrap() = unsafe { enumerate_modes(&reader) };
    // The negotiated subtype decides how the compositor reads the bytes; see
    // `pixel_format_for_subtype` for the (surprising) RGB32 mapping.
    let (pixel_format, bpp) = negotiated_pixel_format(&reader).unwrap_or((PixelFormat::Bgra8, 4));

    // A synchronous ReadSample blocks when the device never streams the
    // negotiated type, so a loop-side timeout can't fire; this watchdog reports
    // the stall from outside.
    {
        let stats = stats.clone();
        let running = running.clone();
        let source_ref = source_ref.clone();
        let _ = std::thread::Builder::new()
            .name(format!("mediafoundation-watchdog-{source_ref}"))
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                if running.load(Ordering::Relaxed)
                    && stats.lock().unwrap().frames_received == 0
                {
                    tracing::warn!(source = source_ref, "MediaFoundation: no frames 3s after opening — reader opened but ReadSample yields nothing");
                }
            });
    }

    capture_frames(&source_ref, &reader, &slot, &stats, &running, pixel_format, bpp);

    // Release the reader before MFShutdown so the device is freed promptly;
    // otherwise a quick restart can find it still held (preempted).
    *reader_slot.lock().unwrap() = None;
    drop(reader);
    unsafe { shutdown() };
}

/// Log what the reader actually negotiated, so a silent zero-size skip is
/// visible without a debugger.
fn log_negotiated_format(source_ref: &str, reader: &IMFSourceReader) {
    let (w, h, stride, num, den) = unsafe { frame_info(reader) };
    if w == 0 || h == 0 {
        tracing::warn!(source = source_ref, "MediaFoundation: negotiated type has no frame size");
        return;
    }
    let subtype = unsafe { reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) }
        .ok()
        .and_then(|mt| unsafe { mt.GetGUID(&MF_MT_SUBTYPE) }.ok())
        .map(|g| format!("{g:?}"))
        .unwrap_or_else(|| "unknown".to_string());
    let fps = if den == 0 { 0.0 } else { num as f64 / den as f64 };
    tracing::info!(source = source_ref, "MediaFoundation opened: {w}x{h} stride={stride} ~{fps:.0}fps subtype={subtype}");
}

/// Map an MF HRESULT to a message, calling out the common device failures.
fn mf_err(context: &str, e: Error) -> String {
    if e.code() == E_ACCESSDENIED {
        return format!(
            "{context}: access denied — check Windows Settings > Privacy & security > Camera [{e}]"
        );
    }
    if e.code() == MF_E_HW_MFT_FAILED_START_STREAMING {
        return format!(
            "{context}: camera is in use or was preempted by another app — close other camera apps (or it was not released yet after a restart) [{e}]"
        );
    }
    return format!("{context}: {e}");
}

unsafe fn shutdown() {
    unsafe {
        let _ = windows::Win32::Media::MediaFoundation::MFShutdown();
        CoUninitialize();
    }
}

/// Open the device by symbolic link (or the first device when empty) and
/// negotiate an output subtype the compositor can render.
unsafe fn open_reader(cfg: &MediaFoundationSourceConfig) -> Result<IMFSourceReader, String> {
    let activate = unsafe { find_activate(&cfg.device_id)? };
    let source: IMFMediaSource =
        unsafe { activate.ActivateObject() }.map_err(|e| mf_err("ActivateObject", e))?;

    // Output subtypes we can feed the compositor, from the requested format
    // (or the full fallback chain for Auto).
    let candidates = candidates_for(cfg.pixel_format);

    let mut last_err = String::new();
    let mut enum_reader: Option<IMFSourceReader> = None;
    // Advanced processing first: it is the only mode that does video resizing
    // and frame-rate conversion, so a requested size/rate is honored. Legacy
    // ("limited") processing is only a fallback for devices with no advanced
    // processor.
    for advanced in [true, false] {
        let reader = match unsafe { create_reader(&source, advanced) } {
            Ok(reader) => reader,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        for &subtype in &candidates {
            match unsafe { set_output_type(&reader, cfg, subtype) } {
                Ok(()) => return Ok(reader),
                Err(e) => {
                    last_err = e;
                    if enum_reader.is_none() {
                        enum_reader = Some(reader.clone());
                    }
                }
            }
        }
    }

    // Report what the device actually advertises so a device that exposes
    // neither RGB32 nor UYVY (e.g. YUY2/NV12 only) is obvious in the log.
    let offered = enum_reader
        .as_ref()
        .map(|r| unsafe { advertised_subtypes(r) })
        .unwrap_or_default();
    return Err(format!("no supported output format ({last_err}); device offers [{}]", offered.join(", ")));
}

/// Whether `activate` can be opened and negotiated to a compositor-supported
/// output type. Open-only: no samples are read, so this is cheap but cannot
/// detect a device that opens and never streams.
///
/// Discovery uses this to drop devices Media Foundation enumerates but cannot
/// open (e.g. Blackmagic's WDM-only capture cards).
///
/// ponytail: open-only per plan; if a device opens yet never streams and still
/// shows up, escalate by reading one sample with a bounded `Flush` watchdog.
pub(super) unsafe fn probe_activate(activate: &IMFActivate) -> bool {
    let Ok(source) = (unsafe { activate.ActivateObject::<IMFMediaSource>() }) else {
        return false;
    };
    let cfg = MediaFoundationSourceConfig::default();
    let candidates = candidates_for(None);
    for advanced in [true, false] {
        let Ok(reader) = (unsafe { create_reader(&source, advanced) }) else {
            continue;
        };
        for &subtype in &candidates {
            if unsafe { set_output_type(&reader, &cfg, subtype) }.is_ok() {
                return true;
            }
        }
    }
    return false;
}

unsafe fn create_reader(source: &IMFMediaSource, advanced: bool) -> Result<IMFSourceReader, String> {
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }.map_err(|e| mf_err("MFCreateAttributes", e))?;
    let attributes = attributes.ok_or("MFCreateAttributes returned null")?;
    let flag = if advanced {
        &MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING
    } else {
        &MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING
    };
    unsafe { attributes.SetUINT32(flag, 1) }.map_err(|e| mf_err("enable video processing", e))?;
    unsafe { attributes.SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1) }
        .map_err(|e| mf_err("disconnect on shutdown", e))?;

    let reader = unsafe { MFCreateSourceReaderFromMediaSource(source, &attributes) }
        .map_err(|e| mf_err("create source reader", e))?;
    return Ok(reader);
}

unsafe fn set_output_type(
    reader: &IMFSourceReader,
    cfg: &MediaFoundationSourceConfig,
    subtype: &GUID,
) -> Result<(), String> {
    let target = unsafe { build_output_type(cfg, subtype) }?;
    unsafe {
        reader.SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &target)
    }
    .map_err(|e| mf_err("select output type", e))?;
    // Drop any samples the reader buffered before the output type was set, so
    // the first delivered frames are the negotiated layout rather than the
    // device's pre-negotiation format (which the compositor would mis-decode).
    let _ = unsafe { reader.Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) };
    return Ok(());
}

/// Locate the `IMFActivate` for `device_id`, or the first video device when
/// `device_id` is empty.
unsafe fn find_activate(device_id: &str) -> Result<IMFActivate, String> {
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }.map_err(|e| e.to_string())?;
    let attributes = attributes.ok_or("MFCreateAttributes returned null")?;
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    }
    .map_err(|e| e.to_string())?;

    let mut raw_devices: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe { MFEnumDeviceSources(&attributes, &mut raw_devices, &mut count) }.map_err(|e| e.to_string())?;
    let slice = unsafe { activate_array(raw_devices, count) };

    let mut chosen = None;
    for activate in slice.iter().flatten() {
        if device_id.is_empty() {
            chosen = Some(activate.clone());
            break;
        }
        if let Some(link) =
            unsafe { read_string(activate, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK) }
            && link == device_id
        {
            chosen = Some(activate.clone());
            break;
        }
    }
    unsafe { CoTaskMemFree(Some(raw_devices as *const core::ffi::c_void)) };

    chosen.ok_or_else(|| format!("no Media Foundation device matches {device_id:?}"))
}

unsafe fn read_string(attributes: &IMFActivate, key: &GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut len = 0u32;
    unsafe { attributes.GetAllocatedString(key, &mut value, &mut len) }.ok()?;
    if value.is_null() {
        return None;
    }
    let s = unsafe { value.to_string() }.ok()?;
    unsafe { CoTaskMemFree(Some(value.as_ptr() as *const core::ffi::c_void)) };
    Some(s)
}

/// Output media type for the requested subtype. An explicit size/rate pins the
/// device to a requested mode; leaving them unset lets the device pick its
/// default.
unsafe fn build_output_type(
    cfg: &MediaFoundationSourceConfig,
    subtype: &GUID,
) -> Result<IMFMediaType, String> {
    let media_type = unsafe { MFCreateMediaType() }.map_err(|e| e.to_string())?;
    unsafe { media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video) }
        .map_err(|e| e.to_string())?;
    unsafe { media_type.SetGUID(&MF_MT_SUBTYPE, subtype) }.map_err(|e| e.to_string())?;
    if cfg.width > 0 && cfg.height > 0 {
        unsafe { media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_u32_pair(cfg.width, cfg.height)) }
            .map_err(|e| e.to_string())?;
    }
    if cfg.fps_num > 0 && cfg.fps_den > 0 {
        unsafe { media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(cfg.fps_num, cfg.fps_den)) }
            .map_err(|e| e.to_string())?;
    }
    Ok(media_type)
}

/// The output subtypes we can feed the compositor, in Auto preference order:
/// `(subtype, internal format, bytes per pixel for the tight-pitch fallback)`.
///
/// NOTE: `MFVideoFormat_RGB32` is **not** RGBA byte order despite the name. It
/// is `D3DFMT_X8R8G8B8` (GUID `...00000016...`), stored little-endian as bytes
/// **B, G, R, X** (BGRX), so it maps to `PixelFormat::Bgra8` (uploaded as
/// `Bgra8Unorm`) and **never** `Rgba8`, which would swap red and blue.
/// `MFVideoFormat_ARGB32` (`D3DFMT_A8R8G8B8`) is likewise BGRA in memory; only
/// `D3DFMT_A8B8G8R8` / `MFVideoFormat_ABGR32` is true RGBA.
const MF_FORMATS: &[(GUID, PixelFormat, u32)] = &[
    (MFVideoFormat_UYVY, PixelFormat::Uyvy422, 2),
    (MFVideoFormat_RGB32, PixelFormat::Bgra8, 4),
    (MFVideoFormat_YUY2, PixelFormat::Yuy2, 2),
    (MFVideoFormat_NV12, PixelFormat::Nv12, 1),
];

/// Internal pixel format and bytes-per-pixel for a negotiated MF subtype.
fn pixel_format_for_subtype(subtype: &GUID) -> Option<(PixelFormat, u32)> {
    return MF_FORMATS
        .iter()
        .find(|(guid, _, _)| guid == subtype)
        .map(|(_, format, bpp)| (*format, *bpp));
}

/// Subtypes to request for `choice`, in preference order. `None` (Auto) tries
/// every supported subtype; an explicit choice requests just that one (so an
/// unsupported request errors rather than silently falling back).
fn candidates_for(choice: Option<PixelFormat>) -> Vec<&'static GUID> {
    return match choice {
        None => MF_FORMATS.iter().map(|(guid, _, _)| guid).collect(),
        Some(format) => MF_FORMATS
            .iter()
            .filter(|(_, candidate, _)| *candidate == format)
            .map(|(guid, _, _)| guid)
            .collect(),
    };
}

fn negotiated_pixel_format(reader: &IMFSourceReader) -> Option<(PixelFormat, u32)> {
    let media_type =
        unsafe { reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) }.ok()?;
    let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.ok()?;
    return pixel_format_for_subtype(&subtype);
}

fn subtype_name(subtype: &GUID) -> &'static str {
    if *subtype == MFVideoFormat_RGB32 {
        return "RGB32";
    }
    if *subtype == MFVideoFormat_UYVY {
        return "UYVY";
    }
    if *subtype == MFVideoFormat_YUY2 {
        return "YUY2";
    }
    if *subtype == MFVideoFormat_NV12 {
        return "NV12";
    }
    if *subtype == MFVideoFormat_MJPG {
        return "MJPG";
    }
    return "other";
}

/// Distinct native subtypes the device advertises, for negotiation diagnostics.
unsafe fn advertised_subtypes(reader: &IMFSourceReader) -> Vec<&'static str> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        let Ok(media_type) = (unsafe { reader.GetNativeMediaType(stream, index) }) else {
            break;
        };
        if let Ok(subtype) = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) } {
            names.push(subtype_name(&subtype));
        }
        index += 1;
    }
    names.sort_unstable();
    names.dedup();
    return names;
}

fn pack_u32_pair(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

/// Poll samples until `running` is cleared, publishing each new frame.
fn capture_frames(
    source_ref: &str,
    reader: &IMFSourceReader,
    slot: &Arc<Mutex<Option<Frame>>>,
    stats: &Arc<Mutex<SourceStats>>,
    running: &Arc<AtomicBool>,
    mut pixel_format: PixelFormat,
    mut bpp: u32,
) {
    let mut pool = FramePool::new();
    let mut seq = 0u64;
    let mut logged_read_error = false;
    let mut logged_buffer_error = false;
    let mut logged_stream_error = false;
    let mut warned_zero_size = false;
    let mut logged_first_frame = false;
    let mut format_checked = false;

    while running.load(Ordering::Relaxed) {
        let mut sample = None;
        // `pdwStreamFlags` is a mandatory out-param: passing NULL makes
        // ReadSample return E_POINTER. `pdwActualStreamIndex` and
        // `pllTimestamp` are optional.
        let mut flags = 0u32;
        let result = unsafe {
            reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                None,
                Some(&mut sample),
            )
        };
        if let Err(e) = result {
            if !logged_read_error {
                logged_read_error = true;
                tracing::warn!(source = source_ref, "MediaFoundation ReadSample failed: {}", mf_err("ReadSample", e));
            }
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 && !logged_stream_error {
            logged_stream_error = true;
            tracing::warn!(source = source_ref, "MediaFoundation: stream reports an error");
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 && !logged_stream_error {
            logged_stream_error = true;
            tracing::warn!(source = source_ref, "MediaFoundation: end of stream");
        }
        let Some(sample) = sample else {
            // A stream tick / format change returns S_OK without a sample.
            continue;
        };

        // Adopt the reader's actual output format: once on the first sample and
        // whenever it changes. A device can deliver its native layout briefly
        // after open before settling on the requested subtype, and the
        // compositor decodes strictly by `pixel_format`.
        if !format_checked || flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            format_checked = true;
            if let Some((format, new_bpp)) = negotiated_pixel_format(reader)
                && (format != pixel_format || new_bpp != bpp)
            {
                pixel_format = format;
                bpp = new_bpp;
                tracing::info!(source = source_ref, "MediaFoundation: output format is now {} ({bpp} bytes/px)", pixel_format.label());
            }
        }

        let frame_span = tracing::debug_span!("mediafoundation_frame");
        let _frame_guard = frame_span.entered();

        let Ok(buffer) = (unsafe { sample.ConvertToContiguousBuffer() }) else {
            if !logged_buffer_error {
                logged_buffer_error = true;
                tracing::warn!(source = source_ref, "MediaFoundation: ConvertToContiguousBuffer failed");
            }
            continue;
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut current_len = 0u32;
        if unsafe { buffer.Lock(&mut ptr, None, Some(&mut current_len)) }.is_err() || ptr.is_null() {
            continue;
        }

        let (w, h, media_stride, fps_num, fps_den) = unsafe { frame_info(reader) };
        if w == 0 || h == 0 {
            if !warned_zero_size {
                warned_zero_size = true;
                tracing::warn!(source = source_ref, "MediaFoundation: frame has no dimensions, skipping");
            }
            let _ = unsafe { buffer.Unlock() };
            continue;
        }
        let nominal_fps = if fps_den == 0 { 0.0 } else { fps_num as f64 / fps_den as f64 };
        let tight = w.saturating_mul(bpp);
        // NV12 stores the half-height interleaved UV plane after Y.
        let rows = if pixel_format == PixelFormat::Nv12 { h + h / 2 } else { h };
        let pitch = effective_pitch(media_stride, current_len, tight, rows);
        let copy_len = (pitch as usize * rows as usize).min(current_len as usize);

        let t0 = std::time::Instant::now();
        let mut buf = pool.take(copy_len);
        unsafe { std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), copy_len) };
        let _ = unsafe { buffer.Unlock() };
        let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;

        seq += 1;
        {
            let mut s = stats.lock().unwrap();
            s.record_frame(w, h, pixel_format.label(), nominal_fps);
            s.record_copy_time(copy_ms);
        }
        if !logged_first_frame {
            logged_first_frame = true;
            tracing::debug!(source = source_ref, "MediaFoundation: first frame {w}x{h} {} pitch={pitch} ({copy_len} bytes)", pixel_format.label());
        }

        let old = {
            let mut guard = slot.lock().unwrap();
            guard.replace(Frame::Cpu(CpuFrame {
                data: Arc::new(buf),
                w,
                h,
                fmt: pixel_format,
                pitch,
                seq,
            }))
        };
        pool.give(old);
    }
}

/// Negotiated dimensions, absolute row stride and frame rate from the current
/// media type. Stride 0 means the attribute was absent.
unsafe fn frame_info(reader: &IMFSourceReader) -> (u32, u32, u32, u32, u32) {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let Ok(media_type) = (unsafe { reader.GetCurrentMediaType(stream) }) else {
        return (0, 0, 0, 0, 0);
    };
    let (w, h) = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }
        .map(|p| ((p >> 32) as u32, (p & 0xFFFF_FFFF) as u32))
        .unwrap_or((0, 0));
    let stride = unsafe { media_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
        .map(|s| (s as i32).unsigned_abs())
        .unwrap_or(0);
    let (num, den) = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) }
        .map(|p| ((p >> 32) as u32, (p & 0xFFFF_FFFF) as u32))
        .unwrap_or((0, 0));
    return (w, h, stride, num, den);
}

/// Enumerate the device's advertised modes via `GetNativeMediaType`, deduped
/// and sorted. A mode with no frame rate is kept with `fps_num/den == 0/0`.
unsafe fn enumerate_modes(reader: &IMFSourceReader) -> Vec<CaptureMode> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut modes = Vec::new();
    let mut index = 0u32;
    loop {
        // Any error means we ran past the last native type (`MF_E_NO_MORE_TYPES`).
        let Ok(media_type) = (unsafe { reader.GetNativeMediaType(stream, index) }) else {
            break;
        };
        if let Some((w, h)) = unsafe { media_size(&media_type) }
            && w > 0
            && h > 0
        {
            for (fps_num, fps_den) in unsafe { media_frame_rates(&media_type) } {
                modes.push(CaptureMode {
                    width: w,
                    height: h,
                    fps_num,
                    fps_den,
                });
            }
        }
        index += 1;
    }
    return normalize_modes(modes);
}

unsafe fn media_size(media_type: &IMFMediaType) -> Option<(u32, u32)> {
    let packed = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
    return Some(((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32));
}

/// Frame rate(s) advertised by a native type: the discrete `MF_MT_FRAME_RATE`
/// if present, else the min/max of the rate range. `(0, 0)` when unknown.
unsafe fn media_frame_rates(media_type: &IMFMediaType) -> Vec<(u32, u32)> {
    if let Ok(packed) = unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) } {
        return vec![((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)];
    }
    let mut rates = Vec::new();
    for key in [&MF_MT_FRAME_RATE_RANGE_MIN, &MF_MT_FRAME_RATE_RANGE_MAX] {
        if let Ok(packed) = unsafe { media_type.GetUINT64(key) } {
            rates.push(((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32));
        }
    }
    if rates.is_empty() {
        rates.push((0, 0));
    }
    return rates;
}

fn normalize_modes(mut modes: Vec<CaptureMode>) -> Vec<CaptureMode> {
    modes.sort_unstable();
    modes.dedup();
    return modes;
}

/// Row stride used for the upload. `tight` is the tightly-packed stride
/// (`w * bpp`); `rows` is the number of rows the buffer holds (`h`, or
/// `h + h/2` for NV12's stacked UV plane). Prefer the media type's stride when
/// it fits the locked buffer; otherwise derive it from the buffer length,
/// falling back to `tight`. Sign is dropped: a bottom-up (negative) stride
/// still occupies `|stride|` bytes per row; orientation is left to the
/// per-source Flip V property.
fn effective_pitch(media_stride: u32, buffer_len: u32, tight: u32, rows: u32) -> u32 {
    if rows == 0 || tight == 0 {
        return tight.max(1);
    }
    let rows = rows as usize;
    let stride = media_stride as usize;
    if stride >= tight as usize && stride * rows <= buffer_len as usize {
        return stride as u32;
    }
    let len = buffer_len as usize;
    if len.is_multiple_of(rows) && (len / rows) as u32 >= tight {
        return (len / rows) as u32;
    }
    return tight;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_modes_sorts_and_dedupes() {
        let input = vec![
            CaptureMode { width: 1920, height: 1080, fps_num: 30, fps_den: 1 },
            CaptureMode { width: 1280, height: 720, fps_num: 60, fps_den: 1 },
            CaptureMode { width: 1920, height: 1080, fps_num: 30, fps_den: 1 },
        ];
        let out = normalize_modes(input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].width, 1280);
        assert_eq!(out[1].width, 1920);
    }

    #[test]
    fn format_label_renders_integral_and_fractional_rates() {
        let integral = CaptureMode { width: 1280, height: 720, fps_num: 30, fps_den: 1 };
        assert_eq!(integral.label(), "1280x720 @ 30");
        let ntsc = CaptureMode { width: 1920, height: 1080, fps_num: 30000, fps_den: 1001 };
        assert_eq!(ntsc.label(), "1920x1080 @ 29.97");
        let unknown = CaptureMode { width: 640, height: 480, fps_num: 0, fps_den: 0 };
        assert_eq!(unknown.label(), "640x480 @ ?");
    }

    /// RGB32 is BGRX in memory, so it must map to Bgra8 — never Rgba8 (which
    /// would swap red and blue). Regression guard for the naming trap.
    #[test]
    fn rgb32_maps_to_bgra_not_rgba() {
        assert_eq!(
            pixel_format_for_subtype(&MFVideoFormat_RGB32),
            Some((PixelFormat::Bgra8, 4))
        );
    }

    #[test]
    fn yuv_subtypes_map() {
        assert_eq!(
            pixel_format_for_subtype(&MFVideoFormat_UYVY),
            Some((PixelFormat::Uyvy422, 2))
        );
        assert_eq!(
            pixel_format_for_subtype(&MFVideoFormat_YUY2),
            Some((PixelFormat::Yuy2, 2))
        );
        assert_eq!(
            pixel_format_for_subtype(&MFVideoFormat_NV12),
            Some((PixelFormat::Nv12, 1))
        );
    }

    #[test]
    fn effective_pitch_prefers_padded_stride_else_tight() {
        // Tight RGB32 and UYVY rows.
        assert_eq!(effective_pitch(0, 1280 * 4 * 720, 1280 * 4, 720), 1280 * 4);
        assert_eq!(effective_pitch(0, 1280 * 2 * 720, 1280 * 2, 720), 1280 * 2);
        // A padded media-type stride wins when it fits the buffer.
        assert_eq!(
            effective_pitch(1280 * 2 + 64, (1280 * 2 + 64) * 720, 1280 * 2, 720),
            1280 * 2 + 64
        );
        // NV12: rows = h + h/2, tight = w.
        let (w, h) = (1920u32, 1080u32);
        let rows = h + h / 2;
        assert_eq!(effective_pitch(0, w * rows, w, rows), w);
    }

    #[test]
    fn candidates_are_auto_fallback_or_a_single_explicit_subtype() {
        // Auto tries every supported subtype.
        assert_eq!(candidates_for(None).len(), MF_FORMATS.len());
        assert_eq!(
            candidates_for(Some(PixelFormat::Bgra8)),
            vec![&MFVideoFormat_RGB32]
        );
        assert_eq!(
            candidates_for(Some(PixelFormat::Uyvy422)),
            vec![&MFVideoFormat_UYVY]
        );
        assert_eq!(
            candidates_for(Some(PixelFormat::Yuy2)),
            vec![&MFVideoFormat_YUY2]
        );
        assert_eq!(
            candidates_for(Some(PixelFormat::Nv12)),
            vec![&MFVideoFormat_NV12]
        );
        // Rgba8 has no requestable MF subtype: no candidates (Auto is used).
        assert!(candidates_for(Some(PixelFormat::Rgba8)).is_empty());
    }
}
