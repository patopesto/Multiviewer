use std::time::{Duration, Instant};

use uuid::Uuid;

use super::Engine;
use crate::config::{Source, TextureMode};
use crate::sources::{Protocol, SourceConfig, SourceKey, SourceRegistry};
use crate::sources::DecklinkSourceConfig;
use crate::sources::NdiSourceConfig;
use crate::sources::OmtSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::{
    find_syphon_server, AvFoundationSourceConfig, ensure_screen_capture_access_requested, ScreenCaptureKitSourceConfig, SyphonSourceConfig,
};
#[cfg(target_os = "windows")]
use crate::sources::{DirectShowSourceConfig, MediaFoundationSourceConfig, SpoutSourceConfig, WindowsCaptureSourceConfig};

const DISCOVERY_INTERVAL: Duration = Duration::from_millis(250);
/// Offset applied to a duplicate so it is visibly distinct from its original.
const DUPLICATE_OFFSET: f32 = 32.0;
/// Liveness accounting interval. A healthy report is logged at debug; a gap
/// longer than `SLOW_TICK`, or a late report, is logged as a warning.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// The runner maintains at 4 Hz; a gap beyond this is a real stall.
const SLOW_TICK: Duration = Duration::from_secs(1);

/// Placements of `protocol` that name a source_ref with no live runtime source, paired with the key they bind to.
fn pending_sources<'a>(placements: &'a [Source], registry: &SourceRegistry, protocol: &Protocol) -> Vec<(&'a Source, SourceKey)> {
    return placements
        .iter()
        .filter(|placement| placement.protocol == *protocol)
        .filter_map(|placement| {
            let source_ref = placement.source_ref.as_deref()?;
            let key = SourceKey::new(protocol.clone(), source_ref.to_string());
            return registry.can_open(&key).then_some((placement, key));
        })
        .collect();
}

impl Engine {
    /// Poll the control listeners and publish their feedback.
    pub fn poll_control(&mut self) {
        for command in self.control.drain() {
            if let Err(e) = self.apply_command(command) {
                tracing::warn!("control command rejected: {e}");
            }
        }
        self.publish_control_feedback();
    }

    /// Discovery, reconnect, heartbeat and pending restart progress.
    pub fn maintain(&mut self) {
        self.record_heartbeat();

        // Rate-limit discovery changes
        let now = Instant::now();
        if now.duration_since(self.last_discovery) >= DISCOVERY_INTERVAL {
            self.last_discovery = now;

            self.reconnect_ndi();
            self.reconnect_omt();
            self.reconnect_decklink();
            #[cfg(target_os = "macos")]
            self.reconnect_syphon();
            #[cfg(target_os = "macos")]
            self.reconnect_avfoundation();
            #[cfg(target_os = "macos")]
            self.reconnect_screencapturekit();
            #[cfg(target_os = "windows")]
            self.reconnect_spout();
            #[cfg(target_os = "windows")]
            self.reconnect_mediafoundation();
            #[cfg(target_os = "windows")]
            self.reconnect_directshow();
            #[cfg(target_os = "windows")]
            self.reconnect_windowscapture();
            self.reconnect_manual(Protocol::Vnc);
            self.reconnect_manual(Protocol::Rdp);
            self.reconnect_manual(Protocol::Text);
        }

        self.registry.apply_pending_restarts();
        self.cleanup_orphaned_sources();
        self.sync_all_placement_configs();
    }

    /// Report that the engine is still ticking, and how long the worst gap
    /// between two ticks was since the last report.
    fn record_heartbeat(&mut self) {
        let now = Instant::now();
        let gap = now.duration_since(self.last_tick);
        self.last_tick = now;
        self.worst_tick_gap = self.worst_tick_gap.max(gap);

        let interval = now.duration_since(self.last_heartbeat);
        if interval < HEARTBEAT_INTERVAL {
            return;
        }
        self.last_heartbeat = now;
        let sources = self.registry.iter().count();
        let outputs = self.output_registry.count();
        let interval_ms = interval.as_millis() as u64;
        let worst_tick_ms = self.worst_tick_gap.as_millis() as u64;
        if self.worst_tick_gap >= SLOW_TICK || interval >= HEARTBEAT_INTERVAL * 2 {
            tracing::warn!(sources, outputs, interval_ms, worst_tick_ms, "engine tick stalled");
        } else {
            tracing::debug!(sources, outputs, interval_ms, worst_tick_ms, "engine tick");
        }
        self.worst_tick_gap = Duration::ZERO;
    }

    /// Auto-connect pending NDI sources when they appear in discovery
    fn reconnect_ndi(&mut self) {
        let Some(discovery) = &self.ndi else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::Ndi) {
            let Some(src) = discovered.iter().find(|s| s.name == key.source_ref) else {
                continue;
            };
            let config = match &placement.config {
                SourceConfig::Ndi(c) => c.clone(),
                _ => NdiSourceConfig::default(),
            };
            self.registry.add_ndi(key.clone(), config, src.clone());
        }
    }

    /// Auto-connect pending OMT sources when they appear in discovery
    fn reconnect_omt(&mut self) {
        let Some(discovery) = &self.omt else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::Omt) {
            let Some(src) = discovered.iter().find(|s| s.instance_name() == key.source_ref) else {
                continue;
            };
            let config = match &placement.config {
                SourceConfig::Omt(c) => c.clone(),
                _ => OmtSourceConfig::default(),
            };
            self.registry.add_omt(key.clone(), config, src.clone());
        }
    }

    /// Auto-connect pending DeckLink sources when they appear in discovery
    fn reconnect_decklink(&mut self) {
        let Some(discovery) = &self.decklink else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::Decklink) {
            let Some(port) = discovered.iter().find(|p| p.name == key.source_ref) else {
                continue;
            };
            let config = match &placement.config {
                SourceConfig::Decklink(c) => c.clone(),
                _ => DecklinkSourceConfig::default(),
            };
            self.registry.add_decklink(key.clone(), config, port.connections);
        }
    }

    /// Auto-connect pending Syphon sources on macOS
    #[cfg(target_os = "macos")]
    fn reconnect_syphon(&mut self) {
        let Some(discovery) = &self.syphon else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::Syphon) {
            let Some(info) = discovered.iter().find(|s| s.display_name() == key.source_ref) else {
                continue;
            };
            let config = match &placement.config {
                SourceConfig::Syphon(c) => c.clone(),
                _ => SyphonSourceConfig::default(),
            };
            self.registry.add_syphon(key.clone(), config, info.clone());
        }
    }

    /// Auto-connect pending AVFoundation sources on macOS
    #[cfg(target_os = "macos")]
    fn reconnect_avfoundation(&mut self) {
        let Some(avf) = &self.avfoundation else {
            return;
        };
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::AvFoundation) {
            let Some(device) = avf.find_by_name(&key.source_ref) else {
                continue;
            };
            let mut config = match &placement.config {
                SourceConfig::AvFoundation(c) => c.clone(),
                _ => AvFoundationSourceConfig::default(),
            };
            config.device_unique_id = device.unique_id.clone();
            self.registry.add_avfoundation(key.clone(), config);
        }
    }

    /// Auto-connect pending ScreenCaptureKit sources on macOS
    #[cfg(target_os = "macos")]
    fn reconnect_screencapturekit(&mut self) {
        // Prompt once per source creation: discovery stays blank until permission exists.
        if self.state.cfg.canvas.sources.iter().any(|s| s.protocol == Protocol::ScreenCaptureKit) {
            ensure_screen_capture_access_requested();
        }
        let Some(discovery) = &self.screencapturekit else {
            return;
        };
        let targets = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::ScreenCaptureKit) {
            let default_config = ScreenCaptureKitSourceConfig::default();
            let candidate = match &placement.config {
                SourceConfig::ScreenCaptureKit(c) => c,
                _ => &default_config,
            };
            // Stale window ids re-match on app + title; exact source_ref needs no config.
            let Some(target) = targets.iter().find(|t| t.matches(&key.source_ref, candidate)) else {
                continue;
            };
            self.registry.add_screencapturekit(key, target.to_config(), target.label.clone());
        }
    }

    /// Auto-connect pending Spout sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_spout(&mut self) {
        let Some(discovery) = &self.spout else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::Spout) {
            if !discovered.contains(&key.source_ref) {
                continue;
            }
            let config = match &placement.config {
                SourceConfig::Spout(c) => c.clone(),
                _ => SpoutSourceConfig::default(),
            };
            self.registry.add_spout(key.clone(), config);
        }
    }

    /// Auto-connect pending Media Foundation sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_mediafoundation(&mut self) {
        let Some(discovery) = &self.mediafoundation else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::MediaFoundation) {
            let Some(device) = discovered.iter().find(|d| d.id == key.source_ref) else {
                continue;
            };
            let mut config = match &placement.config {
                SourceConfig::MediaFoundation(c) => c.clone(),
                _ => MediaFoundationSourceConfig::default(),
            };
            config.device_id = device.id.clone();
            self.registry.add_mediafoundation(key.clone(), config, device.name.clone());
        }
    }

    /// Auto-connect pending DirectShow sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_directshow(&mut self) {
        let Some(discovery) = &self.directshow else {
            return;
        };
        let discovered = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::DirectShow) {
            let Some(device) = discovered.iter().find(|d| d.id == key.source_ref) else {
                continue;
            };
            let mut config = match &placement.config {
                SourceConfig::DirectShow(c) => c.clone(),
                _ => DirectShowSourceConfig::default(),
            };
            config.device_id = device.id.clone();
            self.registry.add_directshow(key.clone(), config, device.name.clone());
        }
    }

    /// Auto-connect pending Windows Graphics Capture sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_windowscapture(&mut self) {
        let Some(discovery) = &self.windowscapture else {
            return;
        };
        let targets = discovery.list();
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &Protocol::WindowsCapture) {
            let default_config = WindowsCaptureSourceConfig::default();
            let candidate = match &placement.config {
                SourceConfig::WindowsCapture(c) => c,
                _ => &default_config,
            };
            let Some(target) = targets.iter().find(|t| t.matches(&key.source_ref, candidate)) else {
                continue;
            };
            let mut config = target.to_config();
            // Discovery only supplies target identity; keep the settings already chosen for this source.
            *config.settings_mut() = candidate.settings().clone();
            self.registry.add_windows_capture(key.clone(), config, target.label.clone());
        }
    }

    /// Auto-connect sources for protocols with no discovery service.
    fn reconnect_manual(&mut self, protocol: Protocol) {
        for (placement, key) in pending_sources(&self.state.cfg.canvas.sources, &self.registry, &protocol) {
            if key.source_ref.is_empty() {
                continue;
            }
            // Protocols with a discovery service never reach here, and a
            // placement whose config does not match its protocol has
            // nothing to open with either.
            match &placement.config {
                SourceConfig::Vnc(config) => {
                    self.registry.add_vnc_at(key.clone(), config.clone());
                }
                SourceConfig::Rdp(config) => {
                    self.registry.add_rdp_at(key.clone(), config.clone());
                }
                SourceConfig::Text(config) => {
                    self.registry.add_text_at(key.clone(), config.clone());
                }
                _ => {}
            }
        }
    }

    /// Source keys the canvas still references. Placements can share a key, so
    /// the result has no duplicates.
    pub fn active_source_keys(&self) -> Vec<SourceKey> {
        let mut keys: Vec<SourceKey> = Vec::new();
        for source in &self.state.cfg.canvas.sources {
            let Some(source_ref) = source.source_ref.clone() else {
                continue;
            };
            let key = SourceKey::new(source.protocol.clone(), source_ref);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        return keys;
    }

    pub fn cleanup_orphaned_sources(&mut self) {
        let active_keys = self.active_source_keys();
        self.registry.cleanup_orphaned_sources(&active_keys);
        if let Some(comp) = self.comp.as_mut() {
            comp.retain_sources(&active_keys);
        }
    }

    /// Persisted config of the source placement on `protocol` that references `source_ref`
    fn source_placement_config(&self, protocol: &Protocol, source_ref: &str) -> Option<&SourceConfig> {
        return self
            .state
            .cfg
            .canvas
            .sources
            .iter()
            .find(|s| s.protocol == *protocol && s.source_ref.as_deref() == Some(source_ref))
            .map(|s| &s.config);
    }

    /// Copy the live runtime config back to every placement bound to it.
    fn sync_placement_config(&mut self, key: &SourceKey) {
        let Some(kind) = self.registry.get(key) else {
            return;
        };
        let config = kind.config.clone();
        for placement in &mut self.state.cfg.canvas.sources {
            if placement.protocol == key.protocol && placement.source_ref.as_deref() == Some(key.source_ref.as_str()) {
                placement.config = config.clone();
            }
        }
    }

    /// Normalize every placement config from its live runtime, if one exists.
    pub(super) fn sync_all_placement_configs(&mut self) {
        for placement in &mut self.state.cfg.canvas.sources {
            let Some(source_ref) = placement.source_ref.clone() else {
                continue;
            };
            let key = SourceKey::new(placement.protocol.clone(), source_ref);
            if let Some(kind) = self.registry.get(&key) {
                placement.config = kind.config.clone();
            }
        }
    }

    /// Persist `config` to every source placement bound to `(protocol, source_ref)`.
    fn apply_config_to_placements(&mut self, protocol: &Protocol, source_ref: &str, config: SourceConfig) {
        for placement in self.state.cfg.canvas.sources.iter_mut().filter(|s| s.protocol == *protocol && s.source_ref.as_deref() == Some(source_ref)) {
            placement.config = config.clone();
        }
    }

    pub fn connect(&mut self, protocol: Protocol, name: &str) {
        let key = SourceKey::new(protocol.clone(), name.to_string());
        match protocol {
            // Test, Text, VNC and RDP sources are created locally, never connected to a discovered source.
            Protocol::Test | Protocol::Text | Protocol::Vnc | Protocol::Rdp => {}
            Protocol::Ndi => {
                if let Some(ref discovery) = self.ndi
                    && let Some(src) = discovery.find_by_name(name)
                {
                    let config = match self.source_placement_config(&Protocol::Ndi, name) {
                        Some(SourceConfig::Ndi(c)) => c.clone(),
                        _ => NdiSourceConfig::default(),
                    };
                    self.registry.add_ndi(key.clone(), config, src);
                }
            }
            Protocol::Decklink => {
                let config = match self.source_placement_config(&Protocol::Decklink, name) {
                    Some(SourceConfig::Decklink(c)) => c.clone(),
                    _ => DecklinkSourceConfig::default(),
                };
                let supported_connections = self.decklink.as_ref().and_then(|d| d.find_by_name(name)).map(|p| p.connections).unwrap_or_default();
                self.registry.add_decklink(key.clone(), config, supported_connections);
            }
            Protocol::Omt => {
                if let Some(ref discovery) = self.omt
                    && let Some(src) = discovery.find_by_name(name)
                {
                    let config = match self.source_placement_config(&Protocol::Omt, name) {
                        Some(SourceConfig::Omt(c)) => c.clone(),
                        _ => OmtSourceConfig::default(),
                    };
                    self.registry.add_omt(key.clone(), config, src);
                }
            }
            #[cfg(target_os = "macos")]
            Protocol::Syphon => {
                if self.syphon.is_some()
                    && let Some(info) = find_syphon_server(name)
                {
                    let config = match self.source_placement_config(&Protocol::Syphon, name) {
                        Some(SourceConfig::Syphon(c)) => c.clone(),
                        _ => SyphonSourceConfig::default(),
                    };
                    self.registry.add_syphon(key.clone(), config, info);
                }
            }
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => {
                if let Some(device) = self.avfoundation.as_ref().and_then(|d| d.find_by_name(name)) {
                    let config = AvFoundationSourceConfig {
                        device_unique_id: device.unique_id.clone(),
                        ..AvFoundationSourceConfig::default()
                    };
                    self.registry.add_avfoundation(key.clone(), config);
                }
            }
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit => {
                if let Some(target) = self.screencapturekit.as_ref().and_then(|d| d.list().find_by_source_ref(name)) {
                    let config = target.to_config();
                    // Persist identity to every placement sharing this source (window re-match needs it).
                    self.apply_config_to_placements(&Protocol::ScreenCaptureKit, name, SourceConfig::ScreenCaptureKit(config.clone()));
                    let label = target.label.clone();
                    self.registry.add_screencapturekit(key.clone(), config, label);
                }
            }
            #[cfg(target_os = "windows")]
            Protocol::Spout => {
                let config = match self.source_placement_config(&Protocol::Spout, name) {
                    Some(SourceConfig::Spout(c)) => c.clone(),
                    _ => SpoutSourceConfig::default(),
                };
                self.registry.add_spout(key.clone(), config);
            }
            #[cfg(target_os = "windows")]
            Protocol::MediaFoundation => {
                let device = self.mediafoundation.as_ref().and_then(|d| d.find_by_id(name));
                let mut config = match self.source_placement_config(&Protocol::MediaFoundation, name) {
                    Some(SourceConfig::MediaFoundation(c)) => c.clone(),
                    _ => MediaFoundationSourceConfig::default(),
                };
                config.device_id = device.as_ref().map(|d| d.id.clone()).unwrap_or_else(|| name.to_string());
                let label = device.map(|d| d.name).unwrap_or_else(|| name.to_string());
                self.registry.add_mediafoundation(key.clone(), config, label);
            }
            #[cfg(target_os = "windows")]
            Protocol::DirectShow => {
                let device = self.directshow.as_ref().and_then(|d| d.find_by_id(name));
                let mut config = match self.source_placement_config(&Protocol::DirectShow, name) {
                    Some(SourceConfig::DirectShow(c)) => c.clone(),
                    _ => DirectShowSourceConfig::default(),
                };
                config.device_id = device.as_ref().map(|d| d.id.clone()).unwrap_or_else(|| name.to_string());
                let label = device.map(|d| d.name).unwrap_or_else(|| name.to_string());
                self.registry.add_directshow(key.clone(), config, label);
            }
            #[cfg(target_os = "windows")]
            Protocol::WindowsCapture => {
                if let Some(target) = self.windowscapture.as_ref().and_then(|d| d.list().find_by_source_ref(name)) {
                    let mut config = target.to_config();
                    // Discovery only supplies target identity; keep the settings already chosen for this source.
                    if let Some(SourceConfig::WindowsCapture(existing)) = self.source_placement_config(&Protocol::WindowsCapture, name) {
                        *config.settings_mut() = existing.settings().clone();
                    }
                    self.apply_config_to_placements(&Protocol::WindowsCapture, name, SourceConfig::WindowsCapture(config.clone()));
                    let label = target.label.clone();
                    self.registry.add_windows_capture(key.clone(), config, label);
                }
            }
            // Unavailable protocols have no runtime source to connect.
            Protocol::Unknown(_) => {}
        }
        self.sync_placement_config(&key);
    }

    pub fn add_source(&mut self) -> String {
        let key = self.registry.add_test(None);
        let num = self.state.cfg.canvas.sources.len() + 1;
        let source = Source::new(
            format!("Source {num}"),
            Protocol::Test,
            Some(key.source_ref),
            self.state.cfg.canvas.width as f32 * 0.25,
            self.state.cfg.canvas.height as f32 * 0.25,
            self.state.cfg.canvas.width / 2,
            self.state.cfg.canvas.height / 2,
            self.state.cfg.canvas.sources.len() as i32,
            TextureMode::Fit,
            false,
            false,
        );
        let mut source = source;
        source.config = SourceConfig::for_protocol(&Protocol::Test);
        let uuid = source.uuid.clone();
        self.state.cfg.canvas.sources.push(source);
        self.state.dirty = true;
        return uuid;
    }

    /// Copy a placement onto the same runtime source and put it on top. The
    /// runtime is keyed by protocol + source_ref, so no second capture starts.
    pub fn duplicate_source(&mut self, uuid: &str) -> Option<String> {
        let index = self.state.cfg.canvas.sources.iter().position(|source| source.uuid == uuid)?;
        let mut copy = self.state.cfg.canvas.sources[index].clone();
        copy.uuid = Uuid::new_v4().to_string();
        copy.name = if copy.name.is_empty() {
            "Source copy".to_string()
        } else {
            format!("{} copy", copy.name)
        };
        copy.x += DUPLICATE_OFFSET;
        copy.y += DUPLICATE_OFFSET;
        copy.z = self.state.cfg.canvas.z_bounds().1.saturating_add(1);
        let copy_uuid = copy.uuid.clone();
        self.state.cfg.canvas.sources.insert(index + 1, copy);
        self.state.dirty = true;
        return Some(copy_uuid);
    }

    pub fn remove_source(&mut self, uuid: &str) {
        self.state.cfg.canvas.sources.retain(|l| l.uuid != uuid);
        if self.state.selected_source_id.as_deref() == Some(uuid) {
            self.state.selected_source_id = None;
        }
        if self.state.expanded_source_id.as_deref() == Some(uuid) {
            self.state.expanded_source_id = None;
        }
        self.state.dirty = true;
        // A placement's runtime textures outlive it otherwise; keys are shared
        // between placements, so prune against what the canvas still references.
        let active_keys = self.active_source_keys();
        if let Some(comp) = self.comp.as_mut() {
            comp.forget_placement(uuid);
            comp.retain_sources(&active_keys);
        }
    }

    /// Persist a runtime protocol config edit. The runtime source is shared by
    /// every source placement bound to the same (protocol, source_ref), so the
    /// edit is written to all of them — one runtime source, one config.
    pub fn sync_source_config(&mut self, source_uuid: &str, config: SourceConfig) {
        let Some((protocol, source_ref)) =
            self.state.cfg.canvas.sources.iter().find(|s| s.uuid == source_uuid).map(|s| (s.protocol.clone(), s.source_ref.clone()))
        else {
            return;
        };
        // The edited placement always takes the config; the siblings sharing
        // its key follow through the same write the connect path uses.
        if let Some(placement) = self.state.cfg.canvas.sources.iter_mut().find(|s| s.uuid == source_uuid) {
            placement.config = config.clone();
        }
        if let Some(source_ref) = &source_ref {
            self.apply_config_to_placements(&protocol, source_ref, config);
        }
        self.state.dirty = true;
    }

    pub fn expand_source(&mut self, uuid: String) {
        self.state.expanded_source_id = Some(uuid);
    }

    pub fn clear_expanded_source(&mut self) {
        self.state.expanded_source_id = None;
    }

    /// Cycle the selection by `step` sources, wrapping in either direction.
    pub fn select_source(&mut self, step: isize) {
        let len = self.state.cfg.canvas.sources.len();
        if len == 0 {
            self.state.selected_source_id = None;
            return;
        }
        let current = self.state.selected_source_id.as_ref().and_then(|uuid| self.state.cfg.canvas.sources.iter().position(|s| &s.uuid == uuid));
        let next = match current {
            Some(i) => (i as isize + step).rem_euclid(len as isize) as usize,
            None if step < 0 => len - 1,
            None => 0,
        };
        self.state.selected_source_id = Some(self.state.cfg.canvas.sources[next].uuid.clone());
    }

    pub fn move_source(&mut self, from_index: usize, to_index: usize) {
        let len = self.state.cfg.canvas.sources.len();
        if from_index == to_index || from_index >= len || to_index >= len {
            return;
        }
        let source = self.state.cfg.canvas.sources.remove(from_index);
        self.state.cfg.canvas.sources.insert(to_index, source);
        self.state.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Canvas;

    /// Only placements of the requested protocol that name a source_ref and have
    /// no live runtime source are pending; a live key, another protocol and an
    /// unbound placement are all skipped.
    #[test]
    fn pending_sources_skips_live_unbound_and_other_protocols() {
        let mut engine = Engine::test_instance(Canvas::default());
        let live = engine.registry.add_test(None);
        for (protocol, source_ref) in [
            (Protocol::Test, Some(live.source_ref.clone())),
            (Protocol::Test, Some("Test B".to_string())),
            (Protocol::Ndi, Some(live.source_ref)),
            (Protocol::Test, None),
        ] {
            engine.state.cfg.canvas.sources.push(Source::new(
                "placement".to_string(),
                protocol,
                source_ref,
                0.0,
                0.0,
                1,
                1,
                0,
                TextureMode::Fit,
                false,
                false,
            ));
        }

        let pending = pending_sources(&engine.state.cfg.canvas.sources, &engine.registry, &Protocol::Test);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.source_ref, "Test B");
    }
}
