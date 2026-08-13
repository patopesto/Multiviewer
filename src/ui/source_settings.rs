use crate::sources::{DecklinkConfig, NdiConfig, SourceKind, TestConfig};

#[cfg(target_os = "macos")]
use crate::sources::SyphonConfig;

pub fn render_source_settings(source: &mut SourceKind, ui: &mut egui::Ui) -> bool {
    match source {
        SourceKind::Test(_, cfg) => test_settings_ui(cfg, ui),
        SourceKind::Ndi(_, cfg, _) => ndi_settings_ui(cfg, ui),
        SourceKind::Decklink(_, cfg, _) => decklink_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceKind::Syphon(_, cfg, _) => syphon_settings_ui(cfg, ui),
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
                    "RGBX/RGBA",
                );
                ui.selectable_value(
                    &mut cfg.color_format,
                    grafton_ndi::ReceiverColorFormat::UYVY_RGBA,
                    "UYVY/RGBA",
                );
            });
    });
    cfg.bandwidth != old_bw || cfg.color_format != old_cf
}

fn decklink_settings_ui(cfg: &mut DecklinkConfig, ui: &mut egui::Ui) -> bool {
    let old_conn = cfg.connection.clone();
    let all_options = [
        ("SDI", "SDI"),
        ("HDMI", "HDMI"),
        ("Optical SDI", "Optical SDI"),
        ("Component", "Component"),
        ("Composite", "Composite"),
        ("S-Video", "S-Video"),
        ("Ethernet", "Ethernet"),
        ("Optical Ethernet", "Optical Ethernet"),
        ("Internal", "Internal"),
    ];

    let available: Vec<(&str, &str)> = if cfg.supported_connections.is_empty() {
        all_options.to_vec()
    } else {
        all_options.iter()
            .filter(|(_, value)| cfg.supported_connections.contains(*value))
            .cloned()
            .collect()
    };

    // Default to the first available connection if none is set.
    if cfg.connection.is_empty() && !available.is_empty() {
        cfg.connection = available[0].1.to_string();
    }

    ui.horizontal(|ui| {
        ui.label("Connection");
        egui::ComboBox::from_id_salt("decklink_conn")
            .selected_text(cfg.connection.clone())
            .show_ui(ui, |ui| {
                for (label, value) in &available {
                    ui.selectable_value(&mut cfg.connection, value.to_string(), *label);
                }
            });
    });
    cfg.connection != old_conn
}

#[cfg(target_os = "macos")]
fn syphon_settings_ui(_cfg: &mut SyphonConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    false
}
