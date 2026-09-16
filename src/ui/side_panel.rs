use crate::config::{BorderVisibility, SourceBorderVisibility, TextureMode};
use crate::sources::decklink::DisplayMode;
use crate::engine::Engine;
use crate::sources::{Protocol, SourceConfig, SourceStats, OutputConfig};
use egui::{Align, Grid, Button, InnerResponse, Layout, ScrollArea, Ui};

pub enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
}

pub fn draw(ui: &mut egui::Ui, engine: &mut Engine) -> Vec<FileAction> {
    let mut actions = Vec::new();
    egui::Panel::left("panel")
        .default_size(280.0)
        .min_size(200.0)
        .max_size(400.0)
        .show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                collapsable_section(ui, "Project", true, |ui| {
                    draw_file_buttons(ui, engine, &mut actions);
                });

                ui.separator();
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
    return actions;
}

fn draw_file_buttons(ui: &mut egui::Ui, engine: &mut Engine, actions: &mut Vec<FileAction>) {
    ui.horizontal(|ui| {
        let size = egui::vec2(55.0, 15.0);
        if ui.add(Button::new("New").min_size(size)).clicked() {
            actions.push(FileAction::New);
        }
        if ui.add(Button::new("Open…").min_size(size)).clicked() {
            actions.push(FileAction::Open);
        }
        let enabled = engine.project_path.is_some();
        if ui.add_enabled(enabled, Button::new("Save").min_size(size)).clicked() {
            actions.push(FileAction::Save);
        }
        if ui.add(Button::new("Save As…").min_size(size)).clicked() {
            actions.push(FileAction::SaveAs);
        }
    });
}

fn draw_global_section(ui: &mut egui::Ui, engine: &mut Engine) {
    let mut decklink_restart_idx: Option<usize> = None;

    settings_grid(ui, "global_settings_grid", |ui| {
        ui.label("Canvas");
        settings_value(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("W");
                if ui
                    .add(egui::DragValue::new(&mut engine.cfg.canvas.width).range(100..=7680))
                    .changed()
                {
                    engine.dirty = true;
                }
                ui.label("H");
                if ui
                    .add(egui::DragValue::new(&mut engine.cfg.canvas.height).range(100..=7680))
                    .changed()
                {
                    engine.dirty = true;
                }
            });
        });
        ui.end_row();

        ui.label("Borders");
        settings_value(ui, |ui| {
            egui::ComboBox::from_id_salt("global_layer_borders")
                .width(ui.available_width())
                .selected_text(engine.cfg.canvas.border_visibility.label())
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(
                            &mut engine.cfg.canvas.border_visibility,
                            BorderVisibility::Show,
                            "Show",
                        )
                        .clicked()
                    {
                        engine.dirty = true;
                    }
                    if ui
                        .selectable_value(
                            &mut engine.cfg.canvas.border_visibility,
                            BorderVisibility::Hide,
                            "Hide",
                        )
                        .clicked()
                    {
                        engine.dirty = true;
                    }
                });
        });
        ui.end_row();

        ui.label("Border Color");
        settings_value(ui, |ui| {
            let mut color_f32 = [
                engine.cfg.canvas.border_color[0] as f32 / 255.0,
                engine.cfg.canvas.border_color[1] as f32 / 255.0,
                engine.cfg.canvas.border_color[2] as f32 / 255.0,
            ];
            if ui.color_edit_button_rgb(&mut color_f32).changed() {
                engine.cfg.canvas.border_color = [
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
            if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.border_width).range(0.0..=100.0)).changed() {
                engine.dirty = true;
            }
        });
        ui.end_row();
    });

    collapsable_section(ui, "Output", false, |ui| {
        settings_grid(ui, "output_grid", |ui| {
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
                                let syphon_config = crate::sources::SyphonOutputConfig {
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
                            let ndi_config = crate::sources::NdiOutputConfig {
                                sender_name: "Multiviewer".to_string(),
                            };
                            engine.add_output(Protocol::Ndi, name, OutputConfig::Ndi(ndi_config));
                        }
                    } else if let Some(id) = registry_id {
                        engine.remove_output(&id);
                    }
                }
            });
            ui.end_row();

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
                            let decklink_config = crate::sources::DecklinkOutputConfig::default();
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
                    .map(|m| format!("{}", m.name))
                    .unwrap_or_else(|| "Unknown".to_string());

                ui.label("Decklink Mode");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("decklink_output_mode")
                        .width(ui.available_width())
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for m in &modes {
                                let label =
                                    format!("{}", m.name);
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
                if let Some(pointer_pos) = ui.ctx().pointer_interact_pos() {
                    if row_rect.contains(pointer_pos) {
                        if let Some(payload) = egui::DragAndDrop::payload::<String>(ui.ctx()) {
                            if payload.as_ref() != &uuid {
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
                        }
                    }
                }

                if let Some(payload) = dropped {
                    if payload.as_ref() != &uuid {
                        if let Some(from_index) = engine
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
        if ui
            .add_enabled(is_layer_selected, egui::Button::new("- Delete Source"))
            .clicked()
        {
            if let Some(uuid) = engine.selected_layer_id.take() {
                engine.remove_layer(&uuid);
            }
        }
    });
}

fn draw_source_properties_section(ui: &mut egui::Ui, engine: &mut Engine, selected_uuid: &str) {
    let mut new_test_source = false;
    let mut new_connect: Option<(Protocol, String)> = None;
    let mut selected_source: Option<String> = None;
    let mut protocol_changed = false;
    let mut restart_sid: Option<String> = None;
    let mut config_sync: Option<(String, SourceConfig)> = None;

    // Extract source_id early to avoid borrow issues
    let source_id_for_sync = engine
        .cfg
        .canvas
        .sources
        .iter()
        .find(|l| l.uuid == selected_uuid)
        .and_then(|s| s.source_id.clone());

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
                            if ui
                                .selectable_value(&mut source.protocol, Protocol::Test, "Test")
                                .clicked()
                            {
                                protocol_changed = true;
                            }
                            if ui
                                .selectable_value(&mut source.protocol, Protocol::Ndi, "NDI")
                                .clicked()
                            {
                                protocol_changed = true;
                            }
                            if ui
                                .selectable_value(
                                    &mut source.protocol,
                                    Protocol::Decklink,
                                    "DeckLink",
                                )
                                .clicked()
                            {
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "macos")]
                            if ui
                                .selectable_value(&mut source.protocol, Protocol::Syphon, "Syphon")
                                .clicked()
                            {
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "macos")]
                            if ui
                                .selectable_value(&mut source.protocol, Protocol::AvFoundation, "AVFoundation")
                                .clicked()
                            {
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
                                .map(|(id, _)| id.clone())
                                .collect();
                            let current = source.source_id.as_deref().unwrap_or("");
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
                                .map(|(id, _)| id.clone())
                                .collect();
                            let current = source.source_id.as_deref().unwrap_or("");
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
                                        if !ndi_ids.iter().any(|id| id == name) {
                                            if ui
                                                .selectable_label(
                                                    current == name,
                                                    format!("{name}"),
                                                )
                                                .clicked()
                                            {
                                                new_connect = Some((Protocol::Ndi, name.clone()));
                                                selected_source = Some(name.clone());
                                            }
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
                                .map(|(id, _)| id.clone())
                                .collect();
                            let current = source.source_id.as_deref().unwrap_or("");
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
                                        if !decklink_ids.iter().any(|id| id == name) {
                                            if ui.selectable_label(current == name, name).clicked()
                                            {
                                                new_connect = Some((Protocol::Decklink, name.clone()));
                                                selected_source = Some(name.clone());
                                            }
                                        }
                                    }
                                    if decklink_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                        #[cfg(target_os = "macos")]
                        Protocol::Syphon => {
                            let syphon_ids: Vec<String> = engine
                                .registry
                                .list_sources(Protocol::Syphon)
                                .into_iter()
                                .map(|(id, _)| id.clone())
                                .collect();
                            let current = source.source_id.as_deref().unwrap_or("");
                            let discovered =
                                engine.syphon.as_ref().map(|d| d.list()).unwrap_or_default();
                            egui::ComboBox::from_id_salt("syphon_source")
                                .width(ui.available_width())
                                .height(1000.0)
                                .selected_text(current.to_string())
                                .truncate()
                                .show_ui(ui, |ui| {
                                    for id in &syphon_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    for name in &discovered {
                                        if !syphon_ids.iter().any(|id| id == name) {
                                            if ui.selectable_label(current == name, name).clicked()
                                            {
                                                new_connect = Some((Protocol::Syphon, name.clone()));
                                                selected_source = Some(name.clone());
                                            }
                                        }
                                    }
                                    if syphon_ids.is_empty() && discovered.is_empty() {
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
                                .map(|(id, _)| id.clone())
                                .collect();
                            let current = source.source_id.as_deref().unwrap_or("");
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
                                        if !avf_ids.iter().any(|id| id == &device.name) {
                                            if ui
                                                .selectable_label(current == device.name, &device.name)
                                                .clicked()
                                            {
                                                new_connect = Some((Protocol::AvFoundation, device.name.clone()));
                                                selected_source = Some(device.name.clone());
                                            }
                                        }
                                    }
                                    if avf_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
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

                ui.label("Borders");
                settings_value(ui, |ui| {
                    egui::ComboBox::from_id_salt("layer_border_visibility")
                        .width(ui.available_width())
                        .selected_text(source.border_visibility.label())
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut source.border_visibility,
                                    SourceBorderVisibility::Inherit,
                                    "Inherit",
                                )
                                .clicked()
                            {
                                engine.dirty = true;
                            }
                            if ui
                                .selectable_value(
                                    &mut source.border_visibility,
                                    SourceBorderVisibility::Show,
                                    "Always show",
                                )
                                .clicked()
                            {
                                engine.dirty = true;
                            }
                            if ui
                                .selectable_value(
                                    &mut source.border_visibility,
                                    SourceBorderVisibility::Hide,
                                    "Always hide",
                                )
                                .clicked()
                            {
                                engine.dirty = true;
                            }
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
            });
        });

        if protocol_changed {
            source.source_id = None;
            engine.dirty = true;
        }

        // Apply source selection (outside the closure to avoid borrow issues)
        if selected_source.is_some() {
            source.source_id = selected_source;
            engine.dirty = true;
        }
        if new_test_source {
            source.source_id = Some(engine.registry.add_test(None));
            engine.dirty = true;
        }

        // Source-specific settings and stats
        let mut config_changed = false;
        let mut new_config = None;
        if let Some(ref sid) = source.source_id {
            if let Some(source) = engine.registry.get_mut(sid) {
                ui.separator();
                collapsable_section(ui, "Protocol Settings", true, |ui| {
                    if super::source_settings::render_source_settings(source, ui) {
                        config_changed = true;
                        new_config = Some(source.to_config());
                        restart_sid = Some(sid.clone());
                    }
                });

                ui.separator();
                collapsable_section(ui, "Source Stats", true, |ui| {
                    let stats_arc = source.stats();
                    let stats = stats_arc.lock().unwrap();
                    draw_source_stats_section(&*stats, ui);
                });
            }
        }

        // Store config sync data for later (outside the borrow)
        if config_changed && let Some(cfg) = new_config && let Some(sid) = source_id_for_sync.clone() {
            config_sync = Some((sid, cfg));
        }
    }

    if let Some((protocol, name)) = new_connect {
        engine.connect(protocol, &name);
    }

    // Sync runtime config back to persisted config
    if let Some((sid, cfg)) = config_sync {
        if let Some(cfg_source) = engine.cfg.canvas.sources.iter_mut().find(|s| s.source_id.as_deref() == Some(sid.as_str())) {
            cfg_source.config = cfg;
        }
        engine.dirty = true;
    }

    // Restart sources if their config changed
    if let Some(sid) = restart_sid {
        engine.registry.restart(&sid);
    }
}

fn draw_source_stats_section(stats: &SourceStats, ui: &mut egui::Ui) {
    settings_grid(ui, "source_stats_grid", |ui| {
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

        ui.label("Frames dropped");
        settings_value(ui, |ui| {
            ui.label(format!("{}", stats.frames_dropped));
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
