use super::Engine;
use crate::config::Output;
use crate::sources::{OutputConfig, Protocol};

impl Engine {
    pub fn add_output(&mut self, protocol: Protocol, name: String, config: OutputConfig) -> String {
        let output = Output::new(name.clone(), protocol.clone(), true, config.clone());
        let id = output.uuid.clone();
        self.cfg.canvas.outputs.push(output);
        self.output_registry.add(&protocol, id.clone(), name, &config, true);

        self.dirty = true;
        return id;
    }

    pub fn remove_output(&mut self, uuid: &str) {
        self.output_registry.remove(&uuid.to_string());
        self.cfg.canvas.outputs.retain(|o| o.uuid != uuid);
        self.dirty = true;
    }

    pub fn set_output_enabled(&mut self, uuid: &str, enabled: bool) {
        if let Some(output) = self.cfg.canvas.outputs.iter_mut().find(|o| o.uuid == uuid) {
            output.enabled = enabled;
        }
        if let Some(out) = self.output_registry.get_mut(&uuid.to_string()) {
            out.set_enabled(enabled);
        } else if enabled {
            tracing::info!("output {uuid}: runtime missing while enabling, recreating");
            self.restart_output(uuid);
        }
        self.dirty = true;
    }

    pub fn restart_output(&mut self, uuid: &str) {
        let output = match self.cfg.canvas.outputs.iter().find(|o| o.uuid == uuid) {
            Some(o) => o.clone(),
            None => return,
        };

        self.output_registry.restart(
            &output.protocol,
            uuid.to_string(),
            output.name.clone(),
            &output.config,
            output.enabled,
        );

        self.dirty = true;
    }
}
