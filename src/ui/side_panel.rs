use crate::config::{BorderVisibility, SourceBorderVisibility, Protocol, TextureMode};
use crate::engine::Engine;
use crate::sources::SourceStats;
use egui::{Align, Grid, InnerResponse, Layout, ScrollArea, Ui};

pub fn draw(ui: &mut egui::Ui, engine: &mut Engine) {
    egui::Panel::left("panel")
        .default_size(280.0)
        .min_size(200.0)
        .max_size(400.0)
        .show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                collapsable_section(ui, "Global Settings", |ui| {
                    draw_global_section(ui, engine);
                });

                ui.separator();
                collapsable_section(ui, "Sources", |ui| {
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
                            if let Some(out) = engine.output_registry.get_mut(&id) {
                                out.set_enabled(true);
                            }
                            if let Some(idx) = syphon_index {
                                engine.cfg.canvas.outputs[idx].enabled = true;
                            }
                        } else {
                            let uuid = uuid::Uuid::new_v4().to_string();
                            let name = "Syphon Output".to_string();
                            let syphon_config = crate::sources::syphon::SyphonOutputConfig {
                                server_name: name.clone(),
                            };
                            engine.output_registry.add_syphon(uuid.clone(), syphon_config.clone());
                            engine
                                .cfg
                                .canvas
                                .outputs
                                .push(crate::config::Output::new_v4(
                                    name,
                                    Protocol::Syphon,
                                    true,
                                    crate::config::OutputConfig::Syphon(syphon_config),
                                ));
                            if let Some(last) = engine.cfg.canvas.outputs.last_mut() {
                                last.uuid = uuid;
                            }
                        }
                    } else if let Some(id) = registry_id {
                        engine.output_registry.remove(&id);
                        if let Some(idx) = syphon_index {
                            engine.cfg.canvas.outputs.remove(idx);
                        }
                    }
                    engine.dirty = true;
                }
            });
            ui.end_row();
        }
    });
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

    const MAX_VISIBLE_SOURCE_ROWS: f32 = 18.0;
    let row_height = ui.spacing().interact_size.y;
    egui::ScrollArea::vertical()
        .max_height(row_height * MAX_VISIBLE_SOURCE_ROWS)
        .show(ui, |ui| {
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
                        if ui.add(label).clicked() {
                            engine.selected_layer_id = Some(uuid.clone());
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
    let mut new_ndi_connect: Option<String> = None;
    let mut new_decklink_connect: Option<String> = None;
    let mut new_syphon_connect: Option<String> = None;
    let mut selected_source: Option<String> = None;
    let mut protocol_changed = false;
    let mut ndi_restart_sid: Option<String> = None;
    let mut decklink_restart_sid: Option<String> = None;
    #[cfg(target_os = "macos")]
    let mut syphon_restart_sid: Option<String> = None;

    if let Some(source) = engine
        .cfg
        .canvas
        .sources
        .iter_mut()
        .find(|l| l.uuid == selected_uuid)
    {
        ui.separator();
        collapsable_section(ui, "Properties", |ui| {
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
                                .list_test_sources()
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
                                .list_ndi_sources()
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
                                                new_ndi_connect = Some(name.clone());
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
                                .list_decklink_sources()
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
                                                new_decklink_connect = Some(name.clone());
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
                                .list_syphon_sources()
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
                                                new_syphon_connect = Some(name.clone());
                                                selected_source = Some(name.clone());
                                            }
                                        }
                                    }
                                    if syphon_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        }
                    }
                });
                ui.end_row();

                ui.label("Position");
                settings_value(ui, |ui| {
                    ui.label("X");
                    if ui.add(egui::DragValue::new(&mut source.x)).changed() {
                        engine.dirty = true;
                    }
                    ui.label("Y");
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
                    ui.label("Z");
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
            source.source_id = Some(engine.registry.add_test());
            engine.dirty = true;
        }

        // Source-specific settings and stats
        if let Some(ref sid) = source.source_id {
            if let Some(source) = engine.registry.get_mut(sid) {
                ui.separator();
                collapsable_section(ui, "Protocol Settings", |ui| {
                    if super::source_settings::render_source_settings(source, ui) {
                        // Config changed — NDI/DeckLink/Syphon need restart
                        if source.protocol() == Protocol::Decklink {
                            decklink_restart_sid = Some(sid.clone());
                        } else if source.protocol() != Protocol::Test {
                            ndi_restart_sid = Some(sid.clone());
                        }
                        #[cfg(target_os = "macos")]
                        if source.protocol() == Protocol::Syphon {
                            syphon_restart_sid = Some(sid.clone());
                        }
                    }
                });

                ui.separator();
                collapsable_section(ui, "Source Stats", |ui| {
                    let stats_arc = source.stats();
                    let stats = stats_arc.lock().unwrap();
                    draw_source_stats_section(&*stats, ui);
                });
            }
        }
    }

    if let Some(name) = new_ndi_connect {
        engine.connect_ndi(&name);
    }
    if let Some(name) = new_decklink_connect {
        engine.connect_decklink(&name);
    }
    #[cfg(target_os = "macos")]
    if let Some(name) = new_syphon_connect {
        engine.connect_syphon(&name);
    }

    // Restart NDI source if its config changed
    if let Some(sid) = ndi_restart_sid {
        engine.registry.restart_ndi(&sid);
    }
    if let Some(sid) = decklink_restart_sid {
        engine.registry.restart_decklink(&sid);
    }
    #[cfg(target_os = "macos")]
    if let Some(sid) = syphon_restart_sid {
        engine.registry.restart_syphon(&sid);
    }
}

pub fn draw_source_stats_section(stats: &SourceStats, ui: &mut egui::Ui) {
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

pub fn collapsable_section<R>(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui) -> R) {
    let id = ui.make_persistent_id(title);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.heading(title);
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
