mod compositor;
mod config;
mod ndi;
mod source;
mod ui;

use compositor::Compositor;
use config::Config;
use eframe::egui_wgpu;
use source::Registry;
use std::sync::Arc;

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
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

struct App {
    cfg: Config,
    registry: Registry,
    ndi: Option<ndi::Discovery>,
    comp: Option<Compositor>,
    dirty: bool,
}

impl App {
    fn new() -> Self {
        let cfg = Config::load();
        let mut registry = Registry::new();
        let mut cfg = cfg;
        // first launch: seed a demo layout so the grid isn't empty
        if cfg.grid.cells.iter().all(|c| c.is_none()) {
            for _ in 0..4 {
                registry.add_test();
            }
            for (i, name) in registry.names().map(str::to_owned).enumerate() {
                cfg.grid.cells[i] = Some(name);
            }
        }
        let ndi = ndi::Discovery::start();
        Self {
            cfg,
            registry,
            ndi: Some(ndi),
            comp: None,
            dirty: true,
        }
    }
}

struct GridCallback {
    rect_px: [f32; 4], // x, y, w, h (physical pixels, origin top-left)
    shared: Arc<compositor::Shared>,
    verts: Arc<Vec<compositor::Vert>>,
    draws: Vec<(u32, Arc<wgpu::BindGroup>)>,
}

impl egui_wgpu::CallbackTrait for GridCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        queue.write_buffer(&self.shared.vb, 0, bytemuck::cast_slice(&self.verts));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        rpass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let [x, y, w, h] = self.rect_px;
        rpass.set_viewport(x, y, w, h, 0.0, 1.0);
        rpass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
        rpass.set_pipeline(&self.shared.pipeline);
        rpass.set_vertex_buffer(0, self.shared.vb.slice(..));
        rpass.set_index_buffer(self.shared.ib.slice(..), wgpu::IndexFormat::Uint16);
        for (first, bg) in &self.draws {
            rpass.set_bind_group(0, &**bg, &[]);
            rpass.draw_indexed(*first..*first + 6, 0, 0..1);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        ui::side_panel(ctx, &mut self.cfg, &mut self.registry, self.ndi.as_ref(), &mut self.dirty);

        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            let (response, painter) = ui.allocate_painter(rect.size(), egui::Sense::hover());
            let rect = response.rect;

            let Some(rs) = frame.wgpu_render_state() else { return };
            if self.comp.is_none() {
                self.comp = Some(Compositor::new(&rs.device, &rs.queue, rs.target_format));
            }
            let comp = self.comp.as_mut().unwrap();

            let draw = comp.build(&rs.device, &rs.queue, &self.cfg.grid, &self.registry, rect);
            let ppp = ctx.pixels_per_point();
            painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                GridCallback {
                    rect_px: [
                        rect.min.x * ppp,
                        rect.min.y * ppp,
                        rect.width() * ppp,
                        rect.height() * ppp,
                    ],
                    shared: comp.shared.clone(),
                    verts: draw.verts,
                    draws: draw.draws,
                },
            ));
        });

        if self.dirty {
            self.cfg.save();
            self.dirty = false;
        }
        ctx.request_repaint(); // vsync-paced; sources arrive independently
    }
}
