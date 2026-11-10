#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{CvFrame, CvFramePipeline, texture_from_iosurface};

#[cfg(target_os = "windows")]
pub use windows::init_console;
#[cfg(not(target_os = "windows"))]
pub fn init_console(_show_console: bool) {}

#[cfg(target_os = "macos")]
pub use macos::setup_process;
#[cfg(not(target_os = "macos"))]
pub fn setup_process() {}

#[cfg(target_os = "macos")]
pub use macos::setup_app;
#[cfg(not(target_os = "macos"))]
pub fn setup_app() {}

#[cfg(target_os = "windows")]
pub use windows::configure_wgpu;
#[cfg(not(target_os = "windows"))]
pub fn configure_wgpu(_options: &mut eframe::NativeOptions) {}

#[cfg(target_os = "macos")]
pub use macos::on_window_created;
#[cfg(not(target_os = "macos"))]
pub fn on_window_created(_ctx: &egui::Context) {}

#[cfg(target_os = "macos")]
pub use macos::poll_open_file;
#[cfg(not(target_os = "macos"))]
pub fn poll_open_file() -> Option<std::path::PathBuf> {
    return None;
}

#[cfg(target_os = "windows")]
pub use windows::headless_backends;
#[cfg(not(target_os = "windows"))]
pub fn headless_backends() -> wgpu::Backends {
    return wgpu::Backends::PRIMARY | wgpu::Backends::GL;
}
