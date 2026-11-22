use crate::config::OutputId;
use crate::sources::{DecklinkOutputPort, OutputStats, Protocol, SourceConfig, SourceKey, SourceKind, SourceRuntimeConfig, SourceStats};

use super::Engine;

/// Read-only runtime data the UI needs without touching the live registries.
#[derive(Clone, Default)]
pub struct RuntimeView {
    pub sources: Vec<SourceView>,
    pub outputs: Vec<OutputView>,
    pub discoveries: Vec<DiscoveryView>,
    pub decklink_outputs: Vec<DecklinkOutputPort>,
    pub control: ControlView,
}

/// Listener state for the snapshot-driven Control settings window.
#[derive(Clone, Default)]
pub struct ControlView {
    pub osc: String,
}

#[derive(Clone)]
pub struct SourceView {
    pub key: SourceKey,
    pub runtime: SourceRuntimeConfig,
    pub stats: SourceStats,
    pub restart_pending: bool,
}

#[derive(Clone)]
pub struct OutputView {
    pub id: OutputId,
    pub protocol: Protocol,
    pub stats: OutputStats,
}

#[derive(Clone)]
pub struct DiscoveryView {
    pub protocol: Protocol,
    pub connected: Vec<PickerItemView>,
    pub groups: Vec<PickerGroupView>,
}

#[derive(Clone, Default)]
pub struct PickerItemView {
    pub id: String,
    pub label: String,
}

impl PickerItemView {
    pub fn plain(id: &str) -> Self {
        return Self {
            id: id.to_string(),
            label: id.to_string(),
        };
    }

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "windows")),
        allow(dead_code) // Only discovered labels on macOS/Windows differ from their id.
    )]
    pub fn new(id: &str, label: &str) -> Self {
        return Self {
            id: id.to_string(),
            label: label.to_string(),
        };
    }
}

#[derive(Clone, Default)]
pub struct PickerGroupView {
    pub header: Option<String>,
    pub items: Vec<PickerItemView>,
}

impl PickerGroupView {
    pub fn flat(items: Vec<PickerItemView>) -> Self {
        return Self { header: None, items };
    }
}

impl RuntimeView {
    pub fn discovery(&self, protocol: &Protocol) -> Option<&DiscoveryView> {
        return self.discoveries.iter().find(|view| view.protocol == *protocol);
    }

    pub fn source(&self, key: &SourceKey) -> Option<&SourceView> {
        return self.sources.iter().find(|view| view.key == *key);
    }
}

impl Engine {
    pub fn runtime_view(&self, focuses: &[Option<Protocol>]) -> RuntimeView {
        return RuntimeView {
            sources: self.source_views(),
            outputs: self.output_views(),
            discoveries: self.discovery_views(focuses),
            decklink_outputs: self.decklink.as_ref().map(|d| d.list_outputs()).unwrap_or_default(),
            control: ControlView {
                osc: self.control.status_label(),
            },
        };
    }

    fn source_views(&self) -> Vec<SourceView> {
        let mut views = Vec::new();
        for (key, kind) in self.registry.iter() {
            views.push(SourceView {
                key: key.clone(),
                runtime: kind.runtime.clone(),
                stats: kind.stats().lock().unwrap().clone(),
                restart_pending: self.registry.is_pending_restart(key),
            });
        }
        return views;
    }

    fn output_views(&self) -> Vec<OutputView> {
        let mut views = Vec::new();
        for output in &self.state.cfg.canvas.outputs {
            let Some(kind) = self.output_registry.get(&output.uuid) else {
                continue;
            };
            views.push(OutputView {
                id: output.uuid.clone(),
                protocol: output.protocol.clone(),
                stats: kind.stats().lock().unwrap().clone(),
            });
        }
        return views;
    }

    fn discovery_views(&self, focuses: &[Option<Protocol>]) -> Vec<DiscoveryView> {
        let mut views = Vec::new();

        self.push_discovery(&mut views, Protocol::Test, Vec::new());
        let ndi = if wants(focuses, &Protocol::Ndi) {
            self.ndi.as_ref().map(|d| d.list().iter().map(|source| PickerItemView::plain(&source.name)).collect()).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.push_discovery(&mut views, Protocol::Ndi, flat(ndi));

        let omt = if wants(focuses, &Protocol::Omt) {
            self.omt
                .as_ref()
                .map(|d| d.list().iter().map(|source| PickerItemView::plain(&source.instance_name())).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.push_discovery(&mut views, Protocol::Omt, flat(omt));

        let decklink = if wants(focuses, &Protocol::Decklink) {
            self.decklink.as_ref().map(|d| d.list().iter().map(|port| PickerItemView::plain(&port.name)).collect()).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.push_discovery(&mut views, Protocol::Decklink, flat(decklink));

        self.push_discovery(&mut views, Protocol::Vnc, Vec::new());
        self.push_discovery(&mut views, Protocol::Rdp, Vec::new());
        self.push_discovery(&mut views, Protocol::Text, Vec::new());

        #[cfg(target_os = "macos")]
        {
            let syphon = if wants(focuses, &Protocol::Syphon) {
                self.syphon
                    .as_ref()
                    .map(|d| {
                        d.list()
                            .iter()
                            .map(|info| PickerItemView::new(info.display_name(), &crate::sources::format_syphon_label(&info.app_name, &info.name)))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            self.push_discovery(&mut views, Protocol::Syphon, flat(syphon));

            let avfoundation = if wants(focuses, &Protocol::AvFoundation) {
                self.avfoundation
                    .as_ref()
                    .map(|d| d.list().iter().map(|device| PickerItemView::plain(&device.name)).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            self.push_discovery(&mut views, Protocol::AvFoundation, flat(avfoundation));

            let targets = if wants(focuses, &Protocol::ScreenCaptureKit) {
                self.screencapturekit.as_ref().map(|d| d.list()).unwrap_or_default()
            } else {
                Default::default()
            };
            self.push_discovery(
                &mut views,
                Protocol::ScreenCaptureKit,
                capture_groups(targets.displays.iter().map(target_item), targets.windows.iter().map(target_item)),
            );
        }

        #[cfg(target_os = "windows")]
        {
            let spout = if wants(focuses, &Protocol::Spout) {
                self.spout.as_ref().map(|d| d.list().iter().map(|name| PickerItemView::plain(name)).collect()).unwrap_or_default()
            } else {
                Vec::new()
            };
            self.push_discovery(&mut views, Protocol::Spout, flat(spout));

            let mediafoundation = if wants(focuses, &Protocol::MediaFoundation) {
                self.mediafoundation
                    .as_ref()
                    .map(|d| d.list().iter().map(|device| PickerItemView::new(&device.id, &device.name)).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            self.push_discovery(&mut views, Protocol::MediaFoundation, flat(mediafoundation));

            let directshow = if wants(focuses, &Protocol::DirectShow) {
                self.directshow
                    .as_ref()
                    .map(|d| d.list().iter().map(|device| PickerItemView::new(&device.id, &device.name)).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            self.push_discovery(&mut views, Protocol::DirectShow, flat(directshow));

            let targets = if wants(focuses, &Protocol::WindowsCapture) {
                self.windowscapture.as_ref().map(|d| d.list()).unwrap_or_default()
            } else {
                Default::default()
            };
            self.push_discovery(
                &mut views,
                Protocol::WindowsCapture,
                capture_groups(targets.displays.iter().map(target_item), targets.windows.iter().map(target_item)),
            );
        }

        return views;
    }

    fn push_discovery(&self, views: &mut Vec<DiscoveryView>, protocol: Protocol, groups: Vec<PickerGroupView>) {
        views.push(DiscoveryView {
            connected: self.connected_items(&protocol),
            protocol,
            groups,
        });
    }

    fn connected_items(&self, protocol: &Protocol) -> Vec<PickerItemView> {
        return self
            .registry
            .list_sources(protocol.clone())
            .into_iter()
            .map(|(key, kind)| PickerItemView {
                id: key.source_ref.clone(),
                label: source_label(key, kind),
            })
            .collect();
    }
}

fn wants(focuses: &[Option<Protocol>], protocol: &Protocol) -> bool {
    return focuses.iter().flatten().any(|focus| focus == protocol);
}

fn flat(items: Vec<PickerItemView>) -> Vec<PickerGroupView> {
    if items.is_empty() {
        return Vec::new();
    }
    return vec![PickerGroupView::flat(items)];
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn capture_groups(displays: impl Iterator<Item = PickerItemView>, windows: impl Iterator<Item = PickerItemView>) -> Vec<PickerGroupView> {
    return vec![
        PickerGroupView {
            header: Some("Displays".to_string()),
            items: displays.collect(),
        },
        PickerGroupView {
            header: Some("Windows".to_string()),
            items: windows.collect(),
        },
    ];
}

#[cfg(target_os = "macos")]
fn target_item(target: &crate::sources::ScreenCaptureTarget) -> PickerItemView {
    return PickerItemView::new(&target.source_ref, &target.label);
}

#[cfg(target_os = "windows")]
fn target_item(target: &crate::sources::WindowsCaptureTarget) -> PickerItemView {
    return PickerItemView::new(&target.source_ref, &target.label);
}

fn source_label(key: &SourceKey, kind: &SourceKind) -> String {
    match &kind.config {
        SourceConfig::Vnc(config) => return config.label(&key.source_ref),
        SourceConfig::Rdp(config) => return config.label(&key.source_ref),
        _ => {}
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        return kind.display_label(&key.source_ref);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        return key.source_ref.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Canvas;

    #[test]
    fn discovery_focus_keeps_connected_lists_for_every_protocol() {
        let mut engine = Engine::test_instance(Canvas::default());
        engine.add_source();

        let view = engine.runtime_view(&[Some(Protocol::Ndi), None]);

        assert_eq!(view.discovery(&Protocol::Test).map(|discovery| discovery.connected.len()), Some(1));
        assert!(view.discovery(&Protocol::Ndi).is_some());
    }
}
