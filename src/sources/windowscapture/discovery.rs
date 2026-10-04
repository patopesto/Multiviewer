use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_capture::monitor::Monitor;
use windows_capture::window::Window;

use super::source::{WindowsCaptureSettings, WindowsCaptureSourceConfig};

const INCLUDE_UNTITLED_WINDOWS: bool = false;

#[derive(Clone, Debug)]
enum TargetKind {
    Display { device_name: String },
    Window { hwnd: u64, process_name: String, title: String },
}

/// A display or window target; `source_ref` is namespaced per kind (`display:…`, `window:…`).
#[derive(Clone, Debug)]
pub struct Target {
    pub source_ref: String,
    pub label: String,
    kind: TargetKind,
}

impl Target {
    fn display(device_name: String, friendly_name: String, width: u32, height: u32) -> Self {
        let label = format!("{friendly_name} ({width}x{height})");
        let source_ref = format!("display:{device_name}");
        return Self {
            label,
            kind: TargetKind::Display {
                device_name: device_name.clone(),
            },
            source_ref,
        };
    }

    fn window(hwnd: u64, process_name: String, title: String) -> Self {
        let source_ref = format!("window:{hwnd}");
        let label = if title.is_empty() {
            format!("{process_name} - window {hwnd}")
        } else {
            format!("{process_name} - {title}")
        };
        return Self {
            label,
            kind: TargetKind::Window {
                hwnd,
                process_name,
                title,
            },
            source_ref,
        };
    }

    /// Whether this is the target `source_ref` + `config` point at (windows re-match on process + title).
    pub fn matches(&self, source_ref: &str, config: &WindowsCaptureSourceConfig) -> bool {
        if self.source_ref == source_ref {
            return true;
        }
        let (
            TargetKind::Window { process_name, title, .. },
            WindowsCaptureSourceConfig::Window {
                process_name: saved_process,
                title: saved_title,
                ..
            },
        ) = (&self.kind, config)
        else {
            return false;
        };
        return process_name == saved_process && title == saved_title;
    }

    /// Identity of this target as it goes on the wire.
    pub fn to_config(&self) -> WindowsCaptureSourceConfig {
        return match &self.kind {
            TargetKind::Display { device_name } => WindowsCaptureSourceConfig::Display {
                device_name: device_name.clone(),
                settings: WindowsCaptureSettings::default(),
            },
            TargetKind::Window {
                hwnd,
                process_name,
                title,
            } => WindowsCaptureSourceConfig::Window {
                hwnd: *hwnd,
                process_name: process_name.clone(),
                title: title.clone(),
                settings: WindowsCaptureSettings::default(),
            },
        };
    }
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

fn fetch_targets() -> Result<Targets, String> {
    let self_pid = std::process::id();

    let monitors = Monitor::enumerate().map_err(|e| format!("monitor enumeration failed: {e}"))?;
    let mut displays = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let device_name = monitor
            .device_name()
            .unwrap_or_else(|_| "Unknown".to_string());
        let friendly_name = monitor.name().unwrap_or_else(|_| device_name.clone());
        let width = monitor.width().unwrap_or(0);
        let height = monitor.height().unwrap_or(0);
        displays.push(Target::display(device_name, friendly_name, width, height));
    }

    let raw_windows =
        Window::enumerate().map_err(|e| format!("window enumeration failed: {e}"))?;
    let mut windows = Vec::new();
    for window in raw_windows {
        let pid = window.process_id().unwrap_or(0);
        if pid == self_pid {
            continue;
        }
        let title = window.title().unwrap_or_default();
        if !INCLUDE_UNTITLED_WINDOWS && title.is_empty() {
            continue;
        }
        let process_name = window
            .process_name()
            .unwrap_or_else(|_| "unknown".to_string());
        let hwnd = window.as_raw_hwnd() as u64;
        windows.push((hwnd, process_name, title));
    }
    sort_windows(&mut windows);

    let windows = windows
        .into_iter()
        .map(|(hwnd, process_name, title)| Target::window(hwnd, process_name, title))
        .collect();
    return Ok(Targets { displays, windows });
}

/// Order windows by process name, then title (both case-insensitive).
fn sort_windows(windows: &mut [(u64, String, String)]) {
    windows.sort_by(|a, b| {
        return a
            .1
            .to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()));
    });
}

/// Background display/window discovery.
pub struct Discovery {
    targets: Arc<Mutex<Targets>>,
}

impl Discovery {
    pub fn start() -> Self {
        let targets = Arc::new(Mutex::new(Targets::default()));
        let targets2 = targets.clone();
        std::thread::Builder::new()
            .name("windowscapture-discovery".into())
            .spawn(move || {
                let mut last_error: Option<String> = None;
                loop {
                    match fetch_targets() {
                        Ok(list) => {
                            *targets2.lock().unwrap() = list;
                            last_error = None;
                        }
                        Err(e) => {
                            *targets2.lock().unwrap() = Targets::default();
                            if last_error.as_deref() != Some(e.as_str()) {
                                tracing::error!("Windows Capture discovery failed: {e}");
                                last_error = Some(e);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("spawn windowscapture-discovery");
        return Self { targets };
    }

    pub fn list(&self) -> Targets {
        return self.targets.lock().unwrap().clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// App-grouped, alphabetical, case-insensitive - not window-server order.
    #[test]
    fn sort_windows_groups_by_process_then_title() {
        let mut windows = vec![
            (1, "notepad.exe".into(), "untitled".into()),
            (2, "chrome.exe".into(), "Tabs".into()),
            (3, "notepad.exe".into(), "Notes".into()),
            (4, "chrome.exe".into(), "apple".into()),
        ];
        sort_windows(&mut windows);

        let labels: Vec<(&str, &str)> = windows
            .iter()
            .map(|(_, process, title)| (process.as_str(), title.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("chrome.exe", "apple"),
                ("chrome.exe", "Tabs"),
                ("notepad.exe", "Notes"),
                ("notepad.exe", "untitled"),
            ]
        );
    }

    /// A window with a stale hwnd re-matches on process + title.
    #[test]
    fn target_matches_on_process_and_title_when_hwnd_changes() {
        let target = Target::window(1234, "notepad.exe".into(), "Notes".into());
        let config = WindowsCaptureSourceConfig::Window {
            hwnd: 9999,
            process_name: "notepad.exe".into(),
            title: "Notes".into(),
            settings: WindowsCaptureSettings::default(),
        };
        assert!(target.matches("window:9999", &config));

        let wrong_title = WindowsCaptureSourceConfig::Window {
            hwnd: 9999,
            process_name: "notepad.exe".into(),
            title: "Other".into(),
            settings: WindowsCaptureSettings::default(),
        };
        assert!(!target.matches("window:9999", &wrong_title));
    }
}
