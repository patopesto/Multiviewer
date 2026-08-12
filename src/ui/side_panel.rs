use crate::config::{Protocol, TextureMode};
use crate::engine::Engine;

pub fn draw(ui: &mut egui::Ui, engine: &mut Engine) {
    egui::Panel::left("panel").default_size(280.0).show(ui, |ui| {
        ui.heading("Global Settings");
        ui.horizontal(|ui| {
            ui.label("Canvas W");
            if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.width).range(100..=7680)).changed() {
                engine.dirty = true;
            }
            ui.label("H");
            if ui.add(egui::DragValue::new(&mut engine.cfg.canvas.height).range(100..=7680)).changed() {
                engine.dirty = true;
            }
        });

        ui.separator();
        ui.heading("Layers");
        if ui.button("+ Add Layer").clicked() {
            let uuid = engine.add_layer();
            engine.selected_layer_id = Some(uuid);
        }

        // Show layer list (push_id so identical names don't collide in egui)
        for layer in &engine.cfg.canvas.layers {
            ui.push_id(&layer.uuid, |ui| {
                let selected = engine.selected_layer_id.as_deref() == Some(&layer.uuid);
                if ui.selectable_label(selected, &layer.name).clicked() {
                    engine.selected_layer_id = Some(layer.uuid.clone());
                }
            });
        }

        // Properties panel
        if let Some(selected_uuid) = engine.selected_layer_id.clone() {
            let mut removed = false;
            let mut new_test_source = false;
            let mut new_ndi_connect: Option<String> = None;
            let mut new_syphon_connect: Option<String> = None;
            let mut selected_source: Option<String> = None;
            let mut protocol_changed = false;
            let mut ndi_restart_sid: Option<String> = None;
            #[cfg(target_os = "macos")]
            let mut syphon_restart_sid: Option<String> = None;

            if let Some(layer) = engine.cfg.canvas.layers.iter_mut().find(|l| l.uuid == selected_uuid) {
                ui.separator();
                ui.heading("Properties");

                // Name
                ui.horizontal(|ui| {
                    ui.label("Name");
                    if ui.text_edit_singleline(&mut layer.name).changed() {
                        engine.dirty = true;
                    }
                });

                // Protocol dropdown
                ui.horizontal(|ui| {
                    ui.label("Protocol");
                    egui::ComboBox::from_id_salt("layer_protocol")
                        .selected_text(layer.protocol.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_value(&mut layer.protocol, Protocol::Test, "Test").clicked() {
                                protocol_changed = true;
                            }
                            if ui.selectable_value(&mut layer.protocol, Protocol::Ndi, "NDI").clicked() {
                                protocol_changed = true;
                            }
                            #[cfg(target_os = "macos")]
                            if ui.selectable_value(&mut layer.protocol, Protocol::Syphon, "Syphon").clicked() {
                                protocol_changed = true;
                            }
                        });
                });

                // Source dropdown
                match layer.protocol {
                    Protocol::Test => {
                        let test_ids: Vec<String> = engine.registry.list_test_sources()
                            .into_iter().map(|(id, _)| id.clone()).collect();
                        let current = layer.source_id.as_deref().unwrap_or("");
                        ui.horizontal(|ui| {
                            ui.label("Source");
                            egui::ComboBox::from_id_salt("test_source")
                                .selected_text(current.to_string())
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
                        });
                    }
                    Protocol::Ndi => {
                        let ndi_ids: Vec<String> = engine.registry.list_ndi_sources()
                            .into_iter().map(|(id, _)| id.clone()).collect();
                        let current = layer.source_id.as_deref().unwrap_or("");
                        let discovered = engine.ndi.as_ref().map(|d| d.list()).unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label("Source");
                            egui::ComboBox::from_id_salt("ndi_source")
                                .selected_text(current.to_string())
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
                                            if ui.selectable_label(current == name, format!("{name}")).clicked() {
                                                new_ndi_connect = Some(name.clone());
                                                selected_source = Some(name.clone());
                                            }
                                        }
                                    }
                                    if ndi_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        });
                    }
                    #[cfg(target_os = "macos")]
                    Protocol::Syphon => {
                        let syphon_ids: Vec<String> = engine.registry.list_syphon_sources()
                            .into_iter().map(|(id, _)| id.clone()).collect();
                        let current = layer.source_id.as_deref().unwrap_or("");
                        let discovered = engine.syphon.as_ref().map(|d| d.list()).unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label("Source");
                            egui::ComboBox::from_id_salt("syphon_source")
                                .selected_text(current.to_string())
                                .show_ui(ui, |ui| {
                                    for id in &syphon_ids {
                                        if ui.selectable_label(current == id, id).clicked() {
                                            selected_source = Some(id.clone());
                                        }
                                    }
                                    for name in &discovered {
                                        if !syphon_ids.iter().any(|id| id == name) {
                                            if ui.selectable_label(current == name, name).clicked() {
                                                new_syphon_connect = Some(name.clone());
                                                selected_source = Some(name.clone());
                                            }
                                        }
                                    }
                                    if syphon_ids.is_empty() && discovered.is_empty() {
                                        ui.weak("(scanning...)");
                                    }
                                });
                        });
                    }
                }

                if protocol_changed {
                    layer.source_id = None;
                    engine.dirty = true;
                }

                // Apply source selection (outside the closure to avoid borrow issues)
                if selected_source.is_some() {
                    layer.source_id = selected_source;
                    engine.dirty = true;
                }
                if new_test_source {
                    layer.source_id = Some(engine.registry.add_test());
                    engine.dirty = true;
                }
                // Common properties
                ui.horizontal(|ui| {
                    ui.label("X");
                    if ui.add(egui::DragValue::new(&mut layer.x)).changed() { engine.dirty = true; }
                    ui.label("Y");
                    if ui.add(egui::DragValue::new(&mut layer.y)).changed() { engine.dirty = true; }
                });
                ui.horizontal(|ui| {
                    ui.label("W");
                    if ui.add(egui::DragValue::new(&mut layer.width).range(1..=7680)).changed() { engine.dirty = true; }
                    ui.label("H");
                    if ui.add(egui::DragValue::new(&mut layer.height).range(1..=7680)).changed() { engine.dirty = true; }
                });
                ui.horizontal(|ui| {
                    ui.label("Z");
                    if ui.add(egui::DragValue::new(&mut layer.z)).changed() { engine.dirty = true; }
                });
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    egui::ComboBox::from_id_salt("tex_mode")
                        .selected_text(layer.mode.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut layer.mode, TextureMode::Fit, "Fit");
                            ui.selectable_value(&mut layer.mode, TextureMode::Fill, "Fill");
                            ui.selectable_value(&mut layer.mode, TextureMode::Stretch, "Stretch");
                        });
                });
                if ui.button("Remove from Canvas").clicked() {
                    removed = true;
                }

                // Source-specific settings
                if let Some(ref sid) = layer.source_id {
                    if let Some(source) = engine.registry.get_mut(sid) {
                        ui.separator();
                        ui.heading("Source Settings");
                        if super::source_settings::render_source_settings(source, ui) {
                            // Config changed — NDI/Syphon need restart
                            if !source.is_test() {
                                ndi_restart_sid = Some(sid.clone());
                            }
                            #[cfg(target_os = "macos")]
                            if source.is_syphon() {
                                syphon_restart_sid = Some(sid.clone());
                            }
                        }
                    }
                }
            }

            if let Some(name) = new_ndi_connect {
                engine.connect_ndi(&name);
            }
            #[cfg(target_os = "macos")]
            if let Some(name) = new_syphon_connect {
                engine.connect_syphon(&name);
            }

            if removed {
                engine.remove_layer(&selected_uuid);
            }

            // Restart NDI source if its config changed
            if let Some(sid) = ndi_restart_sid {
                engine.registry.restart_ndi(&sid);
            }
            #[cfg(target_os = "macos")]
            if let Some(sid) = syphon_restart_sid {
                engine.registry.restart_syphon(&sid);
            }
        }

        // Cleanup all orphaned sources (Test and NDI)
        engine.cleanup_orphaned_sources();
    });
}
