#![allow(non_snake_case)]
// DirectShow SampleGrabber COM interop.
//
// ISampleGrabber/ISampleGrabberCB live in Qedit.h, which the Windows SDK no
// longer ships, so they are declared here from their documented vtable layout.
// The method names are COM's, not Rust's, hence the module-level
// `non_snake_case` allow.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Win32::Media::MediaFoundation::AM_MEDIA_TYPE;
use windows::core::{implement, interface, BOOL, GUID, IUnknown, IUnknown_Vtbl};

use super::super::{CpuFrame, Frame, FramePool, PixelFormat, SourceStats};

pub const CLSID_SAMPLE_GRABBER: GUID = GUID::from_u128(0xc1f400a0_3f08_11d3_9f0b_006008039e37);
pub const CLSID_NULL_RENDERER: GUID = GUID::from_u128(0xc1f400a4_3f08_11d3_9f0b_006008039e37);

#[interface("6B652FFF-11FE-4FCE-92AD-0266B5D7C78F")]
pub unsafe trait ISampleGrabber: IUnknown {
    pub unsafe fn SetOneShot(&self, oneshot: BOOL) -> windows::core::Result<()>;
    pub unsafe fn SetMediaType(&self, ptype: *const AM_MEDIA_TYPE) -> windows::core::Result<()>;
    pub unsafe fn GetConnectedMediaType(&self, ptype: *mut AM_MEDIA_TYPE) -> windows::core::Result<()>;
    pub unsafe fn SetBufferSamples(&self, buffer: BOOL) -> windows::core::Result<()>;
    pub unsafe fn GetCurrentBuffer(&self, pcbuffer: *mut i32, pbuffer: *mut i32) -> windows::core::Result<()>;
    pub unsafe fn GetCurrentSample(&self, sample: *mut *mut c_void) -> windows::core::Result<()>;
    pub unsafe fn SetCallback(&self, callback: *mut c_void, method: i32) -> windows::core::Result<()>;
}

#[interface("0579154A-2B53-4994-B0D0-E773148EFF85")]
pub unsafe trait ISampleGrabberCB: IUnknown {
    pub unsafe fn SampleCB(&self, sampletime: f64, sample: *mut c_void) -> windows::core::Result<()>;
    pub unsafe fn BufferCB(&self, sampletime: f64, buffer: *mut u8, length: i32) -> windows::core::Result<()>;
}

/// SampleGrabber callback: copies each delivered buffer into the frame slot.
#[implement(ISampleGrabberCB)]
pub struct Grabber {
    pub slot: Arc<Mutex<Option<Frame>>>,
    pub stats: Arc<Mutex<SourceStats>>,
    pub pool: Mutex<FramePool>,
    pub seq: AtomicU64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub rows: u32,
    pub pixel_format: PixelFormat,
    pub flip: bool,
    pub nominal_fps: f64,
}

impl ISampleGrabberCB_Impl for Grabber_Impl {
    unsafe fn SampleCB(&self, _sampletime: f64, _sample: *mut c_void) -> windows::core::Result<()> {
        // Use BufferCB (method 1) instead; see SetCallback.
        return Err(windows::Win32::Foundation::E_NOTIMPL.into());
    }

    unsafe fn BufferCB(
        &self,
        _sampletime: f64,
        buffer: *mut u8,
        length: i32,
    ) -> windows::core::Result<()> {
        if buffer.is_null() || length <= 0 {
            return Ok(());
        }
        let t0 = Instant::now();
        let len = (length as usize).min(self.pitch as usize * self.rows as usize);
        let mut pool = self.pool.lock().unwrap();
        let mut buf = pool.take(len);
        unsafe { std::ptr::copy_nonoverlapping(buffer, buf.as_mut_ptr(), len) };
        if self.flip {
            flip_rows_vertical(&mut buf, self.pitch as usize, self.rows as usize);
        }
        let copy_ms = t0.elapsed().as_secs_f32() * 1000.0;
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        {
            let mut s = self.stats.lock().unwrap();
            s.record_frame(self.width, self.height, self.pixel_format.label(), self.nominal_fps);
            s.record_copy_time(copy_ms);
        }
        let old = self.slot.lock().unwrap().replace(Frame::Cpu(CpuFrame {
            data: Arc::new(buf),
            w: self.width,
            h: self.height,
            fmt: self.pixel_format,
            pitch: self.pitch,
            seq,
        }));
        pool.give(old);
        return Ok(());
    }
}

/// Reverse the row order of a bottom-up frame in place.
fn flip_rows_vertical(buf: &mut [u8], row_len: usize, rows: usize) {
    if rows < 2 || row_len == 0 || buf.len() < row_len * rows {
        return;
    }
    let (mut top, mut bottom) = (0usize, rows - 1);
    while top < bottom {
        let (head, tail) = buf.split_at_mut(bottom * row_len);
        head[top * row_len..(top + 1) * row_len].swap_with_slice(&mut tail[..row_len]);
        top += 1;
        bottom -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::flip_rows_vertical;

    #[test]
    fn flip_rows_reverses_row_order() {
        let row_len = 4;
        let mut buf: Vec<u8> = (0..12).collect();
        flip_rows_vertical(&mut buf, row_len, 3);
        assert_eq!(buf, vec![8, 9, 10, 11, 4, 5, 6, 7, 0, 1, 2, 3]);
    }
}
