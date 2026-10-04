mod discovery;
mod source;

pub use discovery::Discovery as WindowsCaptureDiscovery;
pub use source::{WindowsCaptureSource, WindowsCaptureSourceConfig};
pub use source::{
    CaptureBorder as WindowsCaptureBorder,
    CaptureCursor as WindowsCaptureCursor,
    CaptureSecondaryWindows as WindowsCaptureSecondaryWindows,
};
