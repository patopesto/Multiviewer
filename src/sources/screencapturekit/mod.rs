mod discovery;
mod source;

pub use discovery::Discovery as ScreenCaptureKitDiscovery;
pub use discovery::Target as ScreenCaptureTarget;
pub use discovery::ensure_screen_capture_access_requested;
pub use source::{ScreenCaptureKitSource, ScreenCaptureKitSourceConfig};
