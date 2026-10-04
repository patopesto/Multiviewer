use crate::ui::side_panel::{settings_grid, settings_value};
use crate::sources::{SourceKind, SourceRuntimeConfig, SourceConfig};
use crate::sources::{TestSourceConfig, TestPattern, RadarDirection};
use crate::sources::{NdiSourceConfig, NdiReceiverBandwidth, NdiReceiverColorFormat};
use crate::sources::{DecklinkSourceConfig, DecklinkVideoConnection, DecklinkVideoConnections};
#[cfg(target_os = "windows")]
use crate::sources::PixelFormat;
#[cfg(target_os = "macos")]
use crate::sources::SyphonSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::AvFoundationSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::ScreenCaptureKitSourceConfig;
#[cfg(target_os = "windows")]
use crate::sources::SpoutSourceConfig;
#[cfg(target_os = "windows")]
use crate::sources::{MediaFoundationSourceConfig, MediaFoundationMode};
#[cfg(target_os = "windows")]
use crate::sources::{DirectShowSourceConfig, DirectShowMode};
#[cfg(target_os = "windows")]
use crate::sources::{
    WindowsCaptureBorder, WindowsCaptureCursor, WindowsCaptureSecondaryWindows,
    WindowsCaptureSourceConfig,
};

pub fn render_source_settings(source: &mut SourceKind, ui: &mut egui::Ui) -> bool {
    return match &mut source.config {
        SourceConfig::Test(cfg) => test_settings_ui(cfg, ui),
        SourceConfig::Ndi(cfg) => ndi_settings_ui(cfg, ui),
        SourceConfig::Decklink(cfg) => decklink_settings_ui(cfg, &source.runtime, ui),
        #[cfg(target_os = "macos")]
        SourceConfig::Syphon(cfg) => syphon_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceConfig::AvFoundation(cfg) => avfoundation_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceConfig::ScreenCaptureKit(cfg) => screencapturekit_settings_ui(cfg, ui),
        #[cfg(target_os = "windows")]
        SourceConfig::Spout(cfg) => spout_settings_ui(cfg, ui),
        #[cfg(target_os = "windows")]
        SourceConfig::MediaFoundation(cfg) => mediafoundation_settings_ui(cfg, &source.runtime, ui),
        #[cfg(target_os = "windows")]
        SourceConfig::DirectShow(cfg) => directshow_settings_ui(cfg, &source.runtime, ui),
        #[cfg(target_os = "windows")]
        SourceConfig::WindowsCapture(cfg) => windowscapture_settings_ui(cfg, ui),
        // Config from a platform that has this protocol; no runtime source.
        SourceConfig::Unknown(_) => false,
    };
}

fn color_picker_row(ui: &mut egui::Ui, label: &str, color: &mut [u8; 3]) {
    ui.label(label);
    settings_value(ui, |ui| {
        let mut color_f32 = [
            color[0] as f32 / 255.0,
            color[1] as f32 / 255.0,
            color[2] as f32 / 255.0,
        ];
        if ui.color_edit_button_rgb(&mut color_f32).changed() {
            *color = [
                (color_f32[0] * 255.0) as u8,
                (color_f32[1] * 255.0) as u8,
                (color_f32[2] * 255.0) as u8,
            ];
        }
    });
    ui.end_row();
}

fn test_settings_ui(cfg: &mut TestSourceConfig, ui: &mut egui::Ui) -> bool {
    let old_w = cfg.width;
    let old_h = cfg.height;
    let old_pattern = cfg.pattern.clone();
    let old_cursor = cfg.cursor;
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

        if let TestPattern::UvGradient { red, green, blue, rotation } = &mut cfg.pattern {
            ui.label("Channels");
            settings_value(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(red, "R");
                    ui.checkbox(green, "G");
                    ui.checkbox(blue, "B");
                });
            });
            ui.end_row();

            ui.label("Rotation");
            settings_value(ui, |ui| {
                ui.add(egui::DragValue::new(rotation).range(0.0..=360.0).speed(1.0).suffix("°"));
            });
            ui.end_row();
        }

        if let TestPattern::Grid { cols, rows, bg_color, line_color } = &mut cfg.pattern {
            ui.label("Grid");
            settings_value(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Cols");
                    ui.add(egui::DragValue::new(cols).range(1..=50));
                    ui.label("Rows");
                    ui.add(egui::DragValue::new(rows).range(1..=50));
                });
            });
            ui.end_row();

            color_picker_row(ui, "Primary Color", line_color);
            color_picker_row(ui, "Background", bg_color);
        }

        if let TestPattern::Radar { width, speed, direction, bg_color, line_color } = &mut cfg.pattern {
            ui.label("Width");
            settings_value(ui, |ui| {
                ui.add(egui::DragValue::new(width).range(1..=cfg.width));
            });
            ui.end_row();

            ui.label("Speed");
            settings_value(ui, |ui| {
                ui.add(egui::DragValue::new(speed).range(0.0..=50.0).speed(0.5));
            });
            ui.end_row();

            ui.label("Direction");
            settings_value(ui, |ui| {
                egui::ComboBox::from_id_salt("radar_direction")
                    .width(ui.available_width())
                    .selected_text(format!("{:?}", direction))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(direction, RadarDirection::Right, "Right");
                        ui.selectable_value(direction, RadarDirection::Left, "Left");
                        ui.selectable_value(direction, RadarDirection::Down, "Down");
                        ui.selectable_value(direction, RadarDirection::Up, "Up");
                    });
            });
            ui.end_row();

            color_picker_row(ui, "Primary Color", line_color);
            color_picker_row(ui, "Background", bg_color);
        }

        ui.label("Cursor");
        settings_value(ui, |ui| {
            ui.checkbox(&mut cfg.cursor.enabled, "Enabled");
        });
        ui.end_row();

        if cfg.cursor.enabled {
            ui.label("Cursor speed");
            settings_value(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("X ");
                    ui.add(egui::DragValue::new(&mut cfg.cursor.speed_x).range(0.0..=100.0).speed(0.5));
                    ui.label("Y ");
                    ui.add(egui::DragValue::new(&mut cfg.cursor.speed_y).range(0.0..=100.0).speed(0.5));
                });
            });
            ui.end_row();

            ui.label("Cursor width");
            settings_value(ui, |ui| {
                ui.add(egui::DragValue::new(&mut cfg.cursor.width).range(1..=100));
            });
            ui.end_row();
        }
    });
    cfg.width != old_w || cfg.height != old_h || cfg.pattern != old_pattern || cfg.cursor != old_cursor
}

fn ndi_format_label(cf: NdiReceiverColorFormat) -> String {
    match cf {
        NdiReceiverColorFormat::BGRX_BGRA => "BGRX/BGRA".to_string(),
        NdiReceiverColorFormat::UYVY_BGRA => "UYVY/BGRA".to_string(),
        NdiReceiverColorFormat::RGBX_RGBA => "RGBX/RGBA".to_string(),
        NdiReceiverColorFormat::UYVY_RGBA => "UYVY/RGBA".to_string(),
        NdiReceiverColorFormat::Fastest => "Fastest".to_string(),
        NdiReceiverColorFormat::Best => "Best".to_string(),
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
                    ui.selectable_value(&mut cfg.bandwidth, NdiReceiverBandwidth::Highest, "Highest");
                    ui.selectable_value(&mut cfg.bandwidth, NdiReceiverBandwidth::Lowest, "Lowest");
                });
        });
        ui.end_row();

        ui.label("Pixel Format");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("ndi_pixel_format")
                .width(ui.available_width())
                .selected_text(ndi_format_label(cfg.color_format))
                .show_ui(ui, |ui| {
                    for variant in [
                        NdiReceiverColorFormat::BGRX_BGRA,
                        NdiReceiverColorFormat::UYVY_BGRA,
                        NdiReceiverColorFormat::RGBX_RGBA,
                        NdiReceiverColorFormat::UYVY_RGBA,
                        // NdiReceiverColorFormat::Fastest, // TODO: support UYVY+A format
                        // NdiReceiverColorFormat::Best,    // TODO: support PA16 and P216 formats
                    ] {
                        ui.selectable_value(&mut cfg.color_format, variant, ndi_format_label(variant));
                    }
                });
        });
        ui.end_row();
    });
    cfg.bandwidth != old_bw || cfg.color_format != old_cf
}

fn decklink_settings_ui(
    cfg: &mut DecklinkSourceConfig,
    runtime: &SourceRuntimeConfig,
    ui: &mut egui::Ui,
) -> bool {
    let old_conn = cfg.connection;
    // The connection mask lives in runtime data (discovered on connect), not
    // in the persisted config; absent a live source, offer every connection.
    let supported = match runtime {
        SourceRuntimeConfig::Decklink { supported_connections } => *supported_connections,
        _ => DecklinkVideoConnections::EMPTY,
    };

    let available: Vec<DecklinkVideoConnection> = if supported.is_empty() {
        DecklinkVideoConnection::ALL.to_vec()
    } else {
        supported.iter().collect()
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

#[cfg(target_os = "macos")]
fn screencapturekit_settings_ui(_cfg: &mut ScreenCaptureKitSourceConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    false
}

#[cfg(target_os = "windows")]
fn spout_settings_ui(_cfg: &mut SpoutSourceConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    false
}

#[cfg(target_os = "windows")]
fn mediafoundation_settings_ui(cfg: &mut MediaFoundationSourceConfig, runtime: &SourceRuntimeConfig, ui: &mut egui::Ui) -> bool {
    // modes are enumerated by the capture thread on open; absent a live source
    // (or before enumeration finishes) only "Auto" is offered.
    let modes = match runtime {
        SourceRuntimeConfig::MediaFoundation { modes, .. } => modes.lock().unwrap().clone(),
        _ => Vec::new(),
    };
    let mut modes = modes;
    modes.sort_unstable();
    modes.dedup();

    let current = if cfg.width > 0 && cfg.height > 0 && cfg.fps_num > 0 && cfg.fps_den > 0 {
        Some(MediaFoundationMode {
            width: cfg.width,
            height: cfg.height,
            fps_num: cfg.fps_num,
            fps_den: cfg.fps_den,
        })
    } else {
        None
    };
    let old = (
        cfg.width,
        cfg.height,
        cfg.fps_num,
        cfg.fps_den,
        cfg.pixel_format,
    );

    // Formats the compositor can render, plus Auto (the device default).
    const PIXEL_FORMATS: [Option<PixelFormat>; 5] = [
        None,
        Some(PixelFormat::Bgra8),
        Some(PixelFormat::Uyvy422),
        Some(PixelFormat::Yuy2),
        Some(PixelFormat::Nv12),
    ];

    settings_grid(ui, "mediafoundation_settings_grid", |ui| {
        ui.label("Mode");
        settings_value(ui, |ui| {
            let text = current
                .map(|m| m.label())
                .unwrap_or_else(|| "Auto (device default)".to_string());
            egui::ComboBox::from_id_salt("mediafoundation_mode")
                .width(ui.available_width())
                .height(1000.0)
                .selected_text(text)
                .truncate()
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "Auto (device default)").clicked() {
                        cfg.width = 0;
                        cfg.height = 0;
                        cfg.fps_num = 0;
                        cfg.fps_den = 0;
                    }
                    for m in &modes {
                        if ui.selectable_label(current == Some(*m), m.label()).clicked() {
                            cfg.width = m.width;
                            cfg.height = m.height;
                            cfg.fps_num = m.fps_num;
                            cfg.fps_den = m.fps_den;
                        }
                    }
                    if modes.is_empty() {
                        ui.weak("(no modes reported)");
                    }
                });
        });
        ui.end_row();

        ui.label("Pixel Format");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("mediafoundation_pixel_format")
                .width(ui.available_width())
                .selected_text(mediafoundation_format_label(cfg.pixel_format))
                .show_ui(ui, |ui| {
                    for choice in PIXEL_FORMATS {
                        if ui.selectable_label(cfg.pixel_format == choice, mediafoundation_format_label(choice)).clicked() {
                            cfg.pixel_format = choice;
                        }
                    }
                });
        });
        ui.end_row();
    });

    (
        cfg.width,
        cfg.height,
        cfg.fps_num,
        cfg.fps_den,
        cfg.pixel_format,
    ) != old
}

/// Display label for a requested Media Foundation output format; `None` is Auto.
#[cfg(target_os = "windows")]
fn mediafoundation_format_label(format: Option<PixelFormat>) -> &'static str {
    return match format {
        None => "Auto (device default)",
        Some(PixelFormat::Bgra8) => "RGB32",
        Some(PixelFormat::Uyvy422) => "UYVY 4:2:2",
        Some(PixelFormat::Yuy2) => "YUY2 4:2:2",
        Some(PixelFormat::Nv12) => "NV12 4:2:0",
        Some(PixelFormat::Rgba8) => "RGBA8",
    };
}

/// Display label for a requested DirectShow output format; `None` is Auto (RGB32).
#[cfg(target_os = "windows")]
fn directshow_format_label(format: Option<PixelFormat>) -> &'static str {
    return match format {
        None => "Auto (device default)",
        Some(PixelFormat::Bgra8) => "RGB32",
        Some(PixelFormat::Uyvy422) => "UYVY 4:2:2",
        Some(PixelFormat::Yuy2) => "YUY2 4:2:2",
        Some(PixelFormat::Nv12) => "NV12 4:2:0",
        Some(PixelFormat::Rgba8) => "RGBA8",
    };
}

#[cfg(target_os = "windows")]
fn directshow_settings_ui(cfg: &mut DirectShowSourceConfig, runtime: &SourceRuntimeConfig, ui: &mut egui::Ui) -> bool {
    // modes are enumerated by the capture thread on open; absent a live source
    // (or before enumeration finishes) only "Auto" is offered.
    let modes = match runtime {
        SourceRuntimeConfig::DirectShow { modes, .. } => modes.lock().unwrap().clone(),
        _ => Vec::new(),
    };
    let mut modes = modes;
    modes.sort_unstable();
    modes.dedup();

    let current = if cfg.width > 0 && cfg.height > 0 && cfg.fps_num > 0 && cfg.fps_den > 0 {
        Some(DirectShowMode {
            width: cfg.width,
            height: cfg.height,
            fps_num: cfg.fps_num,
            fps_den: cfg.fps_den,
        })
    } else {
        None
    };
    let old = (
        cfg.width,
        cfg.height,
        cfg.fps_num,
        cfg.fps_den,
        cfg.pixel_format,
    );

    // Formats the compositor can render, plus Auto (the device default).
    const PIXEL_FORMATS: [Option<PixelFormat>; 5] = [
        None,
        Some(PixelFormat::Bgra8),
        Some(PixelFormat::Uyvy422),
        Some(PixelFormat::Yuy2),
        Some(PixelFormat::Nv12),
    ];

    settings_grid(ui, "directshow_settings_grid", |ui| {
        ui.label("Mode");
        settings_value(ui, |ui| {
            let text = current
                .map(|m| m.label())
                .unwrap_or_else(|| "Auto (device default)".to_string());
            egui::ComboBox::from_id_salt("directshow_mode")
                .width(ui.available_width())
                .height(1000.0)
                .selected_text(text)
                .truncate()
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "Auto (device default)").clicked() {
                        cfg.width = 0;
                        cfg.height = 0;
                        cfg.fps_num = 0;
                        cfg.fps_den = 0;
                    }
                    for m in &modes {
                        if ui.selectable_label(current == Some(*m), m.label()).clicked() {
                            cfg.width = m.width;
                            cfg.height = m.height;
                            cfg.fps_num = m.fps_num;
                            cfg.fps_den = m.fps_den;
                        }
                    }
                    if modes.is_empty() {
                        ui.weak("(no modes reported)");
                    }
                });
        });
        ui.end_row();

        ui.label("Pixel Format");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("directshow_pixel_format")
                .width(ui.available_width())
                .selected_text(directshow_format_label(cfg.pixel_format))
                .show_ui(ui, |ui| {
                    for choice in PIXEL_FORMATS {
                        if ui.selectable_label(cfg.pixel_format == choice, directshow_format_label(choice)).clicked() {
                            cfg.pixel_format = choice;
                        }
                    }
                });
        });
        ui.end_row();
    });

    (
        cfg.width,
        cfg.height,
        cfg.fps_num,
        cfg.fps_den,
        cfg.pixel_format,
    ) != old
}

#[cfg(target_os = "windows")]
fn windowscapture_settings_ui(cfg: &mut WindowsCaptureSourceConfig, ui: &mut egui::Ui) -> bool {
    let is_window = cfg.is_window();
    let settings = cfg.settings_mut();
    let old = settings.clone();

    settings_grid(ui, "windowscapture_settings_grid", |ui| {
        ui.label("Cursor");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("windowscapture_cursor")
                .width(ui.available_width())
                .selected_text(settings.cursor.label())
                .show_ui(ui, |ui| {
                    for value in [
                        WindowsCaptureCursor::Default,
                        WindowsCaptureCursor::Show,
                        WindowsCaptureCursor::Hide,
                    ] {
                        ui.selectable_value(&mut settings.cursor, value, value.label());
                    }
                });
        });
        ui.end_row();

        ui.label("Borders");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("windowscapture_borders")
                .width(ui.available_width())
                .selected_text(settings.border.label())
                .show_ui(ui, |ui| {
                    for value in [
                        WindowsCaptureBorder::Default,
                        WindowsCaptureBorder::Show,
                        WindowsCaptureBorder::Hide,
                    ] {
                        ui.selectable_value(&mut settings.border, value, value.label());
                    }
                });
        });
        ui.end_row();

        // Secondary windows only apply to a window target.
        if is_window {
            ui.label("Secondary Windows");
            settings_value(ui, |ui| {
                egui::ComboBox::from_id_salt("windowscapture_secondary_windows")
                    .width(ui.available_width())
                    .selected_text(settings.secondary_windows.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            WindowsCaptureSecondaryWindows::Default,
                            WindowsCaptureSecondaryWindows::Include,
                            WindowsCaptureSecondaryWindows::Exclude,
                        ] {
                            ui.selectable_value(
                                &mut settings.secondary_windows,
                                value,
                                value.label(),
                            );
                        }
                    });
            });
            ui.end_row();
        }
    });

    return *settings != old;
}
