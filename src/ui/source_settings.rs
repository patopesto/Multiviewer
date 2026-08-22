use crate::sources::{DecklinkSourceConfig, NdiSourceConfig, SourceKind, TestSourceConfig};
use crate::sources::test::{TestPattern, RadarDirection};
use crate::sources::decklink::VideoConnection;
use crate::ui::side_panel::{settings_grid, settings_value};

#[cfg(target_os = "macos")]
use crate::sources::SyphonSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::AvFoundationSourceConfig;

pub fn render_source_settings(source: &mut SourceKind, ui: &mut egui::Ui) -> bool {
    match source {
        SourceKind::Test(_, cfg) => test_settings_ui(cfg, ui),
        SourceKind::Ndi(_, cfg, _) => ndi_settings_ui(cfg, ui),
        SourceKind::Decklink(_, cfg, _) => decklink_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceKind::Syphon(_, cfg, _) => syphon_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceKind::AvFoundation(_, cfg, _) => avfoundation_settings_ui(cfg, ui),
    }
}

fn test_settings_ui(cfg: &mut TestSourceConfig, ui: &mut egui::Ui) -> bool {
    let old_w = cfg.width;
    let old_h = cfg.height;
    let old_pattern = cfg.pattern.clone();
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

        ui.label("Pattern");
        settings_value(ui, |ui| {
            let patterns = TestPattern::ALL.to_vec();
            egui::ComboBox::from_id_salt("test_pattern")
                .width(ui.available_width())
                .selected_text(cfg.pattern.label())
                .show_ui(ui, |ui| {
                    for pattern in &patterns {
                        ui.selectable_value(&mut cfg.pattern, pattern.clone(), pattern.label());
                    }
                });
        });
        ui.end_row();

        if let TestPattern::UvGradient { red, green, blue } = &mut cfg.pattern {
            ui.label("Channels");
            settings_value(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(red, "R");
                    ui.checkbox(green, "G");
                    ui.checkbox(blue, "B");
                });
            });
            ui.end_row();
        }

        if let TestPattern::Grid { cols, rows } = &mut cfg.pattern {
            ui.label("Grid");
            settings_value(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Cols");
                    ui.add(egui::DragValue::new(cols).range(1..=100));
                    ui.label("Rows");
                    ui.add(egui::DragValue::new(rows).range(1..=100));
                });
            });
            ui.end_row();
        }

        if let TestPattern::Radar { width, speed, direction } = &mut cfg.pattern {
            ui.label("Width");
            settings_value(ui, |ui| {
                ui.add(egui::DragValue::new(width).range(1..=cfg.width));
            });
            ui.end_row();

            ui.label("Speed");
            settings_value(ui, |ui| {
                ui.add(egui::DragValue::new(speed).range(0.0..=20.0));
            });
            ui.end_row();

            ui.label("Direction");
            settings_value(ui, |ui| {
                egui::ComboBox::from_id_salt("ndi_bw")
                    .width(ui.available_width())
                    .selected_text(format!("{:?}", direction))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(direction, RadarDirection::Horizontal, "Horizontal");
                        ui.selectable_value(direction, RadarDirection::Vertical, "Vertical");
                    });
            });
            ui.end_row();
        }
    });
    cfg.width != old_w || cfg.height != old_h || cfg.pattern != old_pattern
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

fn ndi_settings_ui(cfg: &mut NdiSourceConfig, ui: &mut egui::Ui) -> bool {
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

fn decklink_settings_ui(cfg: &mut DecklinkSourceConfig, ui: &mut egui::Ui) -> bool {
    let old_conn = cfg.connection;

    let available: Vec<VideoConnection> = if cfg.supported_connections.is_empty() {
        VideoConnection::ALL.to_vec()
    } else {
        cfg.supported_connections.iter().collect()
    };

    settings_grid(ui, "decklink_settings_grid", |ui| {
        ui.label("Connection");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("decklink_conn")
                .width(ui.available_width())
                .selected_text(cfg.connection.label())
                .show_ui(ui, |ui| {
                    for conn in &available {
                        ui.selectable_value(&mut cfg.connection, *conn, conn.label());
                    }
                });
        });
        ui.end_row();
    });
    cfg.connection != old_conn
}

#[cfg(target_os = "macos")]
fn syphon_settings_ui(_cfg: &mut SyphonSourceConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    false
}

#[cfg(target_os = "macos")]
fn avfoundation_settings_ui(_cfg: &mut AvFoundationSourceConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    false
}
