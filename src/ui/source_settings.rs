use crate::ui::side_panel::{settings_grid, settings_row};
use crate::sources::{SourceKind, SourceRuntimeConfig, SourceConfig};
use crate::sources::{TestSourceConfig, TestPattern, RadarDirection};
use crate::sources::{NdiSourceConfig, NdiReceiverBandwidth, NdiReceiverColorFormat};
use crate::sources::{OmtSourceConfig, OmtQuality};
use crate::sources::{DecklinkSourceConfig, DecklinkVideoConnection, DecklinkVideoConnections};
use crate::sources::VncSourceConfig;
use crate::sources::{RdpColorDepth, RdpSourceConfig};
#[cfg(target_os = "windows")]
use crate::sources::PixelFormat;
#[cfg(target_os = "macos")]
use crate::sources::SyphonSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::{AvFoundationSourceConfig, AvFoundationMode};
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
        SourceConfig::Omt(cfg) => omt_settings_ui(cfg, ui),
        SourceConfig::Decklink(cfg) => decklink_settings_ui(cfg, &source.runtime, ui),
        SourceConfig::Vnc(cfg) => vnc_settings_ui(cfg, ui),
        SourceConfig::Rdp(cfg) => rdp_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceConfig::Syphon(cfg) => syphon_settings_ui(cfg, ui),
        #[cfg(target_os = "macos")]
        SourceConfig::AvFoundation(cfg) => avfoundation_settings_ui(cfg, &source.runtime, ui),
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

fn test_settings_ui(cfg: &mut TestSourceConfig, ui: &mut egui::Ui) -> bool {
    let old = cfg.clone();
    settings_grid(ui, "test_settings_grid", |ui| {
        settings_row(ui, "Size", |ui| {
            ui.horizontal(|ui| {
                ui.label("W");
                ui.add(egui::DragValue::new(&mut cfg.width).range(100..=4096));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut cfg.height).range(100..=4096));
            });
        });

        settings_row(ui, "Pattern", |ui| {
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

        if let TestPattern::UvGradient { red, green, blue, rotation } = &mut cfg.pattern {
            settings_row(ui, "Channels", |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(red, "R");
                    ui.checkbox(green, "G");
                    ui.checkbox(blue, "B");
                });
            });

            settings_row(ui, "Rotation", |ui| {
                ui.add(egui::DragValue::new(rotation).range(0.0..=360.0).speed(1.0).suffix("°"));
            });
        }

        if let TestPattern::Grid { cols, rows, bg_color, line_color } = &mut cfg.pattern {
            settings_row(ui, "Grid", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Cols");
                    ui.add(egui::DragValue::new(cols).range(1..=50));
                    ui.label("Rows");
                    ui.add(egui::DragValue::new(rows).range(1..=50));
                });
            });

            color_picker_row(ui, "Primary Color", line_color);
            color_picker_row(ui, "Background", bg_color);
        }

        if let TestPattern::Radar { width, speed, direction, bg_color, line_color } = &mut cfg.pattern {
            settings_row(ui, "Width", |ui| {
                ui.add(egui::DragValue::new(width).range(1..=cfg.width));
            });

            settings_row(ui, "Speed", |ui| {
                ui.add(egui::DragValue::new(speed).range(0.0..=50.0).speed(0.5));
            });

            settings_row(ui, "Direction", |ui| {
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

            color_picker_row(ui, "Primary Color", line_color);
            color_picker_row(ui, "Background", bg_color);
        }

        settings_row(ui, "FPS", |ui| {
            ui.add(egui::DragValue::new(&mut cfg.fps).range(1.0..=240.0).speed(1.0));
        });

        settings_row(ui, "Cursor", |ui| {
            ui.checkbox(&mut cfg.cursor.enabled, "Enabled");
        });

        if cfg.cursor.enabled {
            settings_row(ui, "Cursor speed", |ui| {
                ui.horizontal(|ui| {
                    ui.label("X ");
                    ui.add(egui::DragValue::new(&mut cfg.cursor.speed_x).range(0.0..=100.0).speed(0.5));
                    ui.label("Y ");
                    ui.add(egui::DragValue::new(&mut cfg.cursor.speed_y).range(0.0..=100.0).speed(0.5));
                });
            });

            settings_row(ui, "Cursor width", |ui| {
                ui.add(egui::DragValue::new(&mut cfg.cursor.width).range(1..=100));
            });
        }
    });
    return *cfg != old;
}

fn ndi_format_label(cf: NdiReceiverColorFormat) -> String {
    return match cf {
        NdiReceiverColorFormat::BGRX_BGRA => "BGRX/BGRA".to_string(),
        NdiReceiverColorFormat::UYVY_BGRA => "UYVY/BGRA".to_string(),
        NdiReceiverColorFormat::RGBX_RGBA => "RGBX/RGBA".to_string(),
        NdiReceiverColorFormat::UYVY_RGBA => "UYVY/RGBA".to_string(),
        NdiReceiverColorFormat::Fastest => "Fastest".to_string(),
        NdiReceiverColorFormat::Best => "Best".to_string(),
        _ => format!("{:?}", cf),
    };
}

fn ndi_settings_ui(cfg: &mut NdiSourceConfig, ui: &mut egui::Ui) -> bool {
    let old = cfg.clone();
    settings_grid(ui, "ndi_settings_grid", |ui| {
        settings_row(ui, "Bandwidth", |ui| {
            egui::ComboBox::from_id_salt("ndi_bw")
                .width(ui.available_width())
                .selected_text(format!("{:?}", cfg.bandwidth))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.bandwidth, NdiReceiverBandwidth::Highest, "Highest");
                    ui.selectable_value(&mut cfg.bandwidth, NdiReceiverBandwidth::Lowest, "Lowest");
                });
        });

        settings_row(ui, "Pixel Format", |ui| {
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
    });
    return *cfg != old;
}

fn omt_settings_ui(cfg: &mut OmtSourceConfig, ui: &mut egui::Ui) -> bool {
    let old = cfg.clone();
    settings_grid(ui, "omt_settings_grid", |ui| {
        settings_row(ui, "Quality", |ui| {
            egui::ComboBox::from_id_salt("omt_quality")
                .width(ui.available_width())
                .selected_text(format!("{:?}", cfg.quality))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.quality, OmtQuality::Default, "Default");
                    ui.selectable_value(&mut cfg.quality, OmtQuality::Low, "Low");
                    ui.selectable_value(&mut cfg.quality, OmtQuality::Medium, "Medium");
                    ui.selectable_value(&mut cfg.quality, OmtQuality::High, "High");
                });
        });

        settings_row(ui, "Preview", |ui| {
            ui.checkbox(&mut cfg.preview, "");
        });
    });
    return *cfg != old;
}

fn decklink_settings_ui(
    cfg: &mut DecklinkSourceConfig,
    runtime: &SourceRuntimeConfig,
    ui: &mut egui::Ui,
) -> bool {
    let old = cfg.clone();
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
        settings_row(ui, "Connection", |ui| {
            egui::ComboBox::from_id_salt("decklink_conn")
                .width(ui.available_width())
                .selected_text(cfg.connection.label())
                .show_ui(ui, |ui| {
                    for conn in &available {
                        ui.selectable_value(&mut cfg.connection, *conn, conn.label());
                    }
                });
        });
    });
    return *cfg != old;
}

#[cfg(target_os = "macos")]
fn syphon_settings_ui(_cfg: &mut SyphonSourceConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    return false;
}

#[cfg(target_os = "macos")]
impl ModeChoice for AvFoundationMode {
    fn from_dims(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        return Self { width, height, fps_num, fps_den };
    }

    fn dims(self) -> (u32, u32, u32, u32) {
        return (self.width, self.height, self.fps_num, self.fps_den);
    }

    fn text(&self) -> String {
        return self.label();
    }
}

#[cfg(target_os = "macos")]
fn avfoundation_settings_ui(
    cfg: &mut AvFoundationSourceConfig,
    runtime: &SourceRuntimeConfig,
    ui: &mut egui::Ui,
) -> bool {
    // modes are enumerated by the capture thread on open; absent a live source
    // (or before enumeration finishes) only "Auto" is offered.
    let mut modes = match runtime {
        SourceRuntimeConfig::AvFoundation { modes } => modes.lock().unwrap().clone(),
        _ => Vec::new(),
    };
    modes.sort_unstable();
    modes.dedup();
    let old = cfg.clone();

    settings_grid(ui, "avfoundation_settings_grid", |ui| {
        settings_row(ui, "Mode", |ui| {
            mode_row(
                ui,
                "avfoundation_mode",
                [&mut cfg.width, &mut cfg.height, &mut cfg.fps_num, &mut cfg.fps_den],
                &modes,
            );
        });

        settings_row(ui, "Drop late frames", |ui| {
            ui.checkbox(&mut cfg.drop_late_frames, "Enabled");
        });
    });

    return *cfg != old;
}

#[cfg(target_os = "macos")]
fn screencapturekit_settings_ui(cfg: &mut ScreenCaptureKitSourceConfig, ui: &mut egui::Ui) -> bool {
    let settings = cfg.settings_mut();
    let old = settings.clone();

    settings_grid(ui, "screencapturekit_settings_grid", |ui| {
        settings_row(ui, "Cursor", |ui| {
            egui::ComboBox::from_id_salt("screencapturekit_cursor")
                .width(ui.available_width())
                .selected_text(if settings.show_cursor { "Show" } else { "Hide" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.show_cursor, true, "Show");
                    ui.selectable_value(&mut settings.show_cursor, false, "Hide");
                });
        });

        settings_row(ui, "Max FPS", |ui| max_fps_drag(ui, &mut settings.max_fps));
    });

    return *settings != old;
}

#[cfg(target_os = "windows")]
fn spout_settings_ui(_cfg: &mut SpoutSourceConfig, _ui: &mut egui::Ui) -> bool {
    // No tunables yet
    return false;
}

#[cfg(target_os = "windows")]
impl ModeChoice for MediaFoundationMode {
    fn from_dims(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        return Self { width, height, fps_num, fps_den };
    }

    fn dims(self) -> (u32, u32, u32, u32) {
        return (self.width, self.height, self.fps_num, self.fps_den);
    }

    fn text(&self) -> String {
        return self.label();
    }
}

#[cfg(target_os = "windows")]
fn mediafoundation_settings_ui(cfg: &mut MediaFoundationSourceConfig, runtime: &SourceRuntimeConfig, ui: &mut egui::Ui) -> bool {
    // modes are enumerated by the capture thread on open; absent a live source
    // (or before enumeration finishes) only "Auto" is offered.
    let mut modes = match runtime {
        SourceRuntimeConfig::MediaFoundation { modes, .. } => modes.lock().unwrap().clone(),
        _ => Vec::new(),
    };
    modes.sort_unstable();
    modes.dedup();
    let old = cfg.clone();

    settings_grid(ui, "mediafoundation_settings_grid", |ui| {
        settings_row(ui, "Mode", |ui| {
            mode_row(
                ui,
                "mediafoundation_mode",
                [&mut cfg.width, &mut cfg.height, &mut cfg.fps_num, &mut cfg.fps_den],
                &modes,
            );
        });

        settings_row(ui, "Pixel Format", |ui| {
            egui::ComboBox::from_id_salt("mediafoundation_pixel_format")
                .width(ui.available_width())
                .selected_text(pixel_format_label(cfg.pixel_format))
                .show_ui(ui, |ui| {
                    for choice in PIXEL_FORMATS {
                        if ui.selectable_label(cfg.pixel_format == choice, pixel_format_label(choice)).clicked() {
                            cfg.pixel_format = choice;
                        }
                    }
                });
        });
    });

    return *cfg != old;
}

/// Formats the compositor can render, plus Auto (the device default).
#[cfg(target_os = "windows")]
const PIXEL_FORMATS: [Option<PixelFormat>; 5] = [
    None,
    Some(PixelFormat::Bgra8),
    Some(PixelFormat::Uyvy422),
    Some(PixelFormat::Yuy2),
    Some(PixelFormat::Nv12),
];

/// Display label for a requested capture format; `None` is Auto.
#[cfg(target_os = "windows")]
fn pixel_format_label(format: Option<PixelFormat>) -> &'static str {
    return match format {
        None => AUTO_MODE_LABEL,
        Some(PixelFormat::Bgra8) => "RGB32",
        Some(PixelFormat::Uyvy422) => "UYVY 4:2:2",
        Some(PixelFormat::Yuy2) => "YUY2 4:2:2",
        Some(PixelFormat::Nv12) => "NV12 4:2:0",
        Some(PixelFormat::Rgba8) => "RGBA8",
    };
}

#[cfg(target_os = "windows")]
impl ModeChoice for DirectShowMode {
    fn from_dims(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        return Self { width, height, fps_num, fps_den };
    }

    fn dims(self) -> (u32, u32, u32, u32) {
        return (self.width, self.height, self.fps_num, self.fps_den);
    }

    fn text(&self) -> String {
        return self.label();
    }
}

#[cfg(target_os = "windows")]
fn directshow_settings_ui(cfg: &mut DirectShowSourceConfig, runtime: &SourceRuntimeConfig, ui: &mut egui::Ui) -> bool {
    // modes are enumerated by the capture thread on open; absent a live source
    // (or before enumeration finishes) only "Auto" is offered.
    let mut modes = match runtime {
        SourceRuntimeConfig::DirectShow { modes, .. } => modes.lock().unwrap().clone(),
        _ => Vec::new(),
    };
    modes.sort_unstable();
    modes.dedup();
    let old = cfg.clone();

    settings_grid(ui, "directshow_settings_grid", |ui| {
        settings_row(ui, "Mode", |ui| {
            mode_row(
                ui,
                "directshow_mode",
                [&mut cfg.width, &mut cfg.height, &mut cfg.fps_num, &mut cfg.fps_den],
                &modes,
            );
        });

        settings_row(ui, "Pixel Format", |ui| {
            egui::ComboBox::from_id_salt("directshow_pixel_format")
                .width(ui.available_width())
                .selected_text(pixel_format_label(cfg.pixel_format))
                .show_ui(ui, |ui| {
                    for choice in PIXEL_FORMATS {
                        if ui.selectable_label(cfg.pixel_format == choice, pixel_format_label(choice)).clicked() {
                            cfg.pixel_format = choice;
                        }
                    }
                });
        });
    });

    return *cfg != old;
}

#[cfg(target_os = "windows")]
fn windowscapture_settings_ui(cfg: &mut WindowsCaptureSourceConfig, ui: &mut egui::Ui) -> bool {
    let is_window = cfg.is_window();
    let settings = cfg.settings_mut();
    let old = settings.clone();

    settings_grid(ui, "windowscapture_settings_grid", |ui| {
        settings_row(ui, "Cursor", |ui| {
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

        settings_row(ui, "Borders", |ui| {
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

        // Secondary windows only apply to a window target.
        if is_window {
            settings_row(ui, "Secondary Windows", |ui| {
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
        }

        settings_row(ui, "Max FPS", |ui| max_fps_drag(ui, &mut settings.max_fps));
    });

    return *settings != old;
}

fn vnc_settings_ui(cfg: &mut VncSourceConfig, ui: &mut egui::Ui) -> bool {
    let mut committed = false;
    settings_grid(ui, "vnc_settings_grid", |ui| {
        committed |= endpoint_rows(ui, "vnc", &mut cfg.host, &mut cfg.port);

        settings_row(ui, "Password", |ui| {
            committed |= password_row(ui, "vnc_password", &mut cfg.password);
        });

        settings_row(ui, "Keep awake", |ui| {
            committed |= ui.checkbox(&mut cfg.keep_awake, "").changed();
        });
    });
    return committed;
}

fn rdp_settings_ui(cfg: &mut RdpSourceConfig, ui: &mut egui::Ui) -> bool {
    let mut committed = false;
    settings_grid(ui, "rdp_settings_grid", |ui| {
        committed |= endpoint_rows(ui, "rdp", &mut cfg.host, &mut cfg.port);

        settings_row(ui, "Username", |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut cfg.username)
                    .hint_text("user or user@domain")
                    .desired_width(ui.available_width()),
            );
            committed |= commit_field_edit(ui, "rdp_username", response.changed(), response.lost_focus(), false);
        });

        settings_row(ui, "Domain", |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut cfg.domain)
                    .hint_text("optional")
                    .desired_width(ui.available_width()),
            );
            committed |= commit_field_edit(ui, "rdp_domain", response.changed(), response.lost_focus(), false);
        });

        settings_row(ui, "Password", |ui| {
            committed |= password_row(ui, "rdp_password", &mut cfg.password);
        });

        settings_row(ui, "Size", |ui| {
            // A `DragValue` never takes focus, so the release is the commit.
            ui.horizontal(|ui| {
                ui.label("W");
                let width = ui.add(egui::DragValue::new(&mut cfg.desktop_width).range(16..=8192));
                ui.label("H");
                let height = ui.add(egui::DragValue::new(&mut cfg.desktop_height).range(16..=8192));
                committed |= commit_field_edit(
                    ui,
                    "rdp_size",
                    width.changed() || height.changed(),
                    width.lost_focus() || height.lost_focus(),
                    width.drag_stopped() || height.drag_stopped(),
                );
            });
        });

        settings_row(ui, "Color depth", |ui| {
            egui::ComboBox::from_id_salt("rdp_color_depth")
                .width(ui.available_width())
                .selected_text(cfg.color_depth.label())
                .show_ui(ui, |ui| {
                    for depth in [RdpColorDepth::Bpp16, RdpColorDepth::Bpp32] {
                        committed |= ui
                            .selectable_value(&mut cfg.color_depth, depth, depth.label())
                            .changed();
                    }
                });
        });
    });
    return committed;
}

// Helpers
/// A device capture mode, as the device pickers need it: build it from the four
/// persisted fields, read those fields back, and label it.
#[cfg(any(target_os = "macos", target_os = "windows"))]
trait ModeChoice: Copy + PartialEq {
    fn from_dims(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self;
    fn dims(self) -> (u32, u32, u32, u32);
    fn text(&self) -> String;
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
const AUTO_MODE_LABEL: &str = "Auto (device default)";

/// Mode picker shared by every device source: "Auto" plus the modes the device
/// advertised, written straight into the four config fields.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn mode_row<M: ModeChoice>(ui: &mut egui::Ui, id_salt: &str, fields: [&mut u32; 4], modes: &[M]) {
    let [width, height, fps_num, fps_den] = fields;
    let current = if *width > 0 && *height > 0 && *fps_num > 0 && *fps_den > 0 {
        Some(M::from_dims(*width, *height, *fps_num, *fps_den))
    } else {
        None
    };
    let text = current
        .as_ref()
        .map(|mode| mode.text())
        .unwrap_or_else(|| AUTO_MODE_LABEL.to_string());

    egui::ComboBox::from_id_salt(id_salt)
        .width(ui.available_width())
        .height(1000.0)
        .selected_text(text)
        .truncate()
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), AUTO_MODE_LABEL).clicked() {
                *width = 0;
                *height = 0;
                *fps_num = 0;
                *fps_den = 0;
            }
            for mode in modes {
                if ui.selectable_label(current == Some(*mode), mode.text()).clicked() {
                    let (w, h, n, d) = mode.dims();
                    *width = w;
                    *height = h;
                    *fps_num = n;
                    *fps_den = d;
                }
            }
            if modes.is_empty() {
                ui.weak("(no modes reported)");
            }
        });
}

/// Host and port rows shared by the VNC and RDP settings.
fn endpoint_rows(ui: &mut egui::Ui, id_prefix: &str, host: &mut String, port: &mut u16) -> bool {
    let mut committed = false;
    settings_row(ui, "Host", |ui| {
        let response = ui.add(
            egui::TextEdit::singleline(host)
                .hint_text("hostname or IP")
                .desired_width(ui.available_width()),
        );
        committed |= commit_field_edit(
            ui,
            &format!("{id_prefix}_host"),
            response.changed(),
            response.lost_focus(),
            false,
        );
    });

    settings_row(ui, "Port", |ui| {
        // The value only lands in the config when the edit finishes, so a
        // half-typed port is never dialled.
        let response = ui.add(
            egui::DragValue::new(port)
                .range(1..=65535)
                .update_while_editing(false),
        );
        committed |= commit_field_edit(
            ui,
            &format!("{id_prefix}_port"),
            response.changed(),
            response.lost_focus(),
            response.drag_stopped(),
        );
    });

    return committed;
}

fn color_picker_row(ui: &mut egui::Ui, label: &str, color: &mut [u8; 3]) {
    settings_row(ui, label, |ui| {
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
}

#[allow(dead_code)]
fn max_fps_drag(ui: &mut egui::Ui, max_fps: &mut u32) {
    ui.add(
        egui::DragValue::new(max_fps)
            .range(0..=240)
            .update_while_editing(false)
            .custom_formatter(|n, _| {
                if n as u32 == 0 {
                    "Uncapped".to_string()
                } else {
                    format!("{}", n as u32)
                }
            })
            .custom_parser(|s| {
                if s.eq_ignore_ascii_case("uncapped") {
                    Some(0.0)
                } else {
                    s.trim().parse::<f64>().ok()
                }
            }),
    );
}

/// Avoid having changed flag mid-edits
fn edit_committed(dirty: bool, lost_focus: bool, drag_stopped: bool) -> bool {
    return dirty && (lost_focus || drag_stopped);
}

/// Applies an edit to a field only when it is committed: Enter, clicking away or the end of a drag
fn commit_field_edit(
    ui: &mut egui::Ui,
    id: &str,
    changed: bool,
    lost_focus: bool,
    drag_stopped: bool,
) -> bool {
    let state_id = ui.make_persistent_id(id);
    let mut dirty = ui.data_mut(|data| *data.get_temp_mut_or_default::<bool>(state_id));
    if changed {
        dirty = true;
    }
    // A singleline `TextEdit` surrenders focus on Enter as well as on blur; a
    // `DragValue` never takes focus, so its drag release is the commit instead.
    let committed = edit_committed(dirty, lost_focus, drag_stopped);
    if committed {
        dirty = false;
    }
    ui.data_mut(|data| data.insert_temp(state_id, dirty));
    return committed;
}

/// Masked password field with a reveal toggle. The value is applied on Enter or when focus leaves
fn password_row(ui: &mut egui::Ui, id: &str, password: &mut String) -> bool {
    let revealed_id = ui.make_persistent_id((id, "revealed"));
    let revealed = ui.data_mut(|data| *data.get_temp_mut_or_default::<bool>(revealed_id));
    let width = (ui.available_width() - 30.0).max(60.0);
    let response = ui.add(
        egui::TextEdit::singleline(password)
            .password(!revealed)
            .desired_width(width),
    );
    let committed = commit_field_edit(ui, id, response.changed(), response.lost_focus(), false);
    let icon = if revealed { "\u{1f648}" } else { "\u{1f441}" };
    let hint = if revealed { "Hide password" } else { "Show password" };
    if ui.button(icon).on_hover_text(hint).clicked() {
        ui.data_mut(|data| data.insert_temp(revealed_id, !revealed));
    }
    return committed;
}


#[cfg(test)]
mod tests {
    use super::*;

    /// An edit applies once the user is done with it, and never on the keystroke
    /// that caused it: a connecting field would otherwise restart the source
    /// once per character.
    #[test]
    fn edit_applies_only_once_it_is_finished() {
        // Nothing edited yet.
        assert!(!edit_committed(false, true, false));
        assert!(!edit_committed(false, false, true));

        // Typing: changed, but still focused.
        assert!(!edit_committed(true, false, false));

        // Enter or clicking away.
        assert!(edit_committed(true, true, false));

        // Releasing a drag: a `DragValue` never takes focus.
        assert!(edit_committed(true, false, true));
    }
}
