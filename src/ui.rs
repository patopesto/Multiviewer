use crate::config::Config;
use crate::ndi::Discovery;
use crate::source::Registry;

pub fn side_panel(
    ctx: &egui::Context,
    cfg: &mut Config,
    registry: &mut Registry,
    ndi: Option<&Discovery>,
    dirty: &mut bool,
) {
    egui::SidePanel::left("panel").default_width(280.0).show(ctx, |ui| {
        ui.heading("Sources");
        if ui.button("+ Test source").clicked() {
            registry.add_test();
        }
        for name in registry.names().map(str::to_owned).collect::<Vec<_>>() {
            ui.label(format!("● {name}"));
        }

        ui.separator();
        ui.heading("NDI Sources");
        if let Some(disco) = ndi {
            let list = disco.list();
            if list.is_empty() {
                ui.label("(scanning…)");
            } else {
                for src in &list {
                    let name = &src.name;
                    let already = registry.names().any(|n| n == name);
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if already {
                            ui.label("✓");
                        } else if ui.button("Connect").clicked() {
                            registry.add_ndi(name.clone(), src.clone());
                        }
                    });
                }
            }
        } else {
            ui.label("NDI unavailable");
        }

        ui.separator();
        ui.heading("Grid");
        ui.horizontal(|ui| {
            let (mut r, mut c) = (cfg.grid.rows, cfg.grid.cols);
            ui.label("Rows");
            if ui.add(egui::DragValue::new(&mut r).range(1..=8)).changed() {
                cfg.grid.resize(r, c);
                *dirty = true;
            }
            ui.label("Cols");
            if ui.add(egui::DragValue::new(&mut c).range(1..=8)).changed() {
                cfg.grid.resize(r, c);
                *dirty = true;
            }
        });

        ui.separator();
        ui.heading("Cells");
        let names: Vec<String> = registry.names().map(str::to_owned).collect();
        let (rows, cols) = (cfg.grid.rows, cfg.grid.cols);
        egui::Grid::new("cells").show(ui, |ui| {
            for row in 0..rows {
                for col in 0..cols {
                    let i = (row * cols + col) as usize;
                    let cur = cfg.grid.cells[i].clone();
                    egui::ComboBox::from_id_salt(i)
                        .selected_text(cur.clone().unwrap_or_else(|| "—".into()))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(cur.is_none(), "—").clicked() {
                                cfg.grid.cells[i] = None;
                                *dirty = true;
                            }
                            for n in &names {
                                if ui.selectable_label(cur.as_deref() == Some(n), n).clicked() {
                                    cfg.grid.cells[i] = Some(n.clone());
                                    *dirty = true;
                                }
                            }
                        });
                }
                ui.end_row();
            }
        });
    });
}
