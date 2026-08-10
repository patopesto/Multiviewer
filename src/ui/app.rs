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
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.engine.update();

        super::side_panel::draw(ctx, &mut self.engine);

        egui::CentralPanel::default().show(ctx, |ui| {
            super::canvas::update(ui, &mut self.engine, ctx, frame);
        });

        self.engine.save_if_dirty();
        ctx.request_repaint(); // vsync-paced; sources arrive independently
    }
}
