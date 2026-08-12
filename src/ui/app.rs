use crate::engine::Engine;

pub struct App {
    engine: Engine,
}

impl App {
    pub fn new() -> Self {
        Self { engine: Engine::new() }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.engine.update();
        self.engine.save_if_dirty();
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
