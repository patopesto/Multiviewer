use crate::sources::SourceStats;

pub fn render_source_stats(stats: &SourceStats, ui: &mut egui::Ui) {
    egui::Grid::new("source_stats_grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Resolution:");
            ui.label(format!(
                "{}x{} {}",
                stats.width, stats.height, stats.pixel_format
            ));
            ui.end_row();

            ui.label("Nominal FPS:");
            if stats.nominal_fps > 0.0 {
                ui.label(format!("{:.2}", stats.nominal_fps));
            } else {
                ui.label("-");
            }
            ui.end_row();

            ui.label("Computed FPS:");
            ui.label(format!("{:.2}", stats.computed_fps));
            ui.end_row();

            ui.label("Frames received:");
            ui.label(format!("{}", stats.frames_received));
            ui.end_row();

            ui.label("Frames presented:");
            ui.label(format!("{}", stats.frames_presented));
            ui.end_row();

            ui.label("Frames dropped:");
            ui.label(format!("{}", stats.frames_dropped));
            ui.end_row();

            ui.label("CPU->GPU copy:");
            ui.label(format!("{:.2} ms", stats.copy_time_ms));
            ui.end_row();

            ui.label("GPU upload:");
            ui.label(format!("{:.2} ms", stats.upload_time_ms));
            ui.end_row();
        });
}
