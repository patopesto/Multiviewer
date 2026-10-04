use egui::{Align, Grid, InnerResponse, Layout, ScrollArea, Ui};

use crate::APP_NAME;
use crate::ui::source_settings;
use crate::config::{BorderVisibility, LabelPosition, LabelVisibility, SourceBorderVisibility, SourceLabelVisibility, TextureMode};
use crate::engine::Engine;
use crate::sources::DisplayMode;
use crate::sources::{Protocol, SourceConfig, SourceKey, SourceRef, SourceStats, OutputConfig};
use crate::sources::{DecklinkOutputConfig, NdiOutputConfig};
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
                collapsable_section(ui, "Global Settings", true, |ui| {
                    draw_global_section(ui, engine);
                });

                ui.separator();
                collapsable_section(ui, "Sources", true, |ui| {
                    draw_sources_section(ui, engine);
                });

                if let Some(selected_uuid) = engine.selected_layer_id.clone() {
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
        ui.label("Canvas");
        settings_value(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("W");
                if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.width).range(100..=7680)).changed() {
                    engine.dirty = true;
                }
                ui.label("H");
                if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.height).range(100..=7680)).changed() {
                    engine.dirty = true;
                }
            });
        });
        ui.end_row();
    });

    collapsable_section(ui, "Labels", false, |ui| {
        let label = &mut engine.cfg.canvas.label;
        settings_grid(ui, "labels_grid", |ui| {
            ui.label("Visibility");
            settings_value(ui, |ui| {
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
            ui.end_row();

            ui.label("Size");
            settings_value(ui, |ui| {
                if ui.add(egui::DragValue::new(&mut label.size).range(1.0..=256.0).speed(1.0).suffix(" px")).changed() {
                    engine.dirty = true;
                }
            });
            ui.end_row();

            ui.label("Position");
            settings_value(ui, |ui| {
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
            ui.end_row();

            color_picker_rgba_row(ui, "Text Color", &mut label.text_color);
            color_picker_rgba_row(ui, "Background", &mut label.background_color);
        });
    });

    collapsable_section(ui, "Borders", false, |ui| {
        let border = &mut engine.cfg.canvas.border;
        settings_grid(ui, "borders_grid", |ui| {
            ui.label("Visibility");
            settings_value(ui, |ui| {
                egui::ComboBox::from_id_salt("global_layer_borders")
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
            ui.end_row();

            ui.label("Border Color");
            settings_value(ui, |ui| {
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
            ui.end_row();

            ui.label("Border Size");
            settings_value(ui, |ui| {
                if ui.add(egui::DragValue::new(&mut border.width).range(0.0..=100.0)).changed() {
                    engine.dirty = true;
                }
            });
            ui.end_row();
        });
    });

    collapsable_section(ui, "Outputs", false, |ui| {
        settings_grid(ui, "output_grid", |ui| {
            ui.label("NDI Output");
            settings_value(ui, |ui| {
                let ndi_index = engine
                    .cfg
                    .canvas
                    .outputs
                    .iter()
                    .position(|o| o.protocol == Protocol::Ndi);
                let registry_id = ndi_index
                    .as_ref()
                    .map(|i| engine.cfg.canvas.outputs[*i].uuid.clone());
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
                            let name = "NDI Output".to_string();
                            let ndi_config = NdiOutputConfig {
                                sender_name: APP_NAME.to_string(),
                            };
                            engine.add_output(Protocol::Ndi, name, OutputConfig::Ndi(ndi_config));
                        }
                    } else if let Some(id) = registry_id {
                        engine.remove_output(&id);
                    }
                }
            });
            ui.end_row();

            #[cfg(target_os = "macos")]
            {
                ui.label("Syphon Output");
                settings_value(ui, |ui| {
                    let syphon_index = engine
                        .cfg
                        .canvas
                        .outputs
                        .iter()
                        .position(|o| o.protocol == Protocol::Syphon);
                    let registry_id = syphon_index
                        .as_ref()
                        .map(|i| engine.cfg.canvas.outputs[*i].uuid.clone());
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
                                let name = "Syphon Output".to_string();
                                let syphon_config = SyphonOutputConfig {
                                    server_name: name.clone(),
                                };
                                engine.add_output(Protocol::Syphon, name, OutputConfig::Syphon(syphon_config));
                            }
                        } else if let Some(id) = registry_id {
                            engine.remove_output(&id);
                        }
                    }
                });
                ui.end_row();
            }

            #[cfg(target_os = "windows")]
            {
                ui.label("Spout Output");
                settings_value(ui, |ui| {
                    let spout_index = engine
                        .cfg
                        .canvas
                        .outputs
                        .iter()
                        .position(|o| o.protocol == Protocol::Spout);
                    let registry_id = spout_index
                        .as_ref()
                        .map(|i| engine.cfg.canvas.outputs[*i].uuid.clone());
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
                                let name = "Spout Output".to_string();
                                let spout_config = SpoutOutputConfig {
                                    sender_name: APP_NAME.to_string(),
                                };
                                engine.add_output(Protocol::Spout, name, OutputConfig::Spout(spout_config));
                            }
                        } else if let Some(id) = registry_id {
                            engine.remove_output(&id);
                        }
                    }
                });
                ui.end_row();
            }

            ui.label("DeckLink Output");
            settings_value(ui, |ui| {
                let decklink_index = engine
                    .cfg
                    .canvas
                    .outputs
                    .iter()
                    .position(|o| o.protocol == Protocol::Decklink);
                let registry_id = decklink_index
                    .as_ref()
                    .map(|i| engine.cfg.canvas.outputs[*i].uuid.clone());
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
                            let name = "DeckLink Output".to_string();
                            let decklink_config = DecklinkOutputConfig::default();
                            engine.add_output(Protocol::Decklink, name, OutputConfig::Decklink(decklink_config));
                        }
                    } else if let Some(id) = registry_id {
                        engine.remove_output(&id);
                    }
                }
            });
            ui.end_row();

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
                    _ => (String::new(), DisplayMode::Hd1080p6000),
                };

                ui.label("Decklink Device");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("decklink_output_device")
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
                                        if let Some(first) = port.modes.first() {
                                            config.display_mode = first.mode;
                                            config.width = first.width;
                                            config.height = first.height;
                                            config.fps = first.fps;
                                        }
                                    }
                                    decklink_restart_idx = Some(idx);
                                    engine.dirty = true;
                                }
                            }
                        });
                });
                ui.end_row();

                let modes = outputs
                    .iter()
                    .find(|p| p.name == current_device)
                    .map(|p| p.modes.clone())
                    .unwrap_or_default();
                let selected_label = modes
                    .iter()
                    .find(|m| m.mode == current_mode)
                    .map(|m| m.name.to_string())
                    .unwrap_or_else(|| "Unknown".to_string());

                ui.label("Decklink Mode");
                settings_value(ui, |ui| {
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
                ui.end_row();
            }
        });
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

    let previous_selected = engine.selected_layer_id.clone();

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
                            egui::Id::new("layer_drag").with(&uuid),
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
                        let selected = engine.selected_layer_id.as_deref() == Some(&uuid);
                        let available_width = ui.available_width();
                        let label = egui::Button::selectable(selected, name.to_string())
                            .min_size(egui::vec2(available_width, 0.0))
                            .right_text("")
                            .truncate();
                        let row_response = ui.add(label);
                        if row_response.clicked() {
                            engine.selected_layer_id = Some(uuid.clone());
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
                    engine.move_layer(from_index, to_index);
                }
            }

            // Keep the selected row visible when selection changes (e.g. via global shortcuts).
            if engine.selected_layer_id != previous_selected && let Some(rect) = selected_row_rect {
                ui.scroll_to_rect(rect, None);
            }
        });

    ui.horizontal(|ui| {
        if ui.button("+ Add Source").clicked() {
            let uuid = engine.add_layer();
            engine.selected_layer_id = Some(uuid);
        }
        let is_layer_selected = engine.selected_layer_id.is_some();
        if ui.add_enabled(is_layer_selected, egui::Button::new("- Delete Source")).clicked()
            && let Some(uuid) = engine.selected_layer_id.take()
        {
            engine.remove_layer(&uuid);
        }
    });
}

fn draw_source_properties_section(ui: &mut egui::Ui, engine: &mut Engine, selected_uuid: &str) {
    let mut new_test_source = false;
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
        collapsable_section(ui, "Properties", true, |ui| {
            settings_grid(ui, "layer_properties_grid", |ui| {
                ui.label("Name");
                settings_value(ui, |ui| {
                    let text_edit = egui::TextEdit::singleline(&mut source.name)
                        .desired_width(ui.available_width());
                    if ui.add(text_edit).changed() {
                        engine.dirty = true;
                    }
                });
                ui.end_row();

                ui.label("Protocol");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("layer_protocol")
                        .width(ui.available_width())
                        .selected_text(source.protocol.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_value(&mut source.protocol, Protocol::Test, Protocol::Test.label()).clicked(){
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut source.protocol, Protocol::Ndi, Protocol::Ndi.label()).clicked(){
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
                        });
                });
                ui.end_row();

                // Source dropdown
                ui.label("Source");
                settings_value(ui, |ui| {
                    match source.protocol {
                        Protocol::Test => {
                            let test_ids: Vec<String> = engine
                                .registry
                                .list_sources(Protocol::Test)
                                .into_iter()
                                .map(|(key, _)| key.source_ref.clone())
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            egui::ComboBox::from_id_salt("test_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current.to_string())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    for id in &test_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    if ui.selectable_label(false, "+ New Test Source").clicked() {
                                        new_test_source = true;
                                    }
                                });
                        }
                        Protocol::Ndi => {
                            let ndi_ids: Vec<String> = engine
                                .registry
                                .list_sources(Protocol::Ndi)
                                .into_iter()
                                .map(|(key, _)| key.source_ref.clone())
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let discovered =
                                engine.ndi.as_ref().map(|d| d.list()).unwrap_or_default();
                            egui::ComboBox::from_id_salt("ndi_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current.to_string())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    // Already connected NDI sources
                                    for id in &ndi_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    // Discovered sources not yet connected (auto-connect on select)
                                    for src in &discovered {
                                        let name = &src.name;
                                        if !ndi_ids.iter().any(|id| id == name)
                                            && ui
                                                .selectable_label(current == name, name.to_string())
                                                .clicked()
                                        {
                                            new_connect = Some((Protocol::Ndi, name.clone()));
                                            selected_source = Some(name.clone());
                                        }
                                    }
                                    if ndi_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        Protocol::Decklink => {
                            let decklink_ids: Vec<String> = engine
                                .registry
                                .list_sources(Protocol::Decklink)
                                .into_iter()
                                .map(|(key, _)| key.source_ref.clone())
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let discovered = engine
                                .decklink
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            egui::ComboBox::from_id_salt("decklink_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current.to_string())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    // Already connected DeckLink sources
                                    for id in &decklink_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    // Discovered ports not yet connected (auto-connect on select)
                                    for port in &discovered {
                                        let name = &port.name;
                                        if !decklink_ids.iter().any(|id| id == name)
                                            && ui.selectable_label(current == name, name).clicked()
                                        {
                                            new_connect = Some((Protocol::Decklink, name.clone()));
                                            selected_source = Some(name.clone());
                                        }
                                    }
                                    if decklink_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::Syphon => {
                            let syphon_sources: Vec<(String, String)> = engine
                                .registry
                                .list_sources(Protocol::Syphon)
                                .into_iter()
                                .map(|(key, kind)| {
                                    let label = kind.display_label(&key.source_ref);
                                    (key.source_ref.clone(), label)
                                })
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let current_label = syphon_sources
                                .iter()
                                .find(|(id, _)| id == current)
                                .map(|(_, label)| label.clone())
                                .unwrap_or_else(|| current.to_string());
                            let discovered =
                                engine.syphon.as_ref().map(|d| d.list()).unwrap_or_default();
                            egui::ComboBox::from_id_salt("syphon_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current_label)
                                .truncate()
                                .show_ui(ui, |ui| {
                                    // Already connected Syphon sources
                                    for (id, label) in &syphon_sources {
                                        if ui.selectable_label(current == id, label).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    // Discovered servers not yet connected (auto-connect on select)
                                    for info in &discovered {
                                        let id = info.display_name();
                                        let label =
                                            format_syphon_label(&info.app_name, &info.name);
                                        if !syphon_sources.iter().any(|(c, _)| c == id)
                                            && ui
                                                .selectable_label(current == id, &label)
                                                .clicked()
                                        {
                                            new_connect =
                                                Some((Protocol::Syphon, id.to_string()));
                                            selected_source = Some(id.to_string());
                                        }
                                    }
                                    if syphon_sources.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::AvFoundation => {
                            let avf_ids: Vec<String> = engine
                                .registry
                                .list_sources(Protocol::AvFoundation)
                                .into_iter()
                                .map(|(key, _)| key.source_ref.clone())
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let discovered = engine
                                .avfoundation
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            egui::ComboBox::from_id_salt("avfoundation_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current.to_string())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    for id in &avf_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    for device in &discovered {
                                        if !avf_ids.iter().any(|id| id == &device.name)
                                            && ui
                                                .selectable_label(current == device.name, &device.name)
                                                .clicked()
                                        {
                                            new_connect = Some((Protocol::AvFoundation, device.name.clone()));
                                            selected_source = Some(device.name.clone());
                                        }
                                    }
                                    if avf_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::ScreenCaptureKit => {
                            let mut sck_sources: Vec<(String, String)> = engine
                                .registry
                                .list_sources(Protocol::ScreenCaptureKit)
                                .into_iter()
                                .map(|(key, kind)| {
                                    let label = kind.display_label(&key.source_ref);
                                    (key.source_ref.clone(), label)
                                })
                                .collect();
                            sck_sources.sort_by_cached_key(|(_, label)| label.to_lowercase());
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let current_label = sck_sources
                                .iter()
                                .find(|(id, _)| id == current)
                                .map(|(_, label)| label.clone())
                                .unwrap_or_else(|| current.to_string());
                            let targets = engine
                                .screencapturekit
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            egui::ComboBox::from_id_salt("screencapturekit_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current_label)
                                .truncate()
                                .show_ui(ui, |ui| {
                                    // Already connected targets
                                    for (id, label) in &sck_sources {
                                        if ui.selectable_label(current == id, label).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    // Discovered targets not yet connected, grouped by kind (auto-connect on select)
                                    for (header, group) in [
                                        ("Displays", &targets.displays),
                                        ("Windows", &targets.windows),
                                    ] {
                                        let pending: Vec<_> = group
                                            .iter()
                                            .filter(|t| {
                                                !sck_sources
                                                    .iter()
                                                    .any(|(c, _)| c == &t.source_ref)
                                            })
                                            .collect();
                                        if pending.is_empty() {
                                            continue;
                                        }
                                        ui.label(egui::RichText::new(header).weak());
                                        for target in pending {
                                            if ui.selectable_label(current == target.source_ref, &target.label).clicked() {
                                                new_connect = Some((Protocol::ScreenCaptureKit, target.source_ref.clone()));
                                                selected_source = Some(target.source_ref.clone());
                                            }
                                        }
                                    }
                                    if targets.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::Spout => {
                            let spout_ids: Vec<String> = engine
                                .registry
                                .list_sources(Protocol::Spout)
                                .into_iter()
                                .map(|(key, _)| key.source_ref.clone())
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let discovered =
                                engine.spout.as_ref().map(|d| d.list()).unwrap_or_default();
                            egui::ComboBox::from_id_salt("spout_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current.to_string())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    for id in &spout_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    for name in &discovered {
                                        if !spout_ids.iter().any(|id| id == name)
                                            && ui.selectable_label(current == name, name).clicked()
                                        {
                                            new_connect = Some((Protocol::Spout, name.clone()));
                                            selected_source = Some(name.clone());
                                        }
                                    }
                                    if spout_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::MediaFoundation => {
                            let discovered = engine
                                .mediafoundation
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            let mf_sources: Vec<(String, String)> = engine
                                .registry
                                .list_sources(Protocol::MediaFoundation)
                                .into_iter()
                                .map(|(key, kind)| {
                                    // Prefer the live friendly name: the label
                                    // stored at connect time can be the raw
                                    // symbolic link if discovery missed then.
                                    let label = discovered
                                        .iter()
                                        .find(|d| d.id == key.source_ref)
                                        .map(|d| d.name.clone())
                                        .unwrap_or_else(|| kind.display_label(&key.source_ref));
                                    (key.source_ref.clone(), label)
                                })
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let current_label = mf_sources
                                .iter()
                                .find(|(id, _)| id == current)
                                .map(|(_, label)| label.clone())
                                .unwrap_or_else(|| current.to_string());
                            egui::ComboBox::from_id_salt("mediafoundation_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current_label)
                                .truncate()
                                .show_ui(ui, |ui| {
                                    // Already connected devices
                                    for (id, label) in &mf_sources {
                                        if ui.selectable_label(current == id, label).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    // Discovered devices not yet connected (auto-connect on select)
                                    for device in &discovered {
                                        if !mf_sources.iter().any(|(id, _)| id == &device.id)
                                            && ui
                                                .selectable_label(current == device.id, &device.name)
                                                .clicked()
                                        {
                                            new_connect = Some((Protocol::MediaFoundation, device.id.clone()));
                                            selected_source = Some(device.id.clone());
                                        }
                                    }
                                    if mf_sources.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "windows")]
                        Protocol::DirectShow => {
                            let discovered = engine
                                .directshow
                                .as_ref()
                                .map(|d| d.list())
                                .unwrap_or_default();
                            let ds_sources: Vec<(String, String)> = engine
                                .registry
                                .list_sources(Protocol::DirectShow)
                                .into_iter()
                                .map(|(key, kind)| {
                                    let label = discovered
                                        .iter()
                                        .find(|d| d.id == key.source_ref)
                                        .map(|d| d.name.clone())
                                        .unwrap_or_else(|| kind.display_label(&key.source_ref));
                                    (key.source_ref.clone(), label)
                                })
                                .collect();
                            let current = source.source_ref.as_deref().unwrap_or("");
                            let current_label = ds_sources
                                .iter()
                                .find(|(id, _)| id == current)
                                .map(|(_, label)| label.clone())
                                .unwrap_or_else(|| current.to_string());
                            egui::ComboBox::from_id_salt("directshow_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current_label)
                                .truncate()
                                .show_ui(ui, |ui| {
                                    // Already connected devices
                                    for (id, label) in &ds_sources {
                                        if ui.selectable_label(current == id, label).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    // Discovered devices not yet connected (auto-connect on select)
                                    for device in &discovered {
                                        if !ds_sources.iter().any(|(id, _)| id == &device.id)
                                            && ui
                                                .selectable_label(current == device.id, &device.name)
                                                .clicked()
                                        {
                                            new_connect = Some((Protocol::DirectShow, device.id.clone()));
                                            selected_source = Some(device.id.clone());
                                        }
                                    }
                                    if ds_sources.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        Protocol::Unknown(_) => {
                            // No runtime source exists for unavailable protocols.
                            ui.weak("(unavailable)");
                        }
                    }
                });
                ui.end_row();

                ui.label("Position");
                settings_value(ui, |ui| {
                    ui.label("X ");
                    if ui.add(egui::DragValue::new(&mut source.x)).changed() {
                        engine.dirty = true;
                    }
                    ui.label("Y ");
                    if ui.add(egui::DragValue::new(&mut source.y)).changed() {
                        engine.dirty = true;
                    }
                });
                ui.end_row();

                ui.label("Size");
                settings_value(ui, |ui| {
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
                ui.end_row();

                ui.label("Order");
                settings_value(ui, |ui| {
                    ui.label("Z ");
                    if ui.add(egui::DragValue::new(&mut source.z)).changed() {
                        engine.dirty = true;
                    }
                });
                ui.end_row();

                ui.label("Mode");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("tex_mode")
                        .width(ui.available_width())
                        .selected_text(source.mode.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut source.mode, TextureMode::Fit, "Fit");
                            ui.selectable_value(&mut source.mode, TextureMode::Fill, "Fill");
                            ui.selectable_value(&mut source.mode, TextureMode::Stretch, "Stretch");
                        });
                });
                ui.end_row();

                ui.label("Flip");
                settings_value(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut source.flip_h, "Flip H").changed() {
                            engine.dirty = true;
                        }
                        if ui.checkbox(&mut source.flip_v, "Flip V").changed() {
                            engine.dirty = true;
                        }
                    });
                });
                ui.end_row();

                ui.label("Label");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("layer_label_visibility")
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
                ui.end_row();

                ui.label("Borders");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("layer_border_visibility")
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
                ui.end_row();
            });
        });

        if protocol_changed {
            source.source_ref = None;
            // Leaving an unavailable protocol: the preserved raw config belongs
            // to the old protocol and must not follow the source.
            if matches!(source.config, SourceConfig::Unknown(_)) {
                source.config = SourceConfig::for_protocol(&source.protocol);
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

        // Source-specific settings and stats, looked up by the quad's own
        // (protocol, source_ref) key — never by another protocol's source.
        let settings_key = source
            .source_ref
            .clone()
            .map(|source_ref| SourceKey::new(source.protocol.clone(), source_ref));
        if let Some(key) = settings_key
            && let Some(runtime) = engine.registry.get_mut(&key)
        {
            ui.separator();
            collapsable_section(ui, "Protocol Settings", false, |ui| {
                if source_settings::render_source_settings(runtime, ui) {
                    config_sync = Some(runtime.to_config());
                    restart_key = Some(key.clone());
                }
            });

            ui.separator();
            collapsable_section(ui, "Source Stats", false, |ui| {
                let stats_arc = runtime.stats();
                let stats = stats_arc.lock().unwrap();
                draw_source_stats_section(&stats, ui);
            });
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

fn draw_source_stats_section(stats: &SourceStats, ui: &mut egui::Ui) {
    settings_grid(ui, "source_stats_grid", |ui| {
        if stats.off_screen {
            ui.label("Status");
            settings_value(ui, |ui| {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Off-screen \u{2014} holding last frame",
                );
            });
            ui.end_row();
        }

        ui.label("Resolution");
        settings_value(ui, |ui| {
            ui.label(format!(
                "{}x{} {}",
                stats.width, stats.height, stats.pixel_format
            ));
        });
        ui.end_row();

        ui.label("FPS Nominal");
        settings_value(ui, |ui| {
            if stats.nominal_fps > 0.0 {
                ui.label(format!("{:.2}", stats.nominal_fps));
            } else {
                ui.label("-");
            }
        });
        ui.end_row();

        ui.label("FPS Computed");
        settings_value(ui, |ui| {
            ui.label(format!("{:.2}", stats.computed_fps));
        });
        ui.end_row();

        ui.label("Frames received");
        settings_value(ui, |ui| {
            ui.label(format!("{}", stats.frames_received));
        });
        ui.end_row();

        ui.label("Frames presented");
        settings_value(ui, |ui| {
            ui.label(format!("{}", stats.frames_presented));
        });
        ui.end_row();

        ui.label("Frames consumed");
        settings_value(ui, |ui| {
            ui.label(format!("{}", stats.frames_consumed));
        });
        ui.end_row();

        ui.label("Frames dropped");
        settings_value(ui, |ui| {
            ui.label(format!("{}", stats.frames_dropped));
        });
        ui.end_row();

        ui.label("Receive");
        settings_value(ui, |ui| {
            ui.label(format!("{:.2} ms", stats.receive_time_ms));
        });
        ui.end_row();

        ui.label("CPU->GPU copy");
        settings_value(ui, |ui| {
            ui.label(format!("{:.2} ms", stats.copy_time_ms));
        });
        ui.end_row();

        ui.label("GPU upload");
        settings_value(ui, |ui| {
            ui.label(format!("{:.2} ms", stats.upload_time_ms));
        });
        ui.end_row();
    });
}

// Helpers
const SETTINGS_GRID_SPACING: [f32; 2] = [8.0, 4.0];
const SETTINGS_GRID_LEFT_MIN_WIDTH: f32 = 120.0;

pub fn collapsable_section<R>(ui: &mut Ui, title: &str, heading: bool, contents: impl FnOnce(&mut Ui) -> R) {
    let id = ui.make_persistent_id(title);
    let default_open = heading;
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

pub fn settings_value(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    ui.with_layout(Layout::left_to_right(Align::Center), contents);
}

fn color_picker_rgba_row(ui: &mut egui::Ui, label: &str, color: &mut [u8; 4]) {
    ui.label(label);
    settings_value(ui, |ui| {
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
    ui.end_row();
}
