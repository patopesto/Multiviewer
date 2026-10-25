use std::sync::Arc;

use eframe::egui_wgpu::WgpuSetup;
use wgpu::{Backend, DeviceDescriptor, Features, Limits};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{init_console, setup_app, configure_wgpu, on_window_created, poll_open_file};
#[cfg(target_os = "macos")]
pub use macos::{CvFrame, CvFramePipeline, texture_from_iosurface};

#[cfg(target_os = "windows")]
pub use windows::{init_console, setup_app, configure_wgpu, on_window_created, poll_open_file};

/// Ask for the texture storage feature `vmx` checks to pick its decode path:
/// without it the decoder computes into a buffer and copies it into the frame.
fn install_device_descriptor(options: &mut eframe::NativeOptions) {
    let WgpuSetup::CreateNew(create_new) = &mut options.wgpu_options.wgpu_setup else {
        return;
    };
    create_new.device_descriptor = Arc::new(
        |adapter: &wgpu::Adapter| -> DeviceDescriptor<'static> {
            let mut features = Features::empty();
            if adapter.features().contains(Features::BGRA8UNORM_STORAGE) {
                features |= Features::BGRA8UNORM_STORAGE;
            }
            let base_limits = if adapter.get_info().backend == Backend::Gl {
                Limits::downlevel_webgl2_defaults()
            } else {
                Limits::default()
            };
            return DeviceDescriptor {
                label: Some("egui wgpu device"),
                required_features: features,
                required_limits: Limits {
                    max_texture_dimension_2d: 8192,
                    ..base_limits
                },
                ..Default::default()
            };
        },
    );
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn configure_wgpu(options: &mut eframe::NativeOptions) {
    install_device_descriptor(options);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn init_console(_show_console: bool) {}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn setup_app() {}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn on_window_created(_ctx: &egui::Context) {}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn poll_open_file() -> Option<std::path::PathBuf> {
    return None;
}
