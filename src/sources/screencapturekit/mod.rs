pub mod discovery;
pub mod source;

pub use discovery::{ensure_screen_capture_access_requested, Discovery};
pub use source::{ScreenCaptureKitSource, ScreenCaptureKitSourceConfig};
