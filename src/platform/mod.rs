#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{init_console, on_window_created, poll_open_file, setup_app};

#[cfg(target_os = "windows")]
pub use windows::{init_console, on_window_created, poll_open_file, setup_app};

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
