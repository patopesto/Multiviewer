mod compositor;
mod config;
mod engine;
mod session;
mod sources;
mod ui;

pub const APP_NAME: &str = "Multiviewer";
pub const PROJECT_FILE_EXTENSION: &str = "multiviewer";

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "multiviewer=info".into()),
        )
        .init();

    let session = session::Session::load();
    let startup_path = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .or(session.last_project);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 900.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |_cc| Ok(Box::new(ui::App::new(startup_path)))),
    )
}
