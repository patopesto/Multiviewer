use super::Engine;
use crate::config::{Source, TextureMode};
use crate::sources::{Protocol, SourceConfig, SourceKey};
use crate::sources::DecklinkSourceConfig;
use crate::sources::NdiSourceConfig;
#[cfg(target_os = "macos")]
use crate::sources::{AvFoundationSourceConfig, ensure_screen_capture_access_requested, ScreenCaptureKitSourceConfig, SyphonSourceConfig};
#[cfg(target_os = "windows")]
use crate::sources::{DirectShowSourceConfig, MediaFoundationSourceConfig, SpoutSourceConfig, WindowsCaptureSourceConfig};

impl Engine {
    pub fn update(&mut self) {
        self.reconnect_ndi();
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

        self.registry.apply_pending_restarts();
    }

    /// Auto-connect pending NDI sources when they appear in discovery
    fn reconnect_ndi(&mut self) {
        let Some(ref discovery) = self.ndi else {
            return;
        };
        let discovered = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::Ndi
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::Ndi, source_ref.clone());
                if !self.registry.contains(&key)
                    && let Some(src) = discovered.iter().find(|s| &s.name == source_ref)
                {
                    let config = match &source.config {
                        SourceConfig::Ndi(c) => c.clone(),
                        _ => NdiSourceConfig::default(),
                    };
                    self.registry.add_ndi(key, config, src.clone());
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending DeckLink sources when they appear in discovery
    fn reconnect_decklink(&mut self) {
        let Some(ref discovery) = self.decklink else {
            return;
        };
        let discovered = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::Decklink
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::Decklink, source_ref.clone());
                if !self.registry.contains(&key)
                    && let Some(port) = discovered.iter().find(|p| &p.name == source_ref)
                {
                    let config = match &source.config {
                        SourceConfig::Decklink(c) => c.clone(),
                        _ => DecklinkSourceConfig::default(),
                    };
                    self.registry.add_decklink(key, config, port.connections);
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending Syphon sources on macOS
    #[cfg(target_os = "macos")]
    fn reconnect_syphon(&mut self) {
        let Some(ref discovery) = self.syphon else {
            return;
        };
        let discovered = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::Syphon
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::Syphon, source_ref.clone());
                if !self.registry.contains(&key)
                    && let Some(info) = discovered
                        .iter()
                        .find(|s| s.display_name() == source_ref.as_str())
                {
                    let config = match &source.config {
                        SourceConfig::Syphon(c) => c.clone(),
                        _ => SyphonSourceConfig::default(),
                    };
                    self.registry.add_syphon(key, config, info.clone());
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending AVFoundation sources on macOS
    #[cfg(target_os = "macos")]
    fn reconnect_avfoundation(&mut self) {
        let Some(ref avf) = self.avfoundation else {
            return;
        };
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::AvFoundation
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::AvFoundation, source_ref.clone());
                if !self.registry.contains(&key)
                    && let Some(device) = avf.find_by_name(source_ref)
                {
                    let mut config = match &source.config {
                        SourceConfig::AvFoundation(c) => c.clone(),
                        _ => AvFoundationSourceConfig::default(),
                    };
                    config.device_unique_id = device.unique_id.clone();
                    self.registry.add_avfoundation(key, config);
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending ScreenCaptureKit sources on macOS
    #[cfg(target_os = "macos")]
    fn reconnect_screencapturekit(&mut self) {
        // Prompt once per source creation: discovery stays blank until permission exists.
        if self
            .cfg
            .canvas
            .sources
            .iter()
            .any(|s| s.protocol == Protocol::ScreenCaptureKit)
        {
            ensure_screen_capture_access_requested();
        }
        let Some(ref discovery) = self.screencapturekit else {
            return;
        };
        let targets = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol != Protocol::ScreenCaptureKit {
                continue;
            }
            let Some(ref source_ref) = source.source_ref else {
                continue;
            };
            let key = SourceKey::new(Protocol::ScreenCaptureKit, source_ref.clone());
            if self.registry.contains(&key) {
                continue;
            }
            let default_config = ScreenCaptureKitSourceConfig::default();
            let source_placement_config = match &source.config {
                SourceConfig::ScreenCaptureKit(c) => Some(c),
                _ => None,
            };
            // Stale window ids re-match on app + title; exact source_ref needs no config.
            let candidate = source_placement_config.unwrap_or(&default_config);
            if let Some(target) = targets.iter().find(|t| t.matches(source_ref, candidate)) {
                let config = target.to_config();
                let label = target.label.clone();
                self.registry.add_screencapturekit(key, config, label);
                self.dirty = true;
            }
        }
    }

    /// Auto-connect pending Spout sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_spout(&mut self) {
        let Some(ref discovery) = self.spout else {
            return;
        };
        let discovered = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::Spout
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::Spout, source_ref.clone());
                if !self.registry.contains(&key) && discovered.iter().any(|s| s == source_ref) {
                    let config = match &source.config {
                        SourceConfig::Spout(c) => c.clone(),
                        _ => SpoutSourceConfig::default(),
                    };
                    self.registry.add_spout(key, config);
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending Media Foundation sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_mediafoundation(&mut self) {
        let Some(ref discovery) = self.mediafoundation else {
            return;
        };
        let discovered = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::MediaFoundation
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::MediaFoundation, source_ref.clone());
                if !self.registry.contains(&key)
                    && let Some(device) = discovered.iter().find(|d| &d.id == source_ref)
                {
                    let mut config = match &source.config {
                        SourceConfig::MediaFoundation(c) => c.clone(),
                        _ => MediaFoundationSourceConfig::default(),
                    };
                    config.device_id = device.id.clone();
                    self.registry
                        .add_mediafoundation(key, config, device.name.clone());
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending DirectShow sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_directshow(&mut self) {
        let Some(ref discovery) = self.directshow else {
            return;
        };
        let discovered = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol == Protocol::DirectShow
                && let Some(ref source_ref) = source.source_ref
            {
                let key = SourceKey::new(Protocol::DirectShow, source_ref.clone());
                if !self.registry.contains(&key)
                    && let Some(device) = discovered.iter().find(|d| &d.id == source_ref)
                {
                    let mut config = match &source.config {
                        SourceConfig::DirectShow(c) => c.clone(),
                        _ => DirectShowSourceConfig::default(),
                    };
                    config.device_id = device.id.clone();
                    self.registry.add_directshow(key, config, device.name.clone());
                    self.dirty = true;
                }
            }
        }
    }

    /// Auto-connect pending Windows Graphics Capture sources on Windows
    #[cfg(target_os = "windows")]
    fn reconnect_windowscapture(&mut self) {
        let Some(ref discovery) = self.windowscapture else {
            return;
        };
        let targets = discovery.list();
        for source in &self.cfg.canvas.sources {
            if source.protocol != Protocol::WindowsCapture {
                continue;
            }
            let Some(ref source_ref) = source.source_ref else {
                continue;
            };
            let key = SourceKey::new(Protocol::WindowsCapture, source_ref.clone());
            if self.registry.contains(&key) {
                continue;
            }
            let default_config = WindowsCaptureSourceConfig::default();
            let source_placement_config = match &source.config {
                SourceConfig::WindowsCapture(c) => Some(c),
                _ => None,
            };
            let candidate = source_placement_config.unwrap_or(&default_config);
            if let Some(target) = targets.iter().find(|t| t.matches(source_ref, candidate)) {
                let mut config = target.to_config();
                // Discovery only supplies target identity; keep the settings already chosen for this source.
                *config.settings_mut() = candidate.settings().clone();
                let label = target.label.clone();
                self.registry.add_windows_capture(key, config, label);
                self.dirty = true;
            }
        }
    }

    pub fn cleanup_orphaned_sources(&mut self) {
        let active_keys: Vec<SourceKey> = self
            .cfg
            .canvas
            .sources
            .iter()
            .filter_map(|l| {
                l.source_ref
                    .clone()
                    .map(|source_ref| SourceKey::new(l.protocol.clone(), source_ref))
            })
            .collect();
        self.registry.cleanup_orphaned_sources(&active_keys);
    }

    /// Persisted config of the source placement on `protocol` that references `source_ref`
    fn source_placement_config(&self, protocol: &Protocol, source_ref: &str) -> Option<&SourceConfig> {
        return self
            .cfg
            .canvas
            .sources
            .iter()
            .find(|s| s.protocol == *protocol && s.source_ref.as_deref() == Some(source_ref))
            .map(|s| &s.config);
    }

    /// Persist `config` to every source placement bound to `(protocol, source_ref)`.
    fn apply_config_to_placements(
        &mut self,
        protocol: &Protocol,
        source_ref: &str,
        config: SourceConfig,
    ) {
        for placement in self.cfg.canvas.sources.iter_mut().filter(|s| {
            s.protocol == *protocol && s.source_ref.as_deref() == Some(source_ref)
        }) {
            placement.config = config.clone();
        }
    }

    pub fn connect(&mut self, protocol: Protocol, name: &str) {
        let key = SourceKey::new(protocol.clone(), name.to_string());
        match protocol {
            // Test sources are created locally, never connected to.
            Protocol::Test => {}
            Protocol::Ndi => {
                if let Some(ref discovery) = self.ndi
                    && let Some(src) = discovery.find_by_name(name)
                {
                    let config = match self.source_placement_config(&Protocol::Ndi, name) {
                        Some(SourceConfig::Ndi(c)) => c.clone(),
                        _ => NdiSourceConfig::default(),
                    };
                    self.registry.add_ndi(key, config, src);
                }
            }
            Protocol::Decklink => {
                let config = match self.source_placement_config(&Protocol::Decklink, name) {
                    Some(SourceConfig::Decklink(c)) => c.clone(),
                    _ => DecklinkSourceConfig::default(),
                };
                let supported_connections = self
                    .decklink
                    .as_ref()
                    .and_then(|d| d.find_by_name(name))
                    .map(|p| p.connections)
                    .unwrap_or_default();
                self.registry.add_decklink(key, config, supported_connections);
            }
            #[cfg(target_os = "macos")]
            Protocol::Syphon => {
                if let Some(ref discovery) = self.syphon
                    && let Some(info) = discovery.find_by_display_name(name)
                {
                    let config = match self.source_placement_config(&Protocol::Syphon, name) {
                        Some(SourceConfig::Syphon(c)) => c.clone(),
                        _ => SyphonSourceConfig::default(),
                    };
                    self.registry.add_syphon(key, config, info);
                }
            }
            #[cfg(target_os = "macos")]
            Protocol::AvFoundation => {
                if let Some(device) = self.avfoundation.as_ref().and_then(|d| d.find_by_name(name)) {
                    let config = AvFoundationSourceConfig {
                        device_unique_id: device.unique_id.clone(),
                        ..AvFoundationSourceConfig::default()
                    };
                    self.registry.add_avfoundation(key, config);
                }
            }
            #[cfg(target_os = "macos")]
            Protocol::ScreenCaptureKit => {
                if let Some(target) = self
                    .screencapturekit
                    .as_ref()
                    .and_then(|d| d.list().find_by_source_ref(name))
                {
                    let config = target.to_config();
                    // Persist identity to every placement sharing this source (window re-match needs it).
                    self.apply_config_to_placements(
                        &Protocol::ScreenCaptureKit,
                        name,
                        SourceConfig::ScreenCaptureKit(config.clone()),
                    );
                    let label = target.label.clone();
                    self.registry.add_screencapturekit(key, config, label);
                }
            }
            #[cfg(target_os = "windows")]
            Protocol::Spout => {
                let config = match self.source_placement_config(&Protocol::Spout, name) {
                    Some(SourceConfig::Spout(c)) => c.clone(),
                    _ => SpoutSourceConfig::default(),
                };
                self.registry.add_spout(key, config);
            }
            #[cfg(target_os = "windows")]
            Protocol::MediaFoundation => {
                let device = self
                    .mediafoundation
                    .as_ref()
                    .and_then(|d| d.find_by_id(name));
                let mut config = match self.source_placement_config(&Protocol::MediaFoundation, name) {
                    Some(SourceConfig::MediaFoundation(c)) => c.clone(),
                    _ => MediaFoundationSourceConfig::default(),
                };
                config.device_id = device
                    .as_ref()
                    .map(|d| d.id.clone())
                    .unwrap_or_else(|| name.to_string());
                let label = device.map(|d| d.name).unwrap_or_else(|| name.to_string());
                self.registry.add_mediafoundation(key, config, label);
            }
            #[cfg(target_os = "windows")]
            Protocol::DirectShow => {
                let device = self
                    .directshow
                    .as_ref()
                    .and_then(|d| d.find_by_id(name));
                let mut config = match self.source_placement_config(&Protocol::DirectShow, name) {
                    Some(SourceConfig::DirectShow(c)) => c.clone(),
                    _ => DirectShowSourceConfig::default(),
                };
                config.device_id = device
                    .as_ref()
                    .map(|d| d.id.clone())
                    .unwrap_or_else(|| name.to_string());
                let label = device.map(|d| d.name).unwrap_or_else(|| name.to_string());
                self.registry.add_directshow(key, config, label);
            }
            #[cfg(target_os = "windows")]
            Protocol::WindowsCapture => {
                if let Some(target) = self
                    .windowscapture
                    .as_ref()
                    .and_then(|d| d.list().find_by_source_ref(name))
                {
                    let mut config = target.to_config();
                    // Discovery only supplies target identity; keep the settings already chosen for this source.
                    if let Some(SourceConfig::WindowsCapture(existing)) =
                        self.source_placement_config(&Protocol::WindowsCapture, name)
                    {
                        *config.settings_mut() = existing.settings().clone();
                    }
                    self.apply_config_to_placements(
                        &Protocol::WindowsCapture,
                        name,
                        SourceConfig::WindowsCapture(config.clone()),
                    );
                    let label = target.label.clone();
                    self.registry.add_windows_capture(key, config, label);
                }
            }
            // Unavailable protocols have no runtime source to connect.
            Protocol::Unknown(_) => {}
        }
    }

    pub fn add_source(&mut self) -> String {
        let key = self.registry.add_test(None);
        let num = self.cfg.canvas.sources.len() + 1;
        let source = Source::new(
            format!("Source {num}"),
            Protocol::Test,
            Some(key.source_ref),
            self.cfg.canvas.width as f32 * 0.25,
            self.cfg.canvas.height as f32 * 0.25,
            self.cfg.canvas.width / 2,
            self.cfg.canvas.height / 2,
            self.cfg.canvas.sources.len() as i32,
            TextureMode::Fit,
            false,
            false,
        );
        let uuid = source.uuid.clone();
        self.cfg.canvas.sources.push(source);
        self.dirty = true;
        return uuid;
    }

    pub fn remove_source(&mut self, uuid: &str) {
        self.cfg.canvas.sources.retain(|l| l.uuid != uuid);
        if self.selected_source_id.as_deref() == Some(uuid) {
            self.selected_source_id = None;
        }
        if self.expanded_source_id.as_deref() == Some(uuid) {
            self.expanded_source_id = None;
        }
        self.dirty = true;
    }

    /// Persist a runtime protocol config edit. The runtime source is shared by
    /// every source placement bound to the same (protocol, source_ref), so the
    /// edit is written to all of them — one runtime source, one config.
    pub fn sync_source_config(&mut self, source_uuid: &str, config: SourceConfig) {
        let Some((protocol, source_ref)) = self
            .cfg
            .canvas
            .sources
            .iter()
            .find(|s| s.uuid == source_uuid)
            .map(|s| (s.protocol.clone(), s.source_ref.clone()))
        else {
            return;
        };
        for placement in self.cfg.canvas.sources.iter_mut() {
            let bound = placement.uuid == source_uuid
                || (source_ref.is_some()
                    && placement.protocol == protocol
                    && placement.source_ref == source_ref);
            if bound {
                placement.config = config.clone();
            }
        }
        self.dirty = true;
    }

    pub fn expand_source(&mut self, uuid: String) {
        self.expanded_source_id = Some(uuid);
    }

    pub fn clear_expanded_source(&mut self) {
        self.expanded_source_id = None;
    }

    pub fn expanded_source_id(&self) -> Option<&str> {
        self.expanded_source_id.as_deref()
    }

    /// Cycle the selection by `step` sources, wrapping in either direction.
    pub fn select_source(&mut self, step: isize) {
        let len = self.cfg.canvas.sources.len();
        if len == 0 {
            self.selected_source_id = None;
            return;
        }
        let current = self
            .selected_source_id
            .as_ref()
            .and_then(|uuid| self.cfg.canvas.sources.iter().position(|s| &s.uuid == uuid));
        let next = match current {
            Some(i) => (i as isize + step).rem_euclid(len as isize) as usize,
            None if step < 0 => len - 1,
            None => 0,
        };
        self.selected_source_id = Some(self.cfg.canvas.sources[next].uuid.clone());
    }

    pub fn move_source(&mut self, from_index: usize, to_index: usize) {
        let len = self.cfg.canvas.sources.len();
        if from_index == to_index || from_index >= len || to_index >= len {
            return;
        }
        let source = self.cfg.canvas.sources.remove(from_index);
        self.cfg.canvas.sources.insert(to_index, source);
        self.dirty = true;
    }
}
