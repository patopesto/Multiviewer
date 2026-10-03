use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{E_ACCESSDENIED, RPC_E_CHANGED_MODE};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaSource, IMFMediaType, IMFSourceReader, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFStartup,
    MFSTARTUP_LITE, MF_VERSION, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MFMediaType_Video, MFVideoFormat_RGB32,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::core::{Error, PWSTR};

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceRef, SourceStats, VideoSource};
use super::discovery::activate_array;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaFoundationSourceConfig {
    /// Symbolic link of the capture device; empty uses the first discovered one.
    pub device_id: String,
}

/// Active Media Foundation capture source on its own thread.
pub struct MediaFoundationSource {
    source_ref: SourceRef,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MediaFoundationSource {
    pub fn spawn(source_ref: SourceRef, cfg: &MediaFoundationSourceConfig) -> Self {
        let slot = Arc::new(Mutex::new(None::<Frame>));
        let stats = Arc::new(Mutex::new(SourceStats::new()));
        let running = Arc::new(AtomicBool::new(true));

        let slot2 = slot.clone();
        let stats2 = stats.clone();
        let running2 = running.clone();
        let trace_ref = source_ref.clone();
        let device_id = cfg.device_id.clone();

        let thread = std::thread::Builder::new()
            .name(format!("mediafoundation-in-{source_ref}"))
            .spawn(move || {
                run_capture(trace_ref, device_id, slot2, stats2, running2);
            })
            .expect("spawn mediafoundation capture thread");

        Self {
            source_ref,
            slot,
            stats,
            running,
            thread: Some(thread),
        }
    }
}

impl Drop for MediaFoundationSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take()
            && let Err(e) = t.join()
        {
            tracing::error!(source = self.source_ref, "MediaFoundation thread join failed: {:?}", e);
        }
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
fn run_capture(
    source_ref: SourceRef,
    device_id: String,
    slot: Arc<Mutex<Option<Frame>>>,
    stats: Arc<Mutex<SourceStats>>,
    running: Arc<AtomicBool>,
) {
    tracing::info!(source = source_ref, device_id = device_id, "MediaFoundation capture thread starting");
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

    let reader = match unsafe { open_reader(&device_id) } {
        Ok(reader) => reader,
        Err(e) => {
            tracing::error!(source = source_ref, "MediaFoundation open failed: {e}");
            unsafe { shutdown() };
            return;
        }
    };
    log_negotiated_format(&source_ref, &reader);

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
                    tracing::warn!(
                        source = source_ref,
                        "MediaFoundation: no frames 3s after opening — reader opened but ReadSample yields nothing"
                    );
                }
            });
    }

    capture_frames(&source_ref, &reader, &slot, &stats, &running);

    unsafe { shutdown() };
}

/// Log what the reader actually negotiated, so a silent zero-size skip is
/// visible without a debugger.
fn log_negotiated_format(source_ref: &str, reader: &IMFSourceReader) {
    let (w, h) = unsafe { frame_size(reader) };
    if w == 0 || h == 0 {
        tracing::warn!(source = source_ref, "MediaFoundation: negotiated type has no frame size");
        return;
    }
    let subtype = unsafe { reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32) }
        .ok()
        .and_then(|mt| unsafe { mt.GetGUID(&MF_MT_SUBTYPE) }.ok())
        .map(|g| format!("{g:?}"))
        .unwrap_or_else(|| "unknown".to_string());
    tracing::info!(source = source_ref, "MediaFoundation opened: {w}x{h} subtype={subtype}");
}

/// Map an MF HRESULT to a message, calling out the camera-privacy case.
fn mf_err(context: &str, e: Error) -> String {
    if e.code() == E_ACCESSDENIED {
        return format!(
            "{context}: access denied — check Windows Settings > Privacy & security > Camera [{e}]"
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
/// negotiate RGB32 output so the compositor needs no YUV path.
unsafe fn open_reader(device_id: &str) -> Result<IMFSourceReader, String> {
    let activate = unsafe { find_activate(device_id)? };
    let source: IMFMediaSource =
        unsafe { activate.ActivateObject() }.map_err(|e| mf_err("ActivateObject", e))?;

    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }.map_err(|e| mf_err("MFCreateAttributes", e))?;
    let attributes = attributes.ok_or("MFCreateAttributes returned null")?;
    unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1) }
        .map_err(|e| mf_err("enable video processing", e))?;

    let reader = unsafe { MFCreateSourceReaderFromMediaSource(&source, &attributes) }
        .map_err(|e| mf_err("create source reader", e))?;

    let target = unsafe { build_rgb32_type() }?;
    unsafe {
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &target)
    }
    .map_err(|e| mf_err("select RGB32 output", e))?;

    Ok(reader)
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

unsafe fn read_string(attributes: &IMFActivate, key: &windows::core::GUID) -> Option<String> {
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

unsafe fn build_rgb32_type() -> Result<IMFMediaType, String> {
    let media_type = unsafe { MFCreateMediaType() }.map_err(|e| e.to_string())?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
    }
    .map_err(|e| e.to_string())?;
    unsafe {
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
    }
    .map_err(|e| e.to_string())?;
    Ok(media_type)
}

/// Poll samples until `running` is cleared, publishing each new frame.
fn capture_frames(
    source_ref: &str,
    reader: &IMFSourceReader,
    slot: &Arc<Mutex<Option<Frame>>>,
    stats: &Arc<Mutex<SourceStats>>,
    running: &Arc<AtomicBool>,
) {
    let mut pool = FramePool::new();
    let mut seq = 0u64;
    let mut logged_read_error = false;
    let mut logged_buffer_error = false;
    let mut logged_stream_error = false;
    let mut warned_zero_size = false;
    let mut logged_first_frame = false;

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

        let (w, h) = unsafe { frame_size(reader) };
        if w == 0 || h == 0 {
            if !warned_zero_size {
                warned_zero_size = true;
                tracing::warn!(source = source_ref, "MediaFoundation: frame has no dimensions, skipping");
            }
            let _ = unsafe { buffer.Unlock() };
            continue;
        }
        let row_len = (w * 4) as usize;
        let copy_len = (row_len * h as usize).min(current_len as usize);

        let t0 = std::time::Instant::now();
        let mut buf = pool.take(copy_len);
        unsafe { std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), copy_len) };
        let _ = unsafe { buffer.Unlock() };
        let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;

        seq += 1;
        {
            let mut s = stats.lock().unwrap();
            s.record_frame(w, h, PixelFormat::Bgra8.label(), 0.0);
            s.record_copy_time(copy_ms);
        }
        if !logged_first_frame {
            logged_first_frame = true;
            tracing::debug!(source = source_ref, "MediaFoundation: first frame {w}x{h} ({} bytes)", copy_len);
        }

        let old = {
            let mut guard = slot.lock().unwrap();
            guard.replace(Frame::Cpu(CpuFrame {
                data: Arc::new(buf),
                w,
                h,
                fmt: PixelFormat::Bgra8,
                pitch: row_len as u32,
                seq,
            }))
        };
        pool.give(old);
    }
}

/// RGB32 frame dimensions, decoded from the packed `MF_MT_FRAME_SIZE` attribute.
unsafe fn frame_size(reader: &IMFSourceReader) -> (u32, u32) {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let Ok(media_type) = (unsafe { reader.GetCurrentMediaType(stream) }) else {
        return (0, 0);
    };
    let Ok(packed) = (unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }) else {
        return (0, 0);
    };
    return ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
}

