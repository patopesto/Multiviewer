use egui::{Align, Grid, InnerResponse, Layout, ScrollArea, Separator, Ui};

use crate::APP_NAME;
use crate::ui::source_settings;
use crate::config::{BorderVisibility, LabelPosition, LabelVisibility, SourceBorderVisibility, SourceLabelVisibility, TextureMode};
use crate::engine::Engine;
use crate::sources::{Protocol, SourceConfig, SourceKey, SourceRef, SourceRegistry, SourceKind, SourceStats, OutputConfig};
use crate::sources::{
    DecklinkMode, DecklinkOutputConfig, DecklinkOutputMode, DecklinkOutputPort, NdiOutputConfig,
    OmtOutputConfig,
};
use crate::sources::VncSourceConfig;
use crate::sources::RdpSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::format_syphon_label;
#[cfg(target_os = "macos")]
use crate::sources::SyphonOutputConfig;
#[cfg(target_os = "windows")]
use crate::sources::SpoutOutputConfig;

pub fn draw(ui: &mut egui::Ui, engine: &mut Engine) {
    egui::Panel::left("panel")
        .default_size(280.0)
        .min_size(200.0)
        .max_size(400.0)
        .show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                collapsable_section(ui, "Global Settings", true, true, |ui| {
                    draw_global_section(ui, engine);
                });

                ui.separator();
                collapsable_section(ui, "Sources", true, true, |ui| {
                    draw_sources_section(ui, engine);
                });

                if let Some(selected_uuid) = engine.selected_source_id.clone() {
                    draw_source_properties_section(ui, engine, &selected_uuid);
                }

                // Cleanup all orphaned sources (Test, NDI, DeckLink)
                engine.cleanup_orphaned_sources();
            });
        });
}

fn draw_global_section(ui: &mut egui::Ui, engine: &mut Engine) {
    let mut decklink_restart_idx: Option<usize> = None;

    settings_grid(ui, "global_settings_grid", |ui| {
        settings_row(ui, "Canvas", |ui| {
            ui.horizontal(|ui| {
                ui.label("W");
                if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.width).range(100..=7680).update_while_editing(false)).changed() {
                    engine.dirty = true;
                }
                ui.label("H");
                if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.height).range(100..=7680).update_while_editing(false)).changed() {
                    engine.dirty = true;
                }
            });
        });
    });

    subsection_separator(ui);
    collapsable_section(ui, "Labels", false, false, |ui| {
        let label = &mut engine.cfg.canvas.label;
        settings_grid(ui, "labels_grid", |ui| {
            settings_row(ui, "Visibility", |ui| {
                egui::ComboBox::from_id_salt("label_visibility")
                    .width(ui.available_width())
                    .selected_text(label.visibility.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut label.visibility, LabelVisibility::Show, "Show").changed() {
                            engine.dirty = true;
                        }
                        if ui.selectable_value(&mut label.visibility, LabelVisibility::Hide, "Hide").changed() {
                            engine.dirty = true;
                        }
                    });
            });

            settings_row(ui, "Size", |ui| {
                if ui.add(egui::DragValue::new(&mut label.size).range(1.0..=256.0).speed(1.0).suffix(" px")).changed() {
                    engine.dirty = true;
                }
            });

            settings_row(ui, "Position", |ui| {
                egui::ComboBox::from_id_salt("label_position")
                    .width(ui.available_width())
                    .selected_text(label.position.label())
                    .show_ui(ui, |ui| {
                        for position in LabelPosition::all() {
                            if ui
                                .selectable_value(&mut label.position, *position, position.label())
                                .changed()
                            {
                                engine.dirty = true;
                            }
                        }
                    });
            });

            color_picker_rgba_row(ui, "Text Color", &mut label.text_color);
            color_picker_rgba_row(ui, "Background", &mut label.background_color);
        });
        subsection_separator(ui);
    });

    collapsable_section(ui, "Borders", false, false, |ui| {
        let border = &mut engine.cfg.canvas.border;
        settings_grid(ui, "borders_grid", |ui| {
            settings_row(ui, "Visibility", |ui| {
                egui::ComboBox::from_id_salt("global_source_borders")
                    .width(ui.available_width())
                    .selected_text(border.visibility.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut border.visibility, BorderVisibility::Show, "Show").clicked() {
                            engine.dirty = true;
                        }
                        if ui.selectable_value(&mut border.visibility, BorderVisibility::Hide, "Hide").clicked() {
                            engine.dirty = true;
                        }
                    });
            });

            settings_row(ui, "Border Color", |ui| {
                let mut color_f32 = [
                    border.color[0] as f32 / 255.0,
                    border.color[1] as f32 / 255.0,
                    border.color[2] as f32 / 255.0,
                ];
                if ui.color_edit_button_rgb(&mut color_f32).changed() {
                    border.color = [
                        (color_f32[0] * 255.0) as u8,
                        (color_f32[1] * 255.0) as u8,
                        (color_f32[2] * 255.0) as u8,
                        255, // lock alpha for now, until we figure out blending modes on the canvas
                    ];
                    engine.dirty = true;
                }
            });

            settings_row(ui, "Border Size", |ui| {
                if ui.add(egui::DragValue::new(&mut border.width).range(0.0..=100.0)).changed() {
                    engine.dirty = true;
                }
            });
        });
        subsection_separator(ui);
    });

    collapsable_section(ui, "Outputs", false, false, |ui| {
        settings_grid(ui, "output_grid", |ui| {
            output_enable_row(ui, engine, Protocol::Ndi, |_| {
                OutputConfig::Ndi(NdiOutputConfig {
                    sender_name: APP_NAME.to_string(),
                })
            });

            output_enable_row(ui, engine, Protocol::Omt, |_| {
                OutputConfig::Omt(OmtOutputConfig {
                    sender_name: APP_NAME.to_string(),
                })
            });

            #[cfg(target_os = "macos")]
            output_enable_row(ui, engine, Protocol::Syphon, |name| {
                OutputConfig::Syphon(SyphonOutputConfig {
                    server_name: name.to_string(),
                })
            });

            #[cfg(target_os = "windows")]
            output_enable_row(ui, engine, Protocol::Spout, |_| {
                OutputConfig::Spout(SpoutOutputConfig {
                    sender_name: APP_NAME.to_string(),
                })
            });

            output_enable_row(ui, engine, Protocol::Decklink, |_| {
                OutputConfig::Decklink(DecklinkOutputConfig::default())
            });

            // Worker errors (device busy, driver missing) sit next to the toggle
            // so a silently dead output is visible.
            let decklink_error = engine
                .cfg
                .canvas
                .outputs
                .iter()
                .find(|o| o.protocol == Protocol::Decklink)
                .and_then(|o| engine.output_registry.get(&o.uuid))
                .and_then(|output| {
                    let stats = output.stats();
                    let error = stats.lock().unwrap().error.clone();
                    return error;
                });
            if let Some(error) = decklink_error {
                settings_row(ui, "Status", |ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, error);
                });
            }

            // Device / mode selection only when a DeckLink output exists.
            if let Some(idx) = engine
                .cfg
                .canvas
                .outputs
                .iter()
                .position(|o| o.protocol == Protocol::Decklink)
            {
                let outputs = engine
                    .decklink
                    .as_ref()
                    .map(|d| d.list_outputs())
                    .unwrap_or_default();

                let (current_device, current_mode) = match &engine.cfg.canvas.outputs[idx].config {
                    OutputConfig::Decklink(c) => (c.device_name.clone(), c.display_mode),
                    _ => (String::new(), DecklinkMode::Hd1080p6000),
                };

                settings_row(ui, "Decklink Device", |ui| {
                    let combo = egui::ComboBox::from_id_salt("decklink_output_device")
                        .width(ui.available_width())
                        .selected_text(if current_device.is_empty() {
                            "(select device)".to_string()
                        } else {
                            current_device.clone()
                        })
                        .show_ui(ui, |ui| {
                            for port in &outputs {
                                if ui
                                    .selectable_label(current_device == port.name, &port.name)
                                    .clicked()
                                {
                                    if let OutputConfig::Decklink(config) =
                                        &mut engine.cfg.canvas.outputs[idx].config
                                    {
                                        config.device_name = port.name.clone();
                                        if let Some(mode) =
                                            preferred_output_mode(port, config.display_mode)
                                        {
                                            config.display_mode = mode.mode;
                                            config.width = mode.width;
                                            config.height = mode.height;
                                            config.fps = mode.fps;
                                        }
                                    }
                                    decklink_restart_idx = Some(idx);
                                    engine.dirty = true;
                                }
                            }
                        });
                    if combo.response.clicked()
                        && let Some(discovery) = engine.decklink.as_ref()
                    {
                        discovery.request_refresh();
                    }
                });

                let modes = outputs
                    .iter()
                    .find(|p| p.name == current_device)
                    .map(|p| p.modes.clone())
                    .unwrap_or_default();
                let selected_label = if current_device.is_empty() {
                    "Select device".to_string()
                } else {
                    modes
                        .iter()
                        .find(|m| m.mode == current_mode)
                        .map(|m| m.name.to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                };

                settings_row(ui, "Decklink Mode", |ui| {
                    ui.add_enabled_ui(!current_device.is_empty(), |ui| {
                        egui::ComboBox::from_id_salt("decklink_output_mode")
                            .width(ui.available_width())
                            .selected_text(selected_label)
                            .show_ui(ui, |ui| {
                                for m in &modes {
                                    let label = m.name.to_string();
                                    if ui.selectable_label(current_mode == m.mode, label).clicked() {
                                        if let OutputConfig::Decklink(config) =
                                            &mut engine.cfg.canvas.outputs[idx].config
                                        {
                                            config.display_mode = m.mode;
                                            config.width = m.width;
                                            config.height = m.height;
                                            config.fps = m.fps;
                                        }
                                        decklink_restart_idx = Some(idx);
                                        engine.dirty = true;
                                    }
                                }
                            });
                    });
                });
            }
        });

        if !engine.cfg.canvas.outputs.is_empty() {
            collapsable_section(ui, "Output Stats", false, false, |ui| {
                settings_grid(ui, "output_stats_grid", |ui| {
                    for output in &engine.cfg.canvas.outputs {
                        let Some(ok) = engine.output_registry.get(&output.uuid) else {
                            continue;
                        };
                        let stats = ok.stats();
                        let (sent, dropped, fps, error) = {
                            let s = stats.lock().unwrap();
                            (
                                s.frames_sent,
                                s.frames_dropped,
                                s.computed_fps,
                                s.error.clone(),
                            )
                        };
                        settings_row(ui, &format!("{} Stats", output.protocol.label()), |ui| {
                            let mut line = format!(
                                "{} sent \u{2022} {} dropped \u{2022} {:.1} fps",
                                sent, dropped, fps
                            );
                            if let Some(error) = &error {
                                line.push_str(" \u{2022} ");
                                line.push_str(error);
                            }
                            ui.label(line);
                        });
                    }
                });
            });
        }
    });

    if let Some(idx) = decklink_restart_idx {
        let id = engine.cfg.canvas.outputs[idx].uuid.clone();
        engine.restart_output(&id);
    }
}


fn draw_sources_section(ui: &mut egui::Ui, engine: &mut Engine) {
    // Sources list with drag-and-drop reordering.
    let rows: Vec<(usize, String, String)> = engine
        .cfg
        .canvas
        .sources
        .iter()
        .enumerate()
        .map(|(i, l)| (i, l.uuid.clone(), l.name.clone()))
        .collect();

    let previous_selected = engine.selected_source_id.clone();

    const MAX_VISIBLE_SOURCE_ROWS: f32 = 18.0;
    let row_height = ui.spacing().interact_size.y;
    egui::ScrollArea::vertical()
        .max_height(row_height * MAX_VISIBLE_SOURCE_ROWS)
        .show(ui, |ui| {
            let mut selected_row_rect: Option<egui::Rect> = None;

            for (index, uuid, name) in rows {
                // Use a plain frame as the drop zone so egui doesn't tint other rows while dragging.
                let drop_zone_response = egui::Frame::new().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Custom drag handle (drawn, not a font glyph) so it always renders.
                        ui.dnd_drag_source(
                            egui::Id::new("source_drag").with(&uuid),
                            uuid.clone(),
                            |ui| {
                                let size = egui::vec2(12.0, 16.0);
                                let (rect, response) =
                                    ui.allocate_exact_size(size, egui::Sense::hover());
                                let painter = ui.painter();
                                let color = ui.visuals().text_color();
                                let center_y = rect.center().y;
                                for dy in [-3.0_f32, 0.0, 3.0] {
                                    let y = center_y + dy;
                                    let x_range = rect.x_range().shrink(2.0);
                                    painter.hline(x_range, y, egui::Stroke::new(1.5, color));
                                }
                                response.on_hover_cursor(egui::CursorIcon::Grab)
                            },
                        );

                        // Full-width, left-aligned selectable label.
                        let selected = engine.selected_source_id.as_deref() == Some(&uuid);
                        let available_width = ui.available_width();
                        let label = egui::Button::selectable(selected, name.to_string())
                            .min_size(egui::vec2(available_width, 0.0))
                            .right_text("")
                            .truncate();
                        let row_response = ui.add(label);
                        if row_response.clicked() {
                            engine.selected_source_id = Some(uuid.clone());
                        }
                        if selected {
                            selected_row_rect = Some(row_response.rect);
                        }
                    });
                });

                let dropped = drop_zone_response.response.dnd_release_payload::<String>();
                let row_rect = drop_zone_response.response.rect;

                // Drop-line indicator while dragging over this row.
                if let Some(pointer_pos) = ui.ctx().pointer_interact_pos()
                    && row_rect.contains(pointer_pos)
                    && let Some(payload) = egui::DragAndDrop::payload::<String>(ui.ctx())
                    && payload.as_ref() != &uuid
                {
                    let line_y = if pointer_pos.y < row_rect.center().y {
                        row_rect.top()
                    } else {
                        row_rect.bottom()
                    };
                    ui.painter().hline(
                        row_rect.x_range(),
                        line_y,
                        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                    );
                }

                if let Some(payload) = dropped
                    && payload.as_ref() != &uuid
                    && let Some(from_index) = engine
                        .cfg
                        .canvas
                        .sources
                        .iter()
                        .position(|l| l.uuid == *payload)
                {
                    let pointer_y = ui
                        .ctx()
                        .pointer_interact_pos()
                        .map(|p| p.y)
                        .unwrap_or(row_rect.center().y);
                    let to_index = if pointer_y < row_rect.center().y {
                        if from_index < index {
                            index.saturating_sub(1)
                        } else {
                            index
                        }
                    } else {
                        if from_index < index { index } else { index + 1 }
                    };
                    engine.move_source(from_index, to_index);
                }
            }

            // Keep the selected row visible when selection changes (e.g. via global shortcuts).
            if engine.selected_source_id != previous_selected && let Some(rect) = selected_row_rect {
                ui.scroll_to_rect(rect, None);
            }
        });

    ui.horizontal(|ui| {
        if ui.button("+ Add Source").clicked() {
            let uuid = engine.add_source();
            engine.selected_source_id = Some(uuid);
        }
        let is_source_selected = engine.selected_source_id.is_some();
        if ui.add_enabled(is_source_selected, egui::Button::new("- Delete Source")).clicked()
            && let Some(uuid) = engine.selected_source_id.take()
        {
            engine.remove_source(&uuid);
        }
    });
}

fn draw_source_properties_section(ui: &mut egui::Ui, engine: &mut Engine, selected_uuid: &str) {
    let mut new_test_source = false;
    let mut new_vnc_source = false;
    let mut new_rdp_source = false;
    let mut new_connect: Option<(Protocol, String)> = None;
    let mut selected_source: Option<SourceRef> = None;
    let mut protocol_changed = false;
    let mut restart_key: Option<SourceKey> = None;
    let mut config_sync: Option<SourceConfig> = None;

    if let Some(source) = engine
        .cfg
        .canvas
        .sources
        .iter_mut()
        .find(|l| l.uuid == selected_uuid)
    {
        ui.separator();
        collapsable_section(ui, "Properties", true, true, |ui| {
            settings_grid(ui, "source_properties_grid", |ui| {
                settings_row(ui, "Name", |ui| {
                    let text_edit = egui::TextEdit::singleline(&mut source.name)
                        .desired_width(ui.available_width());
                    if ui.add(text_edit).changed() {
                        engine.dirty = true;
                    }
                });

                settings_row(ui, "Protocol", |ui| {
                    egui::ComboBox::from_id_salt("source_protocol")
                        .width(ui.available_width())
                        .selected_text(source.protocol.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_value(&mut source.protocol, Protocol::Test, Protocol::Test.label()).clicked(){
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut source.protocol, Protocol::Ndi, Protocol::Ndi.label()).clicked(){
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut source.protocol, Protocol::Omt, Protocol::Omt.label()).clicked() {
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut source.protocol, Protocol::Decklink, Protocol::Decklink.label()).clicked() {
                                protocol_changed = true;
                            }

                            #[cfg(target_os = "macos")]
                            if ui.selectable_value(&mut source.protocol, Protocol::Syphon, Protocol::Syphon.label()).clicked(){
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "macos")]
                            if ui.selectable_value(&mut source.protocol, Protocol::AvFoundation, Protocol::AvFoundation.label()).clicked(){
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "macos")]
                            if ui.selectable_value(&mut source.protocol, Protocol::ScreenCaptureKit, Protocol::ScreenCaptureKit.label()).clicked(){
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "windows")]
                            if ui.selectable_value(&mut source.protocol, Protocol::Spout, Protocol::Spout.label()).clicked(){
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "windows")]
                            if ui.selectable_value(&mut source.protocol, Protocol::MediaFoundation, Protocol::MediaFoundation.label()).clicked(){
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "windows")]
                            if ui.selectable_value(&mut source.protocol, Protocol::DirectShow, Protocol::DirectShow.label()).clicked(){
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "windows")]
                            if ui.selectable_value(&mut source.protocol, Protocol::WindowsCapture, Protocol::WindowsCapture.label()).clicked(){
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut source.protocol, Protocol::Vnc, Protocol::Vnc.label()).clicked() {
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut source.protocol, Protocol::Rdp, Protocol::Rdp.label()).clicked() {
                                protocol_changed = true;
                            }
                        });
                });

                // Source dropdown
                settings_row(ui, "Source", |ui| {
                    let current = source.source_ref.as_deref().unwrap_or("");
                    match source.protocol {
                        Protocol::Test => {
                            let picker = source_picker(
                                ui,
                                "test_source",
                                current,
                                connected_sources(&engine.registry, Protocol::Test),
                                Vec::new(),
                                Some("+ New Test Source".to_string()),
                                false,
                            );
                            selected_source = picker.selected;
                            new_test_source |= picker.new_entry;
                        }
                        Protocol::Ndi => {
                            let discovered = engine.ndi.as_ref().map(|d| d.list()).unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "ndi_source",
                                current,
                                connected_sources(&engine.registry, Protocol::Ndi),
                                vec![PickerGroup::flat(
                                    discovered.iter().map(|src| PickerItem::plain(&src.name)),
                                )],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::Ndi, id));
                            }
                        }
                        Protocol::Omt => {
                            let discovered = engine.omt.as_ref().map(|d| d.list()).unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "omt_source",
                                current,
                                connected_sources(&engine.registry, Protocol::Omt),
                                vec![PickerGroup::flat(discovered.iter().map(|addr| {
                                    PickerItem::plain(&addr.instance_name())
                                }))],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::Omt, id));
                            }
                        }
                        Protocol::Decklink => {
                            let discovered =
                                engine.decklink.as_ref().map(|d| d.list()).unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "decklink_source",
                                current,
                                connected_sources(&engine.registry, Protocol::Decklink),
                                vec![PickerGroup::flat(
                                    discovered.iter().map(|port| PickerItem::plain(&port.name)),
                                )],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::Decklink, id));
                            }
                            if picker.clicked
                                && let Some(discovery) = engine.decklink.as_ref()
                            {
                                discovery.request_refresh();
                            }
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::Syphon => {
                            let discovered =
                                engine.syphon.as_ref().map(|d| d.list()).unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "syphon_source",
                                current,
                                labelled_sources(&engine.registry, Protocol::Syphon, |key, kind| {
                                    kind.display_label(&key.source_ref)
                                }),
                                vec![PickerGroup::flat(discovered.iter().map(|info| {
                                    PickerItem::new(
                                        info.display_name(),
                                        &format_syphon_label(&info.app_name, &info.name),
                                    )
                                }))],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::Syphon, id));
                            }
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::AvFoundation => {
                            let discovered =
                                engine.avfoundation.as_ref().map(|d| d.list()).unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "avfoundation_source",
                                current,
                                connected_sources(&engine.registry, Protocol::AvFoundation),
                                vec![PickerGroup::flat(
                                    discovered
                                        .iter()
                                        .map(|device| PickerItem::plain(&device.name)),
                                )],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::AvFoundation, id));
                            }
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::ScreenCaptureKit => {
                            let targets = engine
                                .screencapturekit
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            let mut connected = labelled_sources(
                                &engine.registry,
                                Protocol::ScreenCaptureKit,
                                |key, kind| kind.display_label(&key.source_ref),
                            );
                            connected.sort_by_cached_key(|item| item.label.to_lowercase());
                            let picker = source_picker(
                                ui,
                                "screencapturekit_source",
                                current,
                                connected,
                                vec![
                                    PickerGroup::titled(
                                        "Displays",
                                        targets.displays.iter().map(|target| {
                                            PickerItem::new(&target.source_ref, &target.label)
                                        }),
                                    ),
                                    PickerGroup::titled(
                                        "Windows",
                                        targets.windows.iter().map(|target| {
                                            PickerItem::new(&target.source_ref, &target.label)
                                        }),
                                    ),
                                ],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::ScreenCaptureKit, id));
                            }
                            if picker.clicked
                                && let Some(discovery) = engine.screencapturekit.as_ref()
                            {
                                discovery.request_refresh();
                            }
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::Spout => {
                            let discovered = engine.spout.as_ref().map(|d| d.list()).unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "spout_source",
                                current,
                                connected_sources(&engine.registry, Protocol::Spout),
                                vec![PickerGroup::flat(
                                    discovered.iter().map(|name| PickerItem::plain(name)),
                                )],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::Spout, id));
                            }
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::MediaFoundation => {
                            let discovered = engine
                                .mediafoundation
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "mediafoundation_source",
                                current,
                                labelled_sources(
                                    &engine.registry,
                                    Protocol::MediaFoundation,
                                    |key, kind| {
                                        // Prefer the live friendly name: the label
                                        // stored at connect time can be the raw
                                        // symbolic link if discovery missed then.
                                        discovered
                                            .iter()
                                            .find(|d| d.id == key.source_ref)
                                            .map(|d| d.name.clone())
                                            .unwrap_or_else(|| kind.display_label(&key.source_ref))
                                    },
                                ),
                                vec![PickerGroup::flat(discovered.iter().map(|device| {
                                    PickerItem::new(&device.id, &device.name)
                                }))],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::MediaFoundation, id));
                            }
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::DirectShow => {
                            let discovered = engine
                                .directshow
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            let picker = source_picker(
                                ui,
                                "directshow_source",
                                current,
                                labelled_sources(&engine.registry, Protocol::DirectShow, |key, kind| {
                                    discovered
                                        .iter()
                                        .find(|d| d.id == key.source_ref)
                                        .map(|d| d.name.clone())
                                        .unwrap_or_else(|| kind.display_label(&key.source_ref))
                                }),
                                vec![PickerGroup::flat(discovered.iter().map(|device| {
                                    PickerItem::new(&device.id, &device.name)
                                }))],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::DirectShow, id));
                            }
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::WindowsCapture => {
                            let targets = engine
                                .windowscapture
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            let mut connected = labelled_sources(
                                &engine.registry,
                                Protocol::WindowsCapture,
                                |key, kind| kind.display_label(&key.source_ref),
                            );
                            connected.sort_by_cached_key(|item| item.label.to_lowercase());
                            let picker = source_picker(
                                ui,
                                "windowscapture_source",
                                current,
                                connected,
                                vec![
                                    PickerGroup::titled(
                                        "Displays",
                                        targets.displays.iter().map(|target| {
                                            PickerItem::new(&target.source_ref, &target.label)
                                        }),
                                    ),
                                    PickerGroup::titled(
                                        "Windows",
                                        targets.windows.iter().map(|target| {
                                            PickerItem::new(&target.source_ref, &target.label)
                                        }),
                                    ),
                                ],
                                None,
                                true,
                            );
                            selected_source = picker.selected;
                            if let Some(id) = picker.connect {
                                new_connect = Some((Protocol::WindowsCapture, id));
                            }
                        }
                        Protocol::Vnc | Protocol::Rdp => {
                            let protocol = source.protocol.clone();
                            let picker = source_picker(
                                ui,
                                &format!("{}_source", protocol.name().to_lowercase()),
                                current,
                                manual_sources(&engine.registry, protocol.clone()),
                                Vec::new(),
                                Some(format!("+ New {} Source", protocol.label())),
                                false,
                            );
                            selected_source = picker.selected;
                            match protocol {
                                Protocol::Vnc => new_vnc_source |= picker.new_entry,
                                _ => new_rdp_source |= picker.new_entry,
                            }
                        }
                        Protocol::Unknown(_) => {
                            // No runtime source exists for unavailable protocols.
                            ui.weak("(unavailable)");
                        }
                    }
                });
                // ui.end_row();

                settings_row(ui, "Position", |ui| {
                    ui.label("X ");
                    if ui.add(egui::DragValue::new(&mut source.x)).changed() {
                        engine.dirty = true;
                    }
                    ui.label("Y ");
                    if ui.add(egui::DragValue::new(&mut source.y)).changed() {
                        engine.dirty = true;
                    }
                });

                settings_row(ui, "Size", |ui| {
                    ui.label("W");
                    if ui
                        .add(egui::DragValue::new(&mut source.width).range(1..=7680))
                        .changed()
                    {
                        engine.dirty = true;
                    }
                    ui.label("H");
                    if ui
                        .add(egui::DragValue::new(&mut source.height).range(1..=7680))
                        .changed()
                    {
                        engine.dirty = true;
                    }
                });

                settings_row(ui, "Order", |ui| {
                    ui.label("Z ");
                    if ui.add(egui::DragValue::new(&mut source.z)).changed() {
                        engine.dirty = true;
                    }
                });

                settings_row(ui, "Mode", |ui| {
                    egui::ComboBox::from_id_salt("tex_mode")
                        .width(ui.available_width())
                        .selected_text(source.mode.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut source.mode, TextureMode::Fit, "Fit");
                            ui.selectable_value(&mut source.mode, TextureMode::Fill, "Fill");
                            ui.selectable_value(&mut source.mode, TextureMode::Stretch, "Stretch");
                        });
                });

                settings_row(ui, "Flip", |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut source.flip_h, "Flip H").changed() {
                            engine.dirty = true;
                        }
                        if ui.checkbox(&mut source.flip_v, "Flip V").changed() {
                            engine.dirty = true;
                        }
                    });
                });

                settings_row(ui, "Label", |ui| {
                    egui::ComboBox::from_id_salt("source_label_visibility")
                        .width(ui.available_width())
                        .selected_text(source.label_visibility.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_value(&mut source.label_visibility, SourceLabelVisibility::Inherit, "Inherit").clicked() {
                                engine.dirty = true;
                            }
                            if ui.selectable_value(&mut source.label_visibility, SourceLabelVisibility::Show, "Always show").clicked() {
                                engine.dirty = true;
                            }
                            if ui.selectable_value(&mut source.label_visibility, SourceLabelVisibility::Hide, "Always hide").clicked() {
                                engine.dirty = true;
                            }
                        });
                });

                settings_row(ui, "Borders", |ui| {
                    egui::ComboBox::from_id_salt("source_border_visibility")
                        .width(ui.available_width())
                        .selected_text(source.border_visibility.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_value(&mut source.border_visibility, SourceBorderVisibility::Inherit, "Inherit").clicked() {
                                engine.dirty = true;
                            }
                            if ui.selectable_value(&mut source.border_visibility, SourceBorderVisibility::Show, "Always show").clicked() {
                                engine.dirty = true;
                            }
                            if ui.selectable_value(&mut source.border_visibility, SourceBorderVisibility::Hide, "Always hide").clicked() {
                                engine.dirty = true;
                            }
                        });
                });
            });
            
            // Source-specific settings and stats, looked up by the quad's own
            // (protocol, source_ref) key — never by another protocol's source.
            let settings_key = source
                .source_ref
                .clone()
                .map(|source_ref| SourceKey::new(source.protocol.clone(), source_ref));
            if let Some(key) = settings_key
                && let Some(runtime) = engine.registry.get_mut(&key)
            {
                subsection_separator(ui);
                collapsable_section(ui, "Protocol Settings", false, true, |ui| {
                    if source_settings::render_source_settings(runtime, ui) {
                        config_sync = Some(runtime.to_config());
                        restart_key = Some(key.clone());
                    }
                });

                subsection_separator(ui);
                collapsable_section(ui, "Source Stats", false, false, |ui| {
                    let stats_arc = runtime.stats();
                    let stats = stats_arc.lock().unwrap();
                    draw_source_stats_section(&stats, ui);
                });
            }

        });

        if protocol_changed {
            source.source_ref = None;
            // Leaving an unavailable protocol: the preserved raw config belongs
            // to the old protocol and must not follow the source.
            if matches!(source.config, SourceConfig::Unknown(_)) {
                source.config = SourceConfig::for_protocol(&source.protocol);
            }
            // The lists are polled lazily; refresh the one just selected now.
            if let Some(discovery) = engine.decklink.as_ref() {
                discovery.request_refresh();
            }
            #[cfg(target_os = "macos")]
            if let Some(discovery) = engine.screencapturekit.as_ref() {
                discovery.request_refresh();
            }
            engine.dirty = true;
        }

        // Apply source selection (outside the closure to avoid borrow issues)
        if selected_source.is_some() {
            source.source_ref = selected_source;
            engine.dirty = true;
        }
        if new_test_source {
            source.source_ref = Some(engine.registry.add_test(None).source_ref);
            engine.dirty = true;
        }
        if new_vnc_source {
            let config = VncSourceConfig::default();
            let key = engine.registry.add_vnc(config.clone());
            source.source_ref = Some(key.source_ref);
            source.config = SourceConfig::Vnc(config);
            engine.dirty = true;
        }
        if new_rdp_source {
            let config = RdpSourceConfig::default();
            let key = engine.registry.add_rdp(config.clone());
            source.source_ref = Some(key.source_ref);
            source.config = SourceConfig::Rdp(config);
            engine.dirty = true;
        }
    }

    if let Some((protocol, name)) = new_connect {
        engine.connect(protocol, &name);
    }

    // Write the runtime config back to every quad sharing the source
    if let Some(cfg) = config_sync {
        engine.sync_source_config(selected_uuid, cfg);
    }

    // Restart the shared runtime source if its config changed
    if let Some(key) = restart_key {
        engine.registry.restart(&key);
    }
}

fn preferred_output_mode<'a>(
    port: &'a DecklinkOutputPort,
    current: DecklinkMode,
) -> Option<&'a DecklinkOutputMode> {
    if let Some(mode) = port.modes.iter().find(|mode| mode.mode == current) {
        return Some(mode);
    }
    for preferred in [DecklinkMode::Hd1080p6000, DecklinkMode::Hd1080p5994] {
        if let Some(mode) = port.modes.iter().find(|mode| mode.mode == preferred) {
            return Some(mode);
        }
    }
    if let Some(mode) = port.modes.iter().find(|mode| mode.mode.is_progressive()) {
        return Some(mode);
    }
    return port.modes.first();
}


/// Global output toggle for one protocol. Enabling it with no output yet
/// creates one from `make_config`.
fn output_enable_row(
    ui: &mut egui::Ui,
    engine: &mut Engine,
    protocol: Protocol,
    make_config: impl FnOnce(&str) -> OutputConfig,
) {
    let label = protocol.label();
    settings_row(ui, &format!("{label} Output"), |ui| {
        let registry_id = engine
            .cfg
            .canvas
            .outputs
            .iter()
            .position(|o| o.protocol == protocol)
            .map(|i| engine.cfg.canvas.outputs[i].uuid.clone());
        let mut enabled = registry_id
            .as_ref()
            .and_then(|id| engine.output_registry.get(id))
            .map(|ok| ok.enabled())
            .unwrap_or(false);

        if ui.checkbox(&mut enabled, "Enable").changed() {
            if enabled {
                if let Some(id) = registry_id {
                    engine.set_output_enabled(&id, true);
                } else {
                    let name = format!("{label} Output");
                    engine.add_output(protocol.clone(), name.clone(), make_config(&name));
                }
            } else if let Some(id) = registry_id {
                engine.remove_output(&id);
            }
        }
    });
}

/// One selectable entry in the source picker.
struct PickerItem {
    id: String,
    label: String,
}

impl PickerItem {
    /// An entry whose label is its id.
    fn plain(id: &str) -> Self {
        return Self {
            id: id.to_string(),
            label: id.to_string(),
        };
    }

    fn new(id: &str, label: &str) -> Self {
        return Self {
            id: id.to_string(),
            label: label.to_string(),
        };
    }
}

/// A section of discovered sources. Screen capture splits them into displays
/// and windows; every other protocol has a single unlabelled section.
struct PickerGroup {
    header: Option<&'static str>,
    items: Vec<PickerItem>,
}

impl PickerGroup {
    fn flat(items: impl IntoIterator<Item = PickerItem>) -> Self {
        return Self {
            header: None,
            items: items.into_iter().collect(),
        };
    }

    fn titled(header: &'static str, items: impl IntoIterator<Item = PickerItem>) -> Self {
        return Self {
            header: Some(header),
            items: items.into_iter().collect(),
        };
    }
}

#[derive(Default)]
struct PickerResult {
    selected: Option<SourceRef>,
    connect: Option<String>,
    new_entry: bool,
    clicked: bool,
}

/// Sources already live in the registry, labelled by their source_ref.
fn connected_sources(registry: &SourceRegistry, protocol: Protocol) -> Vec<PickerItem> {
    return registry
        .list_sources(protocol)
        .into_iter()
        .map(|(key, _)| PickerItem::plain(&key.source_ref))
        .collect();
}

/// Sources already live in the registry, with a caller-supplied label.
fn labelled_sources(
    registry: &SourceRegistry,
    protocol: Protocol,
    label: impl Fn(&SourceKey, &SourceKind) -> String,
) -> Vec<PickerItem> {
    return registry
        .list_sources(protocol)
        .into_iter()
        .map(|(key, kind)| PickerItem::new(&key.source_ref, &label(key, kind)))
        .collect();
}

/// The single picker shared by every protocol: already-connected sources, then
/// the discovered ones not connected yet (selecting one connects it), then the
/// "new" entry when the protocol has one.
fn source_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    current: &str,
    connected: Vec<PickerItem>,
    groups: Vec<PickerGroup>,
    new_entry: Option<String>,
    scanning: bool,
) -> PickerResult {
    let mut result = PickerResult::default();
    let current_label = connected
        .iter()
        .chain(groups.iter().flat_map(|group| group.items.iter()))
        .find(|item| item.id == current)
        .map(|item| item.label.clone())
        .unwrap_or_else(|| current.to_string());

    let combo = egui::ComboBox::from_id_salt(id_salt)
        .width(ui.available_width())
        .height(1000.0)
        .selected_text(current_label)
        .truncate()
        .show_ui(ui, |ui| {
            for item in &connected {
                if ui.selectable_label(current == item.id, &item.label).clicked() {
                    result.selected = Some(item.id.clone());
                }
            }
            for group in &groups {
                let pending: Vec<&PickerItem> = group
                    .items
                    .iter()
                    .filter(|item| !connected.iter().any(|c| c.id == item.id))
                    .collect();
                if pending.is_empty() {
                    continue;
                }
                if let Some(header) = group.header {
                    ui.label(egui::RichText::new(header).weak());
                }
                for item in pending {
                    if ui.selectable_label(current == item.id, &item.label).clicked() {
                        result.connect = Some(item.id.clone());
                        result.selected = Some(item.id.clone());
                    }
                }
            }
            if scanning && connected.is_empty() && groups.iter().all(|group| group.items.is_empty()) {
                ui.weak("(scanning...)");
            }
            if let Some(label) = &new_entry
                && ui.selectable_label(false, label).clicked()
            {
                result.new_entry = true;
            }
        });
    result.clicked = combo.response.clicked();
    return result;
}

/// VNC and RDP have no discovery: the picker lists the connections this project
/// has created, labelled with their host and port.
fn manual_sources(registry: &SourceRegistry, protocol: Protocol) -> Vec<PickerItem> {
    return labelled_sources(registry, protocol, |key, kind| match &kind.config {
        SourceConfig::Vnc(config) => config.label(&key.source_ref),
        SourceConfig::Rdp(config) => config.label(&key.source_ref),
        _ => key.source_ref.clone(),
    });
}

fn draw_source_stats_section(stats: &SourceStats, ui: &mut egui::Ui) {
    settings_grid(ui, "source_stats_grid", |ui| {
        if stats.off_screen {
            settings_row(ui, "Status", |ui| {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Off-screen \u{2014} holding last frame",
                );
            });
        }

        settings_row(ui, "Resolution", |ui| {
            ui.label(format!(
                "{}x{} {}",
                stats.width, stats.height, stats.pixel_format
            ));
        });

        settings_row(ui, "FPS Nominal", |ui| {
            if stats.nominal_fps > 0.0 {
                ui.label(format!("{:.2}", stats.nominal_fps));
            } else {
                ui.label("-");
            }
        });

        settings_row(ui, "FPS Computed", |ui| {
            ui.label(format!("{:.2}", stats.computed_fps));
        });

        settings_row(ui, "Frames received", |ui| {
            ui.label(format!("{}", stats.frames_received));
        });

        settings_row(ui, "Frames consumed", |ui| {
            ui.label(format!("{}", stats.frames_consumed));
        });

        settings_row(ui, "Frames dropped", |ui| {
            ui.label(format!("{}", stats.frames_dropped));
        });

        settings_row(ui, "Receive", |ui| {
            ui.label(format!("{:.2} ms", stats.receive_time_ms));
        });

        if stats.gpu_direct {
            // A GPU-direct source has no CPU copy or upload; report the wire
            // rate and the decoder's own timing instead.
            if stats.network_mbps > 0.0 {
                settings_row(ui, "Network", |ui| {
                    ui.label(format!("{:.1} MB/s", stats.network_mbps));
                });
            }
            if stats.decode_ms > 0.0 {
                settings_row(ui, "Decode", |ui| {
                    ui.label(format!("{:.2} ms", stats.decode_ms));
                });
            }
        } else {
            settings_row(ui, "CPU->GPU copy", |ui| {
                ui.label(format!("{:.2} ms", stats.copy_time_ms));
            });

            settings_row(ui, "GPU upload", |ui| {
                ui.label(format!("{:.2} ms", stats.upload_time_ms));
            });

            settings_row(ui, "GPU bandwidth", |ui| {
                ui.label(format!("{:.1} MB/s", stats.upload_mbps));
            });
        }
    });
}

// Helpers
const SETTINGS_GRID_SPACING: [f32; 2] = [8.0, 4.0];
const SETTINGS_GRID_LEFT_MIN_WIDTH: f32 = 120.0;
const SUBSECTION_SEPARATOR_INSET: f32 = 16.0;

fn subsection_separator(ui: &mut Ui) {
    ui.add(Separator::default().shrink(SUBSECTION_SEPARATOR_INSET));
}

pub fn collapsable_section<R>(
    ui: &mut Ui,
    title: &str,
    heading: bool,
    default_open: bool,
    contents: impl FnOnce(&mut Ui) -> R,
) {
    let id = ui.make_persistent_id(title);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
        .show_header(ui, |ui| {
            if heading {
                ui.heading(title);
            }
            else {
                ui.label(title);
            }
        })
        .body_unindented(contents);
}

pub fn settings_grid<R>(
    ui: &mut Ui,
    id: &str,
    contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    Grid::new(id)
        .num_columns(2)
        .spacing(SETTINGS_GRID_SPACING)
        .min_col_width(SETTINGS_GRID_LEFT_MIN_WIDTH)
        .show(ui, contents)
}

/// One label/value row of a [`settings_grid`].
pub fn settings_row(ui: &mut Ui, label: &str, contents: impl FnOnce(&mut Ui)) {
    ui.label(label);
    ui.with_layout(Layout::left_to_right(Align::Center), contents);
    ui.end_row();
}

fn color_picker_rgba_row(ui: &mut egui::Ui, label: &str, color: &mut [u8; 4]) {
    settings_row(ui, label, |ui| {
        let mut color_f32 = [
            color[0] as f32 / 255.0,
            color[1] as f32 / 255.0,
            color[2] as f32 / 255.0,
            color[3] as f32 / 255.0,
        ];
        if ui.color_edit_button_rgba_unmultiplied(&mut color_f32).changed() {
            *color = [
                (color_f32[0] * 255.0) as u8,
                (color_f32[1] * 255.0) as u8,
                (color_f32[2] * 255.0) as u8,
                (color_f32[3] * 255.0) as u8,
            ];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Canvas, Source, TextureMode};

    /// Every protocol the host supports has its own picker arm; each must build
    /// its item list and draw a frame without panicking, with no discovery
    /// service running and one placement selected.
    #[test]
    fn picker_draws_for_every_available_protocol() {
        let protocols = [
            Protocol::Test,
            Protocol::Ndi,
            Protocol::Omt,
            Protocol::Decklink,
            Protocol::Vnc,
            Protocol::Rdp,
            #[cfg(target_os = "macos")]
            Protocol::Syphon,
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation,
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit,
            #[cfg(target_os = "windows")]
            Protocol::Spout,
            #[cfg(target_os = "windows")]
            Protocol::MediaFoundation,
            #[cfg(target_os = "windows")]
            Protocol::DirectShow,
            #[cfg(target_os = "windows")]
            Protocol::WindowsCapture,
        ];

        for protocol in protocols {
            let mut canvas = Canvas::default();
            canvas.sources.push(Source::new(
                "Source".to_string(),
                protocol,
                Some("ref".to_string()),
                0.0,
                0.0,
                640,
                360,
                0,
                TextureMode::Fit,
                false,
                false,
            ));
            let mut engine = Engine::test_instance(canvas);
            engine.selected_source_id = Some(engine.cfg.canvas.sources[0].uuid.clone());

            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                draw(ui, &mut engine);
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn preferred_output_mode_keeps_current_and_avoids_interlaced() {
        let mode = |name: &str, mode, width, height| DecklinkOutputMode {
            name: name.to_string(),
            mode,
            width,
            height,
            fps: 60.0,
        };
        let full = DecklinkOutputPort {
            name: "device".to_string(),
            modes: vec![
                mode("1080i60", DecklinkMode::Hd1080i6000, 1920, 1080),
                mode("720p60", DecklinkMode::Hd720p60, 1280, 720),
                mode("1080p60", DecklinkMode::Hd1080p6000, 1920, 1080),
            ],
        };
        assert_eq!(
            preferred_output_mode(&full, DecklinkMode::Hd1080p5994).map(|m| m.mode),
            Some(DecklinkMode::Hd1080p6000)
        );
        assert_eq!(
            preferred_output_mode(&full, DecklinkMode::Hd1080p50).map(|m| m.mode),
            Some(DecklinkMode::Hd1080p6000)
        );

        let progressive = DecklinkOutputPort {
            name: "device".to_string(),
            modes: vec![
                mode("1080i60", DecklinkMode::Hd1080i6000, 1920, 1080),
                mode("720p60", DecklinkMode::Hd720p60, 1280, 720),
            ],
        };
        assert_eq!(
            preferred_output_mode(&progressive, DecklinkMode::Hd1080p50).map(|m| m.mode),
            Some(DecklinkMode::Hd720p60)
        );

        let interlaced = DecklinkOutputPort {
            name: "device".to_string(),
            modes: vec![mode("1080i60", DecklinkMode::Hd1080i6000, 1920, 1080)],
        };
        assert_eq!(
            preferred_output_mode(&interlaced, DecklinkMode::Hd1080p50).map(|m| m.mode),
            Some(DecklinkMode::Hd1080i6000)
        );
    }
}
