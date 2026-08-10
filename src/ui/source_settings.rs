use crate::sources::{NdiConfig, SourceKind, TestConfig};

pub fn render_source_settings(source: &mut SourceKind, ui: &mut egui::Ui) -> bool {
    match source {
        SourceKind::Test(_, cfg) => test_settings_ui(cfg, ui),
        SourceKind::Ndi(_, cfg, _) => ndi_settings_ui(cfg, ui),
    }
}

fn test_settings_ui(cfg: &mut TestConfig, ui: &mut egui::Ui) -> bool {
    let old_w = cfg.width;
    let old_h = cfg.height;
    ui.horizontal(|ui| {
        ui.label("Size");
        ui.add(egui::DragValue::new(&mut cfg.width).range(100..=4096));
        ui.label("×");
        ui.add(egui::DragValue::new(&mut cfg.height).range(100..=4096));
    });
    cfg.width != old_w || cfg.height != old_h
}

fn ndi_settings_ui(cfg: &mut NdiConfig, ui: &mut egui::Ui) -> bool {
    let old_bw = cfg.bandwidth;
    let old_cf = cfg.color_format;
    ui.horizontal(|ui| {
        ui.label("Bandwidth");
        egui::ComboBox::from_id_salt("ndi_bw")
            .selected_text(format!("{:?}", cfg.bandwidth))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut cfg.bandwidth,
                    grafton_ndi::ReceiverBandwidth::Highest,
                    "Highest",
                );
                ui.selectable_value(
                    &mut cfg.bandwidth,
                    grafton_ndi::ReceiverBandwidth::Lowest,
                    "Lowest",
                );
            });
    });
    ui.horizontal(|ui| {
        ui.label("Color");
        egui::ComboBox::from_id_salt("ndi_color")
             .selected_text(format!("{:?}", cfg.color_format))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut cfg.color_format,
                    grafton_ndi::ReceiverColorFormat::RGBX_RGBA,
                    "RGBX_RGBA",
                );
                ui.selectable_value(
                    &mut cfg.color_format,
                    grafton_ndi::ReceiverColorFormat::UYVY_RGBA,
                    "UYVY_RGBA",
                );
            });
    });
    cfg.bandwidth != old_bw || cfg.color_format != old_cf
}
