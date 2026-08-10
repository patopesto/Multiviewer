mod compositor;
mod config;
mod engine;
mod sources;
mod ui;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "multiviewer=info".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 900.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Multiviewer",
        options,
        Box::new(|_cc| Ok(Box::new(ui::App::new()))),
    )
}
