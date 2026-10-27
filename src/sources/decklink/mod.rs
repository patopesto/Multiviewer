mod discovery;
mod output;
mod source;

use std::ffi::c_char;

use multiviewer_decklink::{decklink_api_version, DecklinkPixelFormat};

use crate::sources::PixelFormat;

pub use discovery::{
    Discovery as DecklinkDiscovery, OutputMode as DecklinkOutputMode,
    OutputPort as DecklinkOutputPort,
};
pub use source::{DecklinkSource, DecklinkSourceConfig};
pub use output::{DecklinkOutput, DecklinkOutputConfig};

// external crate re-exports
pub use multiviewer_decklink::{
    VideoConnection as DecklinkVideoConnection,
    VideoConnections as DecklinkVideoConnections,
    DisplayMode as DecklinkMode,
};

/// Map a DeckLink capture pixel format to the app's format.
pub(crate) fn pixel_format(fmt: DecklinkPixelFormat) -> PixelFormat {
    return match fmt {
        DecklinkPixelFormat::Bgra8 => PixelFormat::Bgra8,
        DecklinkPixelFormat::Uyvy422 => PixelFormat::Uyvy422,
        _ => PixelFormat::Rgba8,
    };
}

/// Map a raw `BMDPixelFormat` value to the app's format.
pub(crate) fn pixel_format_from_raw(fmt: u32) -> Option<PixelFormat> {
    return DecklinkPixelFormat::try_from(fmt).ok().map(pixel_format);
}

/// Supported DeckLink API version reported by the installed driver, if any.
pub fn decklink_version() -> Option<String> {
    let mut buf = [0u8; 64];
    // SAFETY: buf is a valid, writable slice of `buf.len()` bytes; the shim
    // writes at most that many and NUL-terminates.
    let written = unsafe { decklink_api_version(buf.as_mut_ptr() as *mut c_char, buf.len() as i32) };
    if written <= 0 {
        return None;
    }
    return Some(String::from_utf8_lossy(&buf[..written as usize]).to_string());
}
