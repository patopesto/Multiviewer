use crate::sources::{DecklinkConfig, NdiConfig, SourceKind, TestConfig};
use crate::ui::side_panel::{settings_grid, settings_value};

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
    settings_grid(ui, "test_settings_grid", |ui| {
        ui.label("Size");
        settings_value(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("W");
                ui.add(egui::DragValue::new(&mut cfg.width).range(100..=4096));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut cfg.height).range(100..=4096));
            });
        });
        ui.end_row();
    });
    cfg.width != old_w || cfg.height != old_h
}

fn color_format_label(cf: grafton_ndi::ReceiverColorFormat) -> String {
    match cf {
        grafton_ndi::ReceiverColorFormat::BGRX_BGRA => "BGRX/BGRA".to_string(),
        grafton_ndi::ReceiverColorFormat::UYVY_BGRA => "UYVY/BGRA".to_string(),
        grafton_ndi::ReceiverColorFormat::RGBX_RGBA => "RGBX/RGBA".to_string(),
        grafton_ndi::ReceiverColorFormat::UYVY_RGBA => "UYVY/RGBA".to_string(),
        grafton_ndi::ReceiverColorFormat::Fastest => "Fastest".to_string(),
        grafton_ndi::ReceiverColorFormat::Best => "Best".to_string(),
        _ => format!("{:?}", cf),
    }
}

fn ndi_settings_ui(cfg: &mut NdiConfig, ui: &mut egui::Ui) -> bool {
    let old_bw = cfg.bandwidth;
    let old_cf = cfg.color_format;
    settings_grid(ui, "ndi_settings_grid", |ui| {
        ui.label("Bandwidth");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("ndi_bw")
                .width(ui.available_width())
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
        ui.end_row();

        ui.label("Color");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("ndi_color")
                .width(ui.available_width())
                .selected_text(color_format_label(cfg.color_format))
                .show_ui(ui, |ui| {
                    for variant in [
                        grafton_ndi::ReceiverColorFormat::BGRX_BGRA,
                        grafton_ndi::ReceiverColorFormat::UYVY_BGRA,
                        grafton_ndi::ReceiverColorFormat::RGBX_RGBA,
                        grafton_ndi::ReceiverColorFormat::UYVY_RGBA,
                        // grafton_ndi::ReceiverColorFormat::Fastest, // TODO: support UYVY+A format
                        // grafton_ndi::ReceiverColorFormat::Best,    // TODO: support PA16 and P216 formats
                    ] {
                        ui.selectable_value(
                            &mut cfg.color_format,
                            variant,
                            color_format_label(variant),
                        );
                    }
                });
        });
        ui.end_row();
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
        all_options
            .iter()
            .filter(|(_, value)| cfg.supported_connections.contains(*value))
            .cloned()
            .collect()
    };

    // Default to the first available connection if none is set.
    if cfg.connection.is_empty() && !available.is_empty() {
        cfg.connection = available[0].1.to_string();
    }

    settings_grid(ui, "decklink_settings_grid", |ui| {
        ui.label("Connection");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("decklink_conn")
                .width(ui.available_width())
                .selected_text(cfg.connection.clone())
                .show_ui(ui, |ui| {
                    for (label, value) in &available {
                        ui.selectable_value(&mut cfg.connection, value.to_string(), *label);
                    }
                });
        });
        ui.end_row();
    });
    cfg.connection != old_conn
}

#[cfg(target_os = "macos")]
fn syphon_settings_ui(_cfg: &mut SyphonConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    false
}
