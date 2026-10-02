use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_graphics2::window::{preflight_screen_capture_access, request_screen_capture_access};
use objc2::rc::Retained;
use screen_capture_kit::shareable_content::SCShareableContent;

use super::source::ScreenCaptureKitSourceConfig;

const INCLUDE_UNTITLED_WINDOWS: bool = false;

#[derive(Clone, Debug)]
enum TargetKind {
    Display {
        display_id: String,
    },
    Window {
        window_id: u32,
        bundle_id: String,
        title: String,
    },
}

/// A display or window target; `source_ref` is namespaced per kind (`display:…`, `window:…`).
#[derive(Clone, Debug)]
pub struct Target {
    pub source_ref: String,
    pub label: String,
    kind: TargetKind,
}

impl Target {
    fn display(display_id: u32, width: u32, height: u32) -> Self {
        let display_id = display_id.to_string();
        let label = format_display_label(&display_id, width, height);
        return Self {
            label,
            kind: TargetKind::Display {
                display_id: display_id.clone(),
            },
            source_ref: format!("display:{display_id}"),
        };
    }

    fn window(window_id: u32, bundle_id: String, app_name: String, title: String) -> Self {
        let source_ref = format!("window:{window_id}");
        let label = if title.is_empty() {
            format!("{app_name} — window {window_id}")
        } else {
            format!("{app_name} — {title}")
        };
        return Self {
            label,
            kind: TargetKind::Window {
                window_id,
                bundle_id,
                title,
            },
            source_ref,
        };
    }

    /// Whether this is the target `source_ref` + `config` point at (windows re-match on app + title).
    pub fn matches(&self, source_ref: &str, config: &ScreenCaptureKitSourceConfig) -> bool {
        if self.source_ref == source_ref {
            return true;
        }
        let (
            TargetKind::Window { bundle_id, title, .. },
            ScreenCaptureKitSourceConfig::Window {
                bundle_id: saved_bundle,
                title: saved_title,
                ..
            },
        ) = (&self.kind, config)
        else {
            return false;
        };
        return bundle_id == saved_bundle && title == saved_title;
    }

    /// Identity of this target as it goes on the wire.
    pub fn to_config(&self) -> ScreenCaptureKitSourceConfig {
        return match &self.kind {
            TargetKind::Display { display_id } => ScreenCaptureKitSourceConfig::Display {
                display_id: display_id.clone(),
            },
            TargetKind::Window {
                window_id,
                bundle_id,
                title,
            } => ScreenCaptureKitSourceConfig::Window {
                window_id: *window_id,
                bundle_id: bundle_id.clone(),
                title: title.clone(),
            },
        };
    }
}

fn format_display_label(display_id: &str, width: u32, height: u32) -> String {
    return format!("Display {display_id} ({width}x{height})");
}

/// One discovery snapshot, grouped so the UI can render section headers.
#[derive(Clone, Debug, Default)]
pub struct Targets {
    pub displays: Vec<Target>,
    pub windows: Vec<Target>,
}

impl Targets {
    pub fn iter(&self) -> impl Iterator<Item = &Target> {
        return self.displays.iter().chain(self.windows.iter());
    }

    pub fn find_by_source_ref(&self, source_ref: &str) -> Option<Target> {
        return self.iter().find(|t| t.source_ref == source_ref).cloned();
    }

    pub fn is_empty(&self) -> bool {
        return self.displays.is_empty() && self.windows.is_empty();
    }
}

/// Block on the async `SCShareableContent` callback and return the content.
pub(super) fn fetch_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    // onScreenWindowsOnly = NO: other-Space windows stay listed; the capture loop checks visibility.
    SCShareableContent::get_shareable_content_excluding_desktop_windows(
        true,
        false,
        move |content, error| {
            let result = match content {
                Some(content) => Ok(content),
                None => Err(error
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "no shareable content".to_string())),
            };
            let _ = tx.send(result);
        },
    );
    return rx.recv().map_err(|_| "shareable content request dropped".to_string())?;
}

fn fetch_targets() -> Result<Targets, String> {
    let content = fetch_content()?;
    let self_pid = std::process::id() as i32;

    // Layer-0 windows of other apps, on any Space.
    let mut windows = Vec::new();
    for window in content.windows().iter() {
        if window.window_layer() != 0 {
            continue;
        }
        let frame = window.frame();
        if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
            continue;
        }
        let Some(owner) = window.owning_application() else {
            continue;
        };
        if owner.process_id() == self_pid {
            continue;
        }
        let title = window.title().map(|t| t.to_string()).unwrap_or_default();
        if !INCLUDE_UNTITLED_WINDOWS && title.is_empty() {
            continue;
        }
        windows.push((
            window.window_id(),
            owner.bundle_identifier().to_string(),
            owner.application_name().to_string(),
            title,
        ));
    }

    // Case-insensitive app-then-title order groups each app's windows.
    sort_windows(&mut windows);

    let mut targets = Targets::default();
    for display in content.displays().iter() {
        targets
            .displays
            .push(Target::display(display.display_id(), display.width() as u32, display.height() as u32));
    }
    for (window_id, bundle_id, app_name, title) in windows {
        targets.windows.push(Target::window(
            window_id,
            bundle_id,
            app_name,
            title,
        ));
    }
    return Ok(targets);
}

/// Order windows by app name, then title (both case-insensitive)
fn sort_windows(windows: &mut [(u32, String, String, String)]) {
    windows.sort_by(|a, b| {
        return a
            .2
            .to_lowercase()
            .cmp(&b.2.to_lowercase())
            .then_with(|| a.3.to_lowercase().cmp(&b.3.to_lowercase()));
    });
}

/// One prompt per process: macOS caches the TCC decision.
static PROMPTED: AtomicBool = AtomicBool::new(false);

/// Ask for screen-recording permission once per process, off the UI thread.
pub fn ensure_screen_capture_access_requested() {
    if PROMPTED.load(Ordering::Relaxed) {
        return;
    }
    if preflight_screen_capture_access() {
        PROMPTED.store(true, Ordering::Relaxed);
        return;
    }
    if PROMPTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        request_screen_capture_access();
    });
}

/// Background shareable-content discovery.
pub struct Discovery {
    targets: Arc<Mutex<Targets>>,
}

impl Discovery {
    pub fn start() -> Self {
        let targets = Arc::new(Mutex::new(Targets::default()));
        let targets2 = targets.clone();
        std::thread::Builder::new()
            .name("screencapturekit-discovery".into())
            .spawn(move || {
                // Screen-recording permission errors repeat every cycle; log each distinct one once instead of every 2 seconds.
                let mut last_error: Option<String> = None;
                loop {
                    // Preflight never prompts; SCShareableContent does.
                    if !preflight_screen_capture_access() {
                        *targets2.lock().unwrap() = Targets::default();
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                    match fetch_targets() {
                        Ok(list) => {
                            *targets2.lock().unwrap() = list;
                            last_error = None;
                        }
                        Err(e) => {
                            *targets2.lock().unwrap() = Targets::default();
                            if last_error.as_deref() != Some(e.as_str()) {
                                tracing::error!("ScreenCaptureKit discovery failed: {e}");
                                last_error = Some(e);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn screencapturekit-discovery");
        Self { targets }
    }

    pub fn list(&self) -> Targets {
        self.targets.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// App-grouped, alphabetical, case-insensitive — not window-server order.
    #[test]
    fn sort_windows_groups_by_app_then_title() {
        let mut windows = vec![
            (1, "b".into(), "TextEdit".into(), "untitled".into()),
            (2, "a".into(), "Safari".into(), "Tabs".into()),
            (3, "b".into(), "TextEdit".into(), "Notes".into()),
            (4, "a".into(), "Safari".into(), "apple".into()),
        ];
        sort_windows(&mut windows);

        let labels: Vec<(&str, &str)> = windows
            .iter()
            .map(|(_, _, app, title)| (app.as_str(), title.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("Safari", "apple"),
                ("Safari", "Tabs"),
                ("TextEdit", "Notes"),
                ("TextEdit", "untitled"),
            ]
        );
    }
}
