use crate::engine::Engine;

pub struct App {
    engine: Engine,
    image_loaders_installed: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            engine: Engine::new(),
            image_loaders_installed: false,
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.image_loaders_installed {
            egui_extras::install_image_loaders(ctx);
            self.image_loaders_installed = true;
        }
        self.engine.update();
        self.engine.save_if_dirty();

        if let Some(rs) = frame.wgpu_render_state() {
            self.engine.ensure_compositor(&rs.device, &rs.queue, rs.target_format);
            self.engine.render_outputs();
        }

        // Keep the UI rendering continuously; without this egui only repaints on input events.
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        super::side_panel::draw(ui, &mut self.engine);

        egui::CentralPanel::default().show(ui, |ui| {
            super::canvas::update(ui, &mut self.engine, &ctx, frame);
        });
    }
}
