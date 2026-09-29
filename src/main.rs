// Hide the console window in release builds
// Use the --console flag to re-attach stdout/stderr when launching from a terminal for debugging.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

mod compositor;
mod config;
mod engine;
mod platform;
mod session;
mod sources;
mod ui;

pub const APP_NAME: &str = "Multiviewer";
pub const PROJECT_FILE_EXTENSION: &str = "multiviewer";

fn main() -> eframe::Result<()> {
    let (show_console, startup_path) = parse_cli();
    platform::init_console(show_console);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "multiviewer=info".into()),
        )
        .init();

    platform::setup_app();

    let session = session::Session::load();
    let startup_path = startup_path.or(session.last_project);

    let icon = eframe::icon_data::from_png_bytes(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/AppIcon.png")))
        .expect("failed to decode app icon");
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_icon(Arc::new(icon)),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    platform::configure_wgpu(&mut options);

    println!("Hello from Multiviewer");
    return eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            platform::on_window_created(&cc.egui_ctx);
            log_wgpu_backend(cc);
            return Ok(Box::new(ui::App::new(startup_path)));
        }),
    );
}

fn parse_cli() -> (bool, Option<std::path::PathBuf>) {
    let mut show_console = false;
    let mut positional: Option<std::path::PathBuf> = None;

    for arg in std::env::args().skip(1) {
        if arg == "--console" {
            show_console = true;
            continue;
        }
        if positional.is_none() && !arg.starts_with('-') {
            positional = Some(std::path::PathBuf::from(arg));
        }
    }

    return (show_console, positional);
}

fn log_wgpu_backend(cc: &eframe::CreationContext<'_>) {
    match &cc.wgpu_render_state {
        Some(render_state) => {
            let info = render_state.adapter.get_info();
            tracing::info!(
                "wgpu backend {:?}, adapter '{}' ({:?})",
                info.backend,
                info.name,
                info.device_type
            );
        }
        None => tracing::info!("wgpu renderer not in use"),
    }
}

