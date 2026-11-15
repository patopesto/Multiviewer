use std::sync::mpsc::Sender;

use egui::{Align, Grid, InnerResponse, Layout, ScrollArea, Separator, Ui};

use crate::APP_NAME;
use crate::ui::source_settings;
use crate::config::{
    BorderVisibility, Config, LabelPosition, LabelVisibility, SourceBorderVisibility,
    SourceId, SourceLabelVisibility, TextureMode,
};
use crate::engine::{
    Command, EngineSnapshot, PickerGroupView, PickerItemView, Selector, SourcePlacementPatch,
};
use crate::sources::{Protocol, SourceKey, SourceRef, SourceStats, OutputConfig};
use crate::sources::{
    DecklinkMode, DecklinkOutputConfig, DecklinkOutputMode, DecklinkOutputPort, NdiOutputConfig,
    OmtOutputConfig,
};
#[cfg(target_os = "macos")]
use crate::sources::SyphonOutputConfig;
#[cfg(target_os = "windows")]
use crate::sources::SpoutOutputConfig;

pub struct SidePanel {
    draft: Config,
    draft_revision: u64,
    initialized: bool,
    last_selected: Option<SourceId>,
}

impl Default for SidePanel {
    fn default() -> Self {
        return Self {
            draft: Config::default(),
            draft_revision: 0,
            initialized: false,
            last_selected: None,
        };
    }
}

impl SidePanel {
    fn sync(&mut self, snapshot: &EngineSnapshot, ctx: &egui::Context) {
        let interacting = ctx.text_edit_focused() || ctx.input(|input| input.pointer.any_down());
        if !self.initialized
            || (!interacting && snapshot.state.revision != self.draft_revision)
        {
            self.draft = snapshot.state.cfg.clone();
            self.draft_revision = snapshot.state.revision;
            self.initialized = true;
        }
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    panel: &mut SidePanel,
    commands: &Sender<Command>,
) {
    panel.sync(snapshot, ui.ctx());
    let mut out = Vec::new();
    egui::Panel::left("panel")
        .default_size(320.0)
        .min_size(200.0)
        .max_size(400.0)
        .show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                collapsable_section(ui, "Global Settings", true, true, |ui| {
                    draw_global_section(ui, snapshot, &mut panel.draft, &mut out);
                });

                ui.separator();
                collapsable_section(ui, "Sources", true, true, |ui| {
                    draw_sources_section(ui, snapshot, &mut panel.last_selected, &mut out);
                });

                if let Some(selected_uuid) = snapshot.state.selected_source_id.clone() {
                    draw_source_properties_section(
                        ui,
                        snapshot,
                        &mut panel.draft,
                        &selected_uuid,
                        &mut out,
                    );
                }
            });
        });
    for command in out {
        let _ = commands.send(command);
    }
}

fn draw_global_section(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    draft: &mut Config,
    out: &mut Vec<Command>,
) {
    let mut canvas_changed = false;
    let mut output_config_changed: Option<usize> = None;

    settings_grid(ui, "global_settings_grid", |ui| {
        settings_row(ui, "Canvas", |ui| {
            ui.horizontal(|ui| {
                ui.label("W");
                if ui.add(egui::DragValue::new(&mut draft.canvas.width).range(100..=7680).update_while_editing(false)).changed() {
                    canvas_changed = true;
                }
                ui.label("H");
                if ui.add(egui::DragValue::new(&mut draft.canvas.height).range(100..=7680).update_while_editing(false)).changed() {
                    canvas_changed = true;
                }
                if resolution_preset_menu(ui, 100, &mut draft.canvas.width, &mut draft.canvas.height) {
                    canvas_changed = true;
                }
            });
        });
    });

    subsection_separator(ui);
    collapsable_section(ui, "Labels", false, false, |ui| {
        let label = &mut draft.canvas.label;
        let text_color = label.text_color;
        let background_color = label.background_color;
        settings_grid(ui, "labels_grid", |ui| {
            settings_row(ui, "Visibility", |ui| {
                egui::ComboBox::from_id_salt("label_visibility")
                    .width(ui.available_width())
                    .selected_text(label.visibility.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut label.visibility, LabelVisibility::Show, "Show").changed() {
                            canvas_changed = true;
                        }
                        if ui.selectable_value(&mut label.visibility, LabelVisibility::Hide, "Hide").changed() {
                            canvas_changed = true;
                        }
                    });
            });

            settings_row(ui, "Size", |ui| {
                if ui.add(egui::DragValue::new(&mut label.size).range(1.0..=256.0).speed(1.0).suffix(" px")).changed() {
                    canvas_changed = true;
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
                                canvas_changed = true;
                            }
                        }
                    });
            });

            color_picker_rgba_row(ui, "Text Color", &mut label.text_color);
            color_picker_rgba_row(ui, "Background", &mut label.background_color);
        });
        if label.text_color != text_color || label.background_color != background_color {
            canvas_changed = true;
        }
        subsection_separator(ui);
    });

    collapsable_section(ui, "Borders", false, false, |ui| {
        let border = &mut draft.canvas.border;
        settings_grid(ui, "borders_grid", |ui| {
            settings_row(ui, "Visibility", |ui| {
                egui::ComboBox::from_id_salt("global_source_borders")
                    .width(ui.available_width())
                    .selected_text(border.visibility.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut border.visibility, BorderVisibility::Show, "Show").clicked() {
                            canvas_changed = true;
                        }
                        if ui.selectable_value(&mut border.visibility, BorderVisibility::Hide, "Hide").clicked() {
                            canvas_changed = true;
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
                    canvas_changed = true;
                }
            });

            settings_row(ui, "Border Size", |ui| {
                if ui.add(egui::DragValue::new(&mut border.width).range(0.0..=100.0)).changed() {
                    canvas_changed = true;
                }
            });
        });
        subsection_separator(ui);
    });

    collapsable_section(ui, "Outputs", false, false, |ui| {
        settings_grid(ui, "output_grid", |ui| {
            output_enable_row(ui, snapshot, out, Protocol::Ndi, |_| {
                OutputConfig::Ndi(NdiOutputConfig {
                    sender_name: APP_NAME.to_string(),
                })
            });

            output_enable_row(ui, snapshot, out, Protocol::Omt, |_| {
                OutputConfig::Omt(OmtOutputConfig {
                    sender_name: APP_NAME.to_string(),
                })
            });

            #[cfg(target_os = "macos")]
            output_enable_row(ui, snapshot, out, Protocol::Syphon, |name| {
                OutputConfig::Syphon(SyphonOutputConfig {
                    server_name: name.to_string(),
                })
            });

            #[cfg(target_os = "windows")]
            output_enable_row(ui, snapshot, out, Protocol::Spout, |_| {
                OutputConfig::Spout(SpoutOutputConfig {
                    sender_name: APP_NAME.to_string(),
                })
            });

            output_enable_row(ui, snapshot, out, Protocol::Decklink, |_| {
                OutputConfig::Decklink(DecklinkOutputConfig::default())
            });

            // Worker errors (device busy, driver missing) sit next to the toggle
            // so a silently dead output is visible.
            let decklink_error = snapshot
                .runtime
                .outputs
                .iter()
                .find(|output| output.protocol == Protocol::Decklink)
                .and_then(|output| output.stats.error.clone());
            if let Some(error) = decklink_error {
                settings_row(ui, "Status", |ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, error);
                });
            }

            // Device / mode selection only when a DeckLink output exists.
            if let Some(idx) = draft
                .canvas
                .outputs
                .iter()
                .position(|output| output.protocol == Protocol::Decklink)
            {
                let outputs = &snapshot.runtime.decklink_outputs;

                let (current_device, current_mode) = match &draft.canvas.outputs[idx].config {
                    OutputConfig::Decklink(config) => (config.device_name.clone(), config.display_mode),
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
                            for port in outputs {
                                if ui
                                    .selectable_label(current_device == port.name, &port.name)
                                    .clicked()
                                {
                                    if let OutputConfig::Decklink(config) =
                                        &mut draft.canvas.outputs[idx].config
                                    {
                                        config.device_name = port.name.clone();
                                        if let Some(mode) = preferred_output_mode(port, config.display_mode) {
                                            config.display_mode = mode.mode;
                                            config.width = mode.width;
                                            config.height = mode.height;
                                            config.fps = mode.fps;
                                        }
                                    }
                                    output_config_changed = Some(idx);
                                }
                            }
                        });
                    if combo.response.clicked() {
                        out.push(Command::RefreshDiscovery {
                            protocol: Protocol::Decklink,
                        });
                    }
                });

                let modes = outputs
                    .iter()
                    .find(|port| port.name == current_device)
                    .map(|port| port.modes.clone())
                    .unwrap_or_default();
                let selected_label = if current_device.is_empty() {
                    "Select device".to_string()
                } else {
                    modes
                        .iter()
                        .find(|mode| mode.mode == current_mode)
                        .map(|mode| mode.name.to_string())
                        .unwrap_or_else(|| "Unknown".to_string())
                };

                settings_row(ui, "Decklink Mode", |ui| {
                    ui.add_enabled_ui(!current_device.is_empty(), |ui| {
                        egui::ComboBox::from_id_salt("decklink_output_mode")
                            .width(ui.available_width())
                            .selected_text(selected_label)
                            .show_ui(ui, |ui| {
                                for mode in &modes {
                                    let label = mode.name.to_string();
                                    if ui.selectable_label(current_mode == mode.mode, label).clicked() {
                                        if let OutputConfig::Decklink(config) =
                                            &mut draft.canvas.outputs[idx].config
                                        {
                                            config.display_mode = mode.mode;
                                            config.width = mode.width;
                                            config.height = mode.height;
                                            config.fps = mode.fps;
                                        }
                                        output_config_changed = Some(idx);
                                    }
                                }
                            });
                    });
                });
            }
        });

        if !draft.canvas.outputs.is_empty() {
            collapsable_section(ui, "Output Stats", false, false, |ui| {
                settings_grid(ui, "output_stats_grid", |ui| {
                    for output in &snapshot.runtime.outputs {
                        settings_row(ui, &format!("{} Stats", output.protocol.label()), |ui| {
                            let mut line = format!(
                                "{} sent \u{2022} {} dropped \u{2022} {:.1} fps",
                                output.stats.frames_sent,
                                output.stats.frames_dropped,
                                output.stats.computed_fps
                            );
                            if let Some(error) = &output.stats.error {
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

    if let Some(idx) = output_config_changed {
        let uuid = draft.canvas.outputs[idx].uuid.clone();
        out.push(Command::SetOutputConfig {
            output: Selector::Id(uuid),
            config: draft.canvas.outputs[idx].config.clone(),
        });
    }
    if canvas_changed {
        out.push(Command::PatchCanvas {
            width: draft.canvas.width,
            height: draft.canvas.height,
            label: draft.canvas.label,
            border: draft.canvas.border,
        });
    }
}

fn draw_sources_section(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    last_selected: &mut Option<SourceId>,
    out: &mut Vec<Command>,
) {
    // Sources list with drag-and-drop reordering.
    let rows: Vec<(usize, String, String)> = snapshot
        .state
        .cfg
        .canvas
        .sources
        .iter()
        .enumerate()
        .map(|(i, source)| (i, source.uuid.clone(), source.name.clone()))
        .collect();

    let selection_changed = *last_selected != snapshot.state.selected_source_id;
    *last_selected = snapshot.state.selected_source_id.clone();

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
                        let selected = snapshot.state.selected_source_id.as_deref() == Some(&uuid);
                        let available_width = ui.available_width();
                        let label = egui::Button::selectable(selected, name.to_string())
                            .min_size(egui::vec2(available_width, 0.0))
                            .right_text("")
                            .truncate();
                        let row_response = ui.add(label);
                        if row_response.clicked() {
                            out.push(Command::SelectSource(Selector::Id(uuid.clone())));
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
                    && let Some(from_index) = snapshot
                        .state
                        .cfg
                        .canvas
                        .sources
                        .iter()
                        .position(|source| source.uuid == *payload)
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
                    out.push(Command::MoveSource {
                        from: from_index,
                        to: to_index,
                    });
                }
            }

            // Keep the selected row visible when selection changes (e.g. via global shortcuts).
            if selection_changed && let Some(rect) = selected_row_rect {
                ui.scroll_to_rect(rect, None);
            }
        });

    ui.horizontal(|ui| {
        if ui.button("+ Add Source").clicked() {
            out.push(Command::AddSource);
        }
        let is_source_selected = snapshot.state.selected_source_id.is_some();
        if ui.add_enabled(is_source_selected, egui::Button::new("- Delete Source")).clicked()
            && let Some(uuid) = snapshot.state.selected_source_id.clone()
        {
            out.push(Command::RemoveSource(Selector::Id(uuid)));
        }
    });
}

fn draw_source_properties_section(
    ui: &mut egui::Ui,
    snapshot: &EngineSnapshot,
    draft: &mut Config,
    selected_uuid: &str,
    out: &mut Vec<Command>,
) {
    let mut placement_changed = false;
    let mut protocol_changed = false;
    let mut binding_changed = false;
    let mut config_changed = false;
    let mut new_source: Option<Protocol> = None;
    let mut refresh: Option<Protocol> = None;

    let Some(source) = draft
        .canvas
        .sources
        .iter_mut()
        .find(|source| source.uuid == selected_uuid)
    else {
        return;
    };

    ui.separator();
    collapsable_section(ui, "Properties", true, true, |ui| {
        settings_grid(ui, "source_properties_grid", |ui| {
            settings_row(ui, "Name", |ui| {
                let text_edit = egui::TextEdit::singleline(&mut source.name)
                    .desired_width(ui.available_width());
                if ui.add(text_edit).changed() {
                    placement_changed = true;
                }
            });

            settings_row(ui, "Protocol", |ui| {
                egui::ComboBox::from_id_salt("source_protocol")
                    .width(ui.available_width())
                    .selected_text(source.protocol.label())
                    .show_ui(ui, |ui| {
                        for protocol in all_protocols() {
                            if ui
                                .selectable_value(&mut source.protocol, protocol.clone(), protocol.label())
                                .clicked()
                            {
                                protocol_changed = true;
                                refresh = Some(protocol);
                            }
                        }
                    });
            });

            settings_row(ui, "Source", |ui| {
                if matches!(source.protocol, Protocol::Unknown(_)) {
                    ui.weak("(unavailable)");
                } else {
                    let current = source.source_ref.as_deref().unwrap_or("");
                    let discovery = snapshot.runtime.discovery(&source.protocol);
                    let connected = discovery.map(|view| view.connected.as_slice()).unwrap_or(&[]);
                    let groups = discovery.map(|view| view.groups.as_slice()).unwrap_or(&[]);
                    let new_entry = match source.protocol {
                        Protocol::Test => Some("+ New Test Source".to_string()),
                        Protocol::Vnc => Some("+ New VNC Source".to_string()),
                        Protocol::Rdp => Some("+ New RDP Source".to_string()),
                        _ => None,
                    };
                    let scanning = !matches!(
                        source.protocol,
                        Protocol::Test | Protocol::Vnc | Protocol::Rdp
                    );
                    let picker = source_picker(
                        ui,
                        &picker_id(&source.protocol),
                        current,
                        connected,
                        groups,
                        new_entry,
                        scanning,
                    );
                    if let Some(selected) = picker.selected {
                        source.source_ref = Some(selected);
                        binding_changed = true;
                    }
                    if picker.new_entry {
                        new_source = Some(source.protocol.clone());
                    }
                    if picker.clicked {
                        refresh = Some(source.protocol.clone());
                    }
                }
            });

            settings_row(ui, "Position", |ui| {
                ui.label("X ");
                if ui.add(egui::DragValue::new(&mut source.x)).changed() {
                    placement_changed = true;
                }
                ui.label("Y ");
                if ui.add(egui::DragValue::new(&mut source.y)).changed() {
                    placement_changed = true;
                }
            });

            settings_row(ui, "Size", |ui| {
                ui.label("W");
                if ui
                    .add(egui::DragValue::new(&mut source.width).range(1..=7680))
                    .changed()
                {
                    placement_changed = true;
                }
                ui.label("H");
                if ui
                    .add(egui::DragValue::new(&mut source.height).range(1..=7680))
                    .changed()
                {
                    placement_changed = true;
                }
                if resolution_preset_menu(ui, 1, &mut source.width, &mut source.height) {
                    placement_changed = true;
                }
            });

            settings_row(ui, "Order", |ui| {
                ui.label("Z ");
                if ui.add(egui::DragValue::new(&mut source.z)).changed() {
                    placement_changed = true;
                }
            });

            settings_row(ui, "Mode", |ui| {
                egui::ComboBox::from_id_salt("tex_mode")
                    .width(ui.available_width())
                    .selected_text(source.mode.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut source.mode, TextureMode::Fit, "Fit").changed() {
                            placement_changed = true;
                        }
                        if ui.selectable_value(&mut source.mode, TextureMode::Fill, "Fill").changed() {
                            placement_changed = true;
                        }
                        if ui.selectable_value(&mut source.mode, TextureMode::Stretch, "Stretch").changed() {
                            placement_changed = true;
                        }
                    });
            });

            settings_row(ui, "Flip", |ui| {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut source.flip_h, "Flip H").changed() {
                        placement_changed = true;
                    }
                    if ui.checkbox(&mut source.flip_v, "Flip V").changed() {
                        placement_changed = true;
                    }
                });
            });

            settings_row(ui, "Label", |ui| {
                egui::ComboBox::from_id_salt("source_label_visibility")
                    .width(ui.available_width())
                    .selected_text(source.label_visibility.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut source.label_visibility, SourceLabelVisibility::Inherit, "Inherit").clicked() {
                            placement_changed = true;
                        }
                        if ui.selectable_value(&mut source.label_visibility, SourceLabelVisibility::Show, "Always show").clicked() {
                            placement_changed = true;
                        }
                        if ui.selectable_value(&mut source.label_visibility, SourceLabelVisibility::Hide, "Always hide").clicked() {
                            placement_changed = true;
                        }
                    });
            });

            settings_row(ui, "Borders", |ui| {
                egui::ComboBox::from_id_salt("source_border_visibility")
                    .width(ui.available_width())
                    .selected_text(source.border_visibility.label())
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut source.border_visibility, SourceBorderVisibility::Inherit, "Inherit").clicked() {
                            placement_changed = true;
                        }
                        if ui.selectable_value(&mut source.border_visibility, SourceBorderVisibility::Show, "Always show").clicked() {
                            placement_changed = true;
                        }
                        if ui.selectable_value(&mut source.border_visibility, SourceBorderVisibility::Hide, "Always hide").clicked() {
                            placement_changed = true;
                        }
                    });
            });
        });

        let settings_key = source
            .source_ref
            .clone()
            .map(|source_ref| SourceKey::new(source.protocol.clone(), source_ref));
        if let Some(key) = settings_key
            && let Some(runtime) = snapshot.runtime.source(&key)
        {
            if runtime.restart_pending {
                settings_row(ui, "Status", |ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, "Restarting...");
                });
            }

            subsection_separator(ui);
            collapsable_section(ui, "Protocol Settings", false, true, |ui| {
                if source_settings::render_source_settings_config(
                    &mut source.config,
                    &runtime.runtime,
                    ui,
                ) {
                    config_changed = true;
                }
            });

            subsection_separator(ui);
            collapsable_section(ui, "Source Stats", false, false, |ui| {
                draw_source_stats_section(&runtime.stats, ui);
            });
        }
    });

    let selector = Selector::Id(selected_uuid.to_string());

    if protocol_changed {
        out.push(Command::SetSourceProtocol {
            source: selector.clone(),
            protocol: source.protocol.clone(),
        });
        source.source_ref = None;
    } else {
        if config_changed && new_source.is_none() {
            out.push(Command::SetSourceConfig {
                source: selector.clone(),
                config: source.config.clone(),
            });
        }
        if let Some(protocol) = new_source {
            out.push(Command::NewSource {
                source: selector.clone(),
                protocol,
            });
        } else if binding_changed {
            out.push(Command::SetSourceBinding {
                source: selector.clone(),
                source_ref: source.source_ref.clone(),
            });
        }
    }

    if placement_changed {
        out.push(Command::PatchSourcePlacement {
            source: selector,
            patch: SourcePlacementPatch {
                name: source.name.clone(),
                x: source.x,
                y: source.y,
                width: source.width,
                height: source.height,
                z: source.z,
                mode: source.mode,
                flip_h: source.flip_h,
                flip_v: source.flip_v,
                label_visibility: source.label_visibility,
                border_visibility: source.border_visibility,
            },
        });
    }

    if let Some(protocol) = refresh {
        out.push(Command::RefreshDiscovery { protocol });
    }
}

fn all_protocols() -> Vec<Protocol> {
    let mut protocols = vec![
        Protocol::Test,
        Protocol::Ndi,
        Protocol::Omt,
        Protocol::Decklink,
    ];
    #[cfg(target_os = "macos")]
    {
        protocols.push(Protocol::Syphon);
        protocols.push(Protocol::AvFoundation);
        protocols.push(Protocol::ScreenCaptureKit);
    }
    #[cfg(target_os = "windows")]
    {
        protocols.push(Protocol::Spout);
        protocols.push(Protocol::MediaFoundation);
        protocols.push(Protocol::DirectShow);
        protocols.push(Protocol::WindowsCapture);
    }
    protocols.push(Protocol::Vnc);
    protocols.push(Protocol::Rdp);
    return protocols;
}

fn picker_id(protocol: &Protocol) -> String {
    return format!("{}_source", protocol.name().to_lowercase());
}

fn preferred_output_mode(
    port: &DecklinkOutputPort,
    current: DecklinkMode,
) -> Option<&DecklinkOutputMode> {
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
    snapshot: &EngineSnapshot,
    out: &mut Vec<Command>,
    protocol: Protocol,
    make_config: impl FnOnce(&str) -> OutputConfig,
) {
    let label = protocol.label();
    settings_row(ui, &format!("{label} Output"), |ui| {
        let existing = snapshot
            .state
            .cfg
            .canvas
            .outputs
            .iter()
            .find(|output| output.protocol == protocol);
        let registry_id = existing.map(|output| output.uuid.clone());
        let mut enabled = existing.map(|output| output.enabled).unwrap_or(false);

        if ui.checkbox(&mut enabled, "Enable").changed() {
            if enabled {
                if let Some(id) = registry_id {
                    out.push(Command::SetOutputEnabled {
                        output: Selector::Id(id),
                        enabled: true,
                    });
                } else {
                    let name = format!("{label} Output");
                    out.push(Command::AddOutput {
                        protocol: protocol.clone(),
                        config: make_config(&name),
                    });
                }
            } else if let Some(id) = registry_id {
                out.push(Command::RemoveOutput(Selector::Id(id)));
            }
        }
    });
}

#[derive(Default)]
struct PickerResult {
    selected: Option<SourceRef>,
    new_entry: bool,
    clicked: bool,
}

/// The single picker shared by every protocol: already-connected sources, then
/// the discovered ones not connected yet (selecting one connects it), then the
/// "new" entry when the protocol has one.
fn source_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    current: &str,
    connected: &[PickerItemView],
    groups: &[PickerGroupView],
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
            for item in connected {
                if ui.selectable_label(current == item.id, &item.label).clicked() {
                    result.selected = Some(item.id.clone());
                }
            }
            for group in groups {
                let pending: Vec<&PickerItemView> = group
                    .items
                    .iter()
                    .filter(|item| !connected.iter().any(|c| c.id == item.id))
                    .collect();
                if pending.is_empty() {
                    continue;
                }
                if let Some(header) = &group.header {
                    ui.label(egui::RichText::new(header).weak());
                }
                for item in pending {
                    if ui.selectable_label(current == item.id, &item.label).clicked() {
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

const RESOLUTION_PRESETS: &[&[(&str, u32, u32)]] = &[
    // 16/9 resolutions
    &[
        ("UHD", 3840, 2160),
        ("QHD", 2560, 1440),
        ("FHD", 1920, 1080),
        ("HD",  1280, 720),
        ("FHD /2", 960, 540),
        ("FHD /3", 640, 360),
        ("FHD /4", 480, 270),
        ("FHD /5", 384, 216),
        ("FHD /6", 320, 180),
        ("FHD /8", 240, 135),
        ("FHD /10", 192, 108),
        ("FHD /12", 160, 90),
        ("FHD /15", 128, 72),
        ("HD /2", 640, 360),
        ("HD /4", 320, 180),
        ("HD /5", 256, 144),
        ("HD /8", 160, 90),
    ],
    // 4/3 resolutions
    &[
        ("QXGA", 2048, 1536),
        ("UXGA", 1600, 1200),
        ("XGA",  1024, 768),
        ("SVGA", 800, 600),
        ("VGA",  640, 480),
        ("QVGA", 320, 240),
    ],
];

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

fn resolution_preset_menu(
    ui: &mut Ui,
    min_size: u32,
    width: &mut u32,
    height: &mut u32,
) -> bool {
    let response = ui.add(
        egui::Button::new("⏵").min_size(egui::vec2(24.0, ui.spacing().interact_size.y)),
    );
    let mut changed = false;
    if let Some(popup) = egui::Popup::menu(&response)
        .align(egui::RectAlign::RIGHT_START)
        .show(|ui| {
            let mut group_changed = false;
            let mut drawn = false;
            for group in RESOLUTION_PRESETS {
                if drawn {
                    ui.separator();
                }
                group_changed |= resolution_preset_rows(ui, group, min_size, width, height);
                drawn = true;
            }
            group_changed
        })
    {
        changed = popup.inner;
    }
    return changed;
}

fn resolution_preset_rows(
    ui: &mut Ui,
    presets: &[(&str, u32, u32)],
    min_size: u32,
    width: &mut u32,
    height: &mut u32,
) -> bool {
    let mut changed = false;
    for (label, preset_width, preset_height) in presets {
        if *preset_width < min_size || *preset_height < min_size {
            continue;
        }
        let selected = *width == *preset_width && *height == *preset_height;
        let text = format!("{preset_width} × {preset_height} ({label})");
        if ui.selectable_label(selected, text).clicked() {
            *width = *preset_width;
            *height = *preset_height;
            changed = true;
        }
    }
    return changed;
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
    use crate::engine::Engine;

    /// Every protocol the host supports has its own picker arm; each must build
    /// its item list and draw a frame without panicking, with no discovery
    /// service running and one placement selected.
    #[test]
    fn picker_draws_for_every_available_protocol() {
        for protocol in all_protocols() {
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
            engine.state.selected_source_id = Some(engine.state.cfg.canvas.sources[0].uuid.clone());

            let snapshot = engine.snapshot();
            let mut panel = SidePanel::default();
            let (commands, receiver) = std::sync::mpsc::channel();

            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                draw(ui, &snapshot, &mut panel, &commands);
            });
            output.textures_delta.clear();
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn resolution_presets_are_labeled_and_in_range() {
        let mut labels = std::collections::HashSet::new();
        for (label, width, height) in RESOLUTION_PRESETS.iter().copied().flatten() {
            assert!(!label.is_empty());
            assert!(labels.insert(*label), "{label} is listed twice");
            assert!((1..=7680).contains(width), "{label}");
            assert!((1..=7680).contains(height), "{label}");
        }
    }

    #[test]
    fn draft_refreshes_when_the_document_revision_changes() {
        let mut engine = Engine::test_instance(Canvas::default());
        let ctx = egui::Context::default();
        let mut panel = SidePanel::default();

        let snapshot = engine.snapshot();
        panel.sync(&snapshot, &ctx);
        assert_eq!(panel.draft.canvas.width, 1920);

        engine.state.cfg.canvas.width = 1234;
        engine.state.revision = engine.state.revision.wrapping_add(1);
        let snapshot = engine.snapshot();
        panel.sync(&snapshot, &ctx);
        assert_eq!(panel.draft.canvas.width, 1234);
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
