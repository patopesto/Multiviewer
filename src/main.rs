// Hide the console window in release builds
// Use the --console flag to re-attach stdout/stderr when launching from a terminal for debugging.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tracing_subscriber::prelude::*;

mod compositor;
mod config;
mod control;
mod engine;
mod platform;
mod secret;
mod session;
mod sources;
mod ui;

pub const APP_NAME: &str = "Multiviewer";
pub const PROJECT_FILE_EXTENSION: &str = "multiviewer";

fn main() -> eframe::Result<()> {
    let start = std::time::Instant::now();
    let cli = parse_cli(std::env::args().skip(1));
    // Wall clock from process start, so --exit-after includes startup.
    let deadline = cli.exit_after.map(|run| start + run);
    platform::init_console(cli.console);

    let trace_guard = init_tracing(cli.trace);

    platform::setup_app();

    let session = session::Session::load();
    let startup_path = cli.project.or(session.last_project);

    let icon = eframe::icon_data::from_png_bytes(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/appicon/AppIcon.png")))
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
            install_wgpu_error_handler(cc);
            return Ok(Box::new(ui::App::new(cc, startup_path, trace_guard, deadline)));
        }),
    );
}

/// Command-line options. Anything unrecognised is ignored.
#[derive(Default)]
struct Cli {
    console: bool,
    project: Option<std::path::PathBuf>,
    trace: Option<std::path::PathBuf>,
    exit_after: Option<std::time::Duration>,
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Cli {
    let mut cli = Cli::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--console" => cli.console = true,
            "--trace" => cli.trace = args.next().map(std::path::PathBuf::from),
            "--exit-after" => {
                let Some(value) = args.next() else {
                    eprintln!("--exit-after needs a duration in seconds");
                    continue;
                };
                match value.parse::<f32>() {
                    Ok(secs) if secs.is_finite() && secs >= 0.0 => {
                        cli.exit_after = Some(std::time::Duration::from_secs_f32(secs));
                    }
                    _ => eprintln!("ignoring --exit-after {value}: expected seconds"),
                }
            }
            other if cli.project.is_none() && !other.starts_with('-') => {
                cli.project = Some(std::path::PathBuf::from(other));
            }
            _ => {}
        }
    }
    return cli;
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

/// wgpu panics on every uncaptured validation error by default, which aborts the
/// process when it happens inside winit's macOS event handlers. The OMT source and
/// output submit GPU work on their own threads, racing eframe's surface reconfigure
/// during a resize and tripping "Failed to wait for GPU to come idle"; log the error
/// and let eframe reconfigure on the next frame instead of dying.
fn install_wgpu_error_handler(cc: &eframe::CreationContext<'_>) {
    let Some(render_state) = &cc.wgpu_render_state else {
        return;
    };
    render_state.device.on_uncaptured_error(Arc::new(|error| {
        tracing::error!("wgpu error: {error}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cli(args: &[&str]) -> Cli {
        return parse_cli(args.iter().map(|arg| arg.to_string()));
    }

    #[test]
    fn parses_exit_after_and_project() {
        let parsed = cli(&["--exit-after", "2.5", "show.multiviewer"]);
        assert_eq!(parsed.exit_after, Some(Duration::from_secs_f32(2.5)));
        assert_eq!(
            parsed.project,
            Some(std::path::PathBuf::from("show.multiviewer"))
        );
    }

    #[test]
    fn rejects_missing_or_invalid_exit_after() {
        assert_eq!(cli(&["--exit-after"]).exit_after, None);
        assert_eq!(cli(&["--exit-after", "soon"]).exit_after, None);
        assert_eq!(cli(&["--exit-after", "-1"]).exit_after, None);
        assert_eq!(cli(&["--exit-after", "inf"]).exit_after, None);
    }
}
