// Hide the console window in release builds
// Use the --console flag to re-attach stdout/stderr when launching from a terminal for debugging.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tracing_subscriber::prelude::*;

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
    let (show_console, startup_path, trace_path) = parse_cli();
    platform::init_console(show_console);

    let trace_guard = init_tracing(trace_path);

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
            return Ok(Box::new(ui::App::new(cc, startup_path, trace_guard)));
        }),
    );
}

fn parse_cli() -> (bool, Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    let mut show_console = false;
    let mut trace_path: Option<std::path::PathBuf> = None;
    let mut positional: Option<std::path::PathBuf> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--console" => show_console = true,
            "--trace" => {
                i += 1;
                trace_path = args.get(i).map(std::path::PathBuf::from);
            }
            arg if positional.is_none() && !arg.starts_with('-') => {
                positional = Some(std::path::PathBuf::from(arg));
            }
            _ => {}
        }
        i += 1;
    }

    return (show_console, positional, trace_path);
}

/// Chrome/Perfetto trace export when `--trace <file>.json` is passed;
fn init_tracing(trace_path: Option<std::path::PathBuf>) -> Option<tracing_chrome::FlushGuard> {

    // Console logging, controlled by RUST_LOG
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "Multiviewer=info".into());
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(filter);

    // Profiling to Chrome/Perfecto format
    let Some(trace_path) = trace_path else {
        tracing_subscriber::registry().with(fmt_layer).init();
        return None;
    };
    let writer = match std::fs::File::create(&trace_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot create trace file {}: {e}", trace_path.display());
            tracing_subscriber::registry().with(fmt_layer).init();
            return None;
        }
    };
    let (chrome_layer, guard) = tracing_chrome::ChromeLayerBuilder::new()
        .writer(writer)
        .include_args(true)
        .build();

    // Filter out external lib traces
    let chrome_layer = chrome_layer.with_filter(
        tracing_subscriber::filter::Targets::new()
            .with_target(env!("CARGO_CRATE_NAME"), tracing::Level::DEBUG),
    );
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(chrome_layer)
        .init();
    tracing::info!("writing trace to {}", trace_path.display());
    
    return Some(guard);
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

