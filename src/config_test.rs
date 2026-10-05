use super::*;
use crate::sources::TestSourceConfig;
use crate::sources::{NdiSourceConfig, NdiOutputConfig, NdiReceiverBandwidth, NdiReceiverColorFormat};
use crate::sources::{DecklinkSourceConfig, DecklinkOutputConfig, DecklinkVideoConnection, DecklinkMode};
#[cfg(target_os = "windows")]
use crate::sources::PixelFormat;
#[cfg(target_os = "macos")]
use crate::sources::SyphonOutputConfig;
#[cfg(target_os = "macos")]
use crate::sources::ScreenCaptureKitSourceConfig;
#[cfg(target_os = "windows")]
use crate::sources::SpoutOutputConfig;
#[cfg(target_os = "windows")]
use crate::sources::{WindowsCaptureSourceConfig, WindowsCaptureBorder, WindowsCaptureCursor, WindowsCaptureSecondaryWindows};

// Syphon output types only exist on macOS; other platforms exercise the
// same round-trip through the unavailable-protocol tests below.
#[cfg(target_os = "macos")]
#[test]
fn output_config_round_trips() {
    let output = Output::new(
        "Multiviewer".to_string(),
        Protocol::Syphon,
        true,
        OutputConfig::Syphon(SyphonOutputConfig {
            server_name: "MyServer".to_string(),
        }),
    );
    let json = serde_json::to_string(&output).unwrap();
    let parsed: Output = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "Multiviewer");
    assert_eq!(parsed.protocol, Protocol::Syphon);
    assert!(parsed.enabled);
    match parsed.config {
        OutputConfig::Syphon(cfg) => assert_eq!(cfg.server_name, "MyServer"),
        _ => panic!("expected Syphon config"),
    }
}

#[test]
fn output_config_is_required() {
    let json = r#"{"uuid":"abc","name":"Test","protocol":"Syphon","enabled":true}"#;
    assert!(serde_json::from_str::<Output>(json).is_err());
}

/// The "no config" default still round-trips in its historical wire form.
#[test]
fn default_output_config_wire_form() {
    let v = serde_json::to_value(OutputConfig::default()).unwrap();
    assert_eq!(v, serde_json::json!({"protocol": "Unknown"}));
}

/// A macOS-authored Syphon output must load on any platform and be written
/// back byte-for-byte compatible, so opening the project on macOS again
/// restores it untouched.
#[test]
fn syphon_output_round_trips_on_unavailable_platform() {
    let json = r#"{"uuid":"out1","name":"Program","protocol":"Syphon","enabled":true,
        "config":{"protocol":"Syphon","server_name":"Multiviewer"}}"#;
    let parsed: Output = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(parsed.protocol, Protocol::Syphon);
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("Syphon".to_string()));
        assert_eq!(parsed.protocol.label(), "Syphon (Unavailable)");
        // The raw config is preserved rather than interpreted.
        let OutputConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(v.get("server_name"), Some(&serde_json::json!("Multiviewer")));
    }

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "Syphon");
    assert_eq!(saved["config"]["server_name"], "Multiviewer");
}

#[cfg(target_os = "windows")]
#[test]
fn spout_output_config_round_trips() {
    let output = Output::new(
        "My Spout".into(),
        Protocol::Spout,
        true,
        OutputConfig::Spout(SpoutOutputConfig {
            sender_name: "Program".into(),
        }),
    );
    let json = serde_json::to_string(&output).unwrap();
    let parsed: Output = serde_json::from_str(&json).unwrap();
    match parsed.config {
        OutputConfig::Spout(c) => assert_eq!(c.sender_name, "Program"),
        _ => panic!("expected Spout config"),
    }
}

/// A Windows-authored Spout output must load on any platform and be written
/// back byte-for-byte compatible, so opening the project on Windows again restores it untouched.
#[test]
fn spout_output_round_trips_on_unavailable_platform() {
    let json = r#"{"uuid":"out1","name":"Program","protocol":"Spout","enabled":true,
        "config":{"protocol":"Spout","sender_name":"Multiviewer"}}"#;
    let parsed: Output = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "windows")]
    {
        assert_eq!(parsed.protocol, Protocol::Spout);
        let OutputConfig::Spout(c) = &parsed.config else {
            panic!("expected Spout config")
        };
        assert_eq!(c.sender_name, "Multiviewer");
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("Spout".to_string()));
        assert_eq!(parsed.protocol.label(), "Spout (Unavailable)");
        // The raw config is preserved rather than interpreted.
        let OutputConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(v.get("sender_name"), Some(&serde_json::json!("Multiviewer")));
    }

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "Spout");
    assert_eq!(saved["config"]["sender_name"], "Multiviewer");
}

/// A macOS-authored AVFoundation source (device-specific config) must
/// survive load + save on a platform that has no AVFoundation.
#[test]
fn avfoundation_source_round_trips_on_unavailable_platform() {
    let json = r#"{
        "uuid":"u1","name":"Camera","protocol":"AvFoundation",
        "source_ref":"FaceTime HD Camera","x":10.0,"y":20.0,
        "width":1920,"height":1080,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"AvFoundation","device_unique_id":"0x802000000a5f123"}
    }"#;
    let parsed: Source = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(parsed.protocol, Protocol::AvFoundation);
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("AvFoundation".to_string()));
        assert_eq!(parsed.protocol.label(), "AvFoundation (Unavailable)");
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(
            v.get("device_unique_id"),
            Some(&serde_json::json!("0x802000000a5f123"))
        );
    }
    // Layout is independent of protocol availability.
    assert_eq!(parsed.name, "Camera");
    assert_eq!(parsed.x, 10.0);
    assert_eq!(parsed.z, 3);

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "AvFoundation");
    assert_eq!(saved["config"]["device_unique_id"], "0x802000000a5f123");
}

/// A Windows-authored Media Foundation source (device-specific config) must
/// survive load + save on a platform that has no Media Foundation.
#[test]
fn mediafoundation_source_round_trips_on_unavailable_platform() {
    let json = r#"{
        "uuid":"u1","name":"Webcam","protocol":"MediaFoundation",
        "source_ref":"\\\\?\\usb#vid_046d&pid_085b","x":10.0,"y":20.0,
        "width":1920,"height":1080,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"MediaFoundation","device_id":"\\\\?\\usb#vid_046d&pid_085b","width":0,"height":0,"fps_num":0,"fps_den":0,"pixel_format":"Bgra8"}
    }"#;
    let parsed: Source = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "windows")]
    {
        assert_eq!(parsed.protocol, Protocol::MediaFoundation);
        let SourceConfig::MediaFoundation(c) = &parsed.config else {
            panic!("expected MediaFoundation config")
        };
        assert_eq!(c.device_id, r"\\?\usb#vid_046d&pid_085b");
        assert_eq!(c.pixel_format, Some(PixelFormat::Bgra8));
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("MediaFoundation".to_string()));
        assert_eq!(parsed.protocol.label(), "MediaFoundation (Unavailable)");
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(
            v.get("device_id"),
            Some(&serde_json::json!(r"\\?\usb#vid_046d&pid_085b"))
        );
        assert_eq!(v.get("pixel_format"), Some(&serde_json::json!("Bgra8")));
    }
    // Layout is independent of protocol availability.
    assert_eq!(parsed.name, "Webcam");
    assert_eq!(parsed.x, 10.0);

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "MediaFoundation");
    assert_eq!(
        saved["config"]["device_id"],
        r"\\?\usb#vid_046d&pid_085b"
    );
    assert_eq!(saved["config"]["pixel_format"], "Bgra8");
}

/// A Windows-authored DirectShow source (device-specific config) must
/// survive load + save on a platform that has no DirectShow.
#[test]
fn directshow_source_round_trips_on_unavailable_platform() {
    let json = r#"{
        "uuid":"u1","name":"DeckLink","protocol":"DirectShow",
        "source_ref":"\\\\?\\usb#vid_1edb&pid_bd3f","x":10.0,"y":20.0,
        "width":1920,"height":1080,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"DirectShow","device_id":"\\\\?\\usb#vid_1edb&pid_bd3f","width":0,"height":0,"fps_num":0,"fps_den":0,"pixel_format":"Yuy2"}
    }"#;
    let parsed: Source = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "windows")]
    {
        assert_eq!(parsed.protocol, Protocol::DirectShow);
        let SourceConfig::DirectShow(c) = &parsed.config else {
            panic!("expected DirectShow config")
        };
        assert_eq!(c.device_id, r"\\?\usb#vid_1edb&pid_bd3f");
        assert_eq!(c.pixel_format, Some(PixelFormat::Yuy2));
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("DirectShow".to_string()));
        assert_eq!(parsed.protocol.label(), "DirectShow (Unavailable)");
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(
            v.get("device_id"),
            Some(&serde_json::json!(r"\\?\usb#vid_1edb&pid_bd3f"))
        );
        assert_eq!(v.get("pixel_format"), Some(&serde_json::json!("Yuy2")));
    }
    assert_eq!(parsed.name, "DeckLink");

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "DirectShow");
    assert_eq!(
        saved["config"]["device_id"],
        r"\\?\usb#vid_1edb&pid_bd3f"
    );
}

/// A macOS-authored ScreenCaptureKit source must survive load + save on a platform without it.
#[test]
fn screencapturekit_source_round_trips_on_unavailable_platform() {
    let json = r#"{
        "uuid":"u1","name":"Screen","protocol":"ScreenCaptureKit",
        "source_ref":"display:724561234","x":10.0,"y":20.0,
        "width":1920,"height":1080,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"ScreenCaptureKit","kind":"display",
                  "display_id":"724561234"}
    }"#;
    let parsed: Source = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "macos")]
    {
        assert_eq!(parsed.protocol, Protocol::ScreenCaptureKit);
        let SourceConfig::ScreenCaptureKit(c) = &parsed.config else {
            panic!("expected ScreenCaptureKit config")
        };
        let ScreenCaptureKitSourceConfig::Display { display_id } = c else {
            panic!("expected display target, got {c:?}")
        };
        assert_eq!(display_id, "724561234");
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("ScreenCaptureKit".to_string()));
        assert_eq!(parsed.protocol.label(), "ScreenCaptureKit (Unavailable)");
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(
            v.get("display_id"),
            Some(&serde_json::json!("724561234"))
        );
    }
    // Layout is independent of protocol availability.
    assert_eq!(parsed.name, "Screen");
    assert_eq!(parsed.source_ref.as_deref(), Some("display:724561234"));

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "ScreenCaptureKit");
    assert_eq!(saved["config"]["kind"], "display");
    assert_eq!(saved["config"]["display_id"], "724561234");
}

/// Window ScreenCaptureKit sources must survive load + save on a platform that has no ScreenCaptureKit.
#[test]
fn screencapturekit_window_source_round_trip() {
    let window_json = r#"{
        "uuid":"u1","name":"Doc","protocol":"ScreenCaptureKit",
        "source_ref":"window:4242","x":10.0,"y":20.0,
        "width":800,"height":600,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"ScreenCaptureKit","kind":"window",
                  "window_id":4242,"bundle_id":"com.apple.TextEdit",
                  "title":"Untitled"}
    }"#;
    let parsed: Source = serde_json::from_str(window_json).unwrap();
    #[cfg(target_os = "macos")]
    {
        assert_eq!(parsed.protocol, Protocol::ScreenCaptureKit);
        let SourceConfig::ScreenCaptureKit(c) = &parsed.config else {
            panic!("expected ScreenCaptureKit config")
        };
        let ScreenCaptureKitSourceConfig::Window {
            window_id,
            bundle_id,
            title,
        } = c
        else {
            panic!("expected window target, got {c:?}")
        };
        assert_eq!(*window_id, 4242);
        assert_eq!(bundle_id, "com.apple.TextEdit");
        assert_eq!(title, "Untitled");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(v.get("window_id"), Some(&serde_json::json!(4242)));
        assert_eq!(parsed.protocol, Protocol::Unknown("ScreenCaptureKit".to_string()));
    }
    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["config"]["kind"], "window");
    assert_eq!(saved["config"]["window_id"], 4242);
    assert_eq!(saved["config"]["title"], "Untitled");
    // A target variant only carries its own fields.
    assert!(saved["config"].get("display_id").is_none());
}

/// A Windows-authored Windows Graphics Capture source must survive load + save
/// on a platform without it.
#[test]
fn windows_capture_source_round_trips_on_unavailable_platform() {
    let json = r#"{
        "uuid":"u1","name":"Screen","protocol":"WindowsCapture",
        "source_ref":"display:\\\\.\\DISPLAY1","x":10.0,"y":20.0,
        "width":1920,"height":1080,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"WindowsCapture","kind":"display",
                  "device_name":"\\\\.\\DISPLAY1",
                  "settings":{"cursor":"default","border":"default","secondary_windows":"default"}}
    }"#;
    let parsed: Source = serde_json::from_str(json).unwrap();
    #[cfg(target_os = "windows")]
    {
        assert_eq!(parsed.protocol, Protocol::WindowsCapture);
        let SourceConfig::WindowsCapture(c) = &parsed.config else {
            panic!("expected WindowsCapture config")
        };
        let WindowsCaptureSourceConfig::Display { device_name, .. } = c else {
            panic!("expected display target, got {c:?}")
        };
        assert_eq!(device_name, "\\\\.\\DISPLAY1");
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert_eq!(parsed.protocol, Protocol::Unknown("WindowsCapture".to_string()));
        assert_eq!(parsed.protocol.label(), "WindowsCapture (Unavailable)");
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(
            v.get("device_name"),
            Some(&serde_json::json!("\\\\.\\DISPLAY1"))
        );
    }
    assert_eq!(parsed.name, "Screen");
    assert_eq!(parsed.source_ref.as_deref(), Some("display:\\\\.\\DISPLAY1"));

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["protocol"], "WindowsCapture");
    assert_eq!(saved["config"]["kind"], "display");
    assert_eq!(saved["config"]["device_name"], "\\\\.\\DISPLAY1");
}

/// Window Windows Graphics Capture sources must survive load + save on a
/// platform that has no Windows Graphics Capture.
#[test]
fn windows_capture_window_source_round_trip() {
    let window_json = r#"{
        "uuid":"u1","name":"Doc","protocol":"WindowsCapture",
        "source_ref":"window:4242","x":10.0,"y":20.0,
        "width":800,"height":600,"z":3,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"WindowsCapture","kind":"window",
                  "hwnd":4242,"process_name":"notepad.exe",
                  "title":"Untitled",
                  "settings":{"cursor":"hide",
                              "border":"hide",
                              "secondary_windows":"include"}}
    }"#;
    let parsed: Source = serde_json::from_str(window_json).unwrap();
    #[cfg(target_os = "windows")]
    {
        assert_eq!(parsed.protocol, Protocol::WindowsCapture);
        let SourceConfig::WindowsCapture(c) = &parsed.config else {
            panic!("expected WindowsCapture config")
        };
        let WindowsCaptureSourceConfig::Window {
            hwnd,
            process_name,
            title,
            settings,
        } = c
        else {
            panic!("expected window target, got {c:?}")
        };
        assert_eq!(*hwnd, 4242);
        assert_eq!(process_name, "notepad.exe");
        assert_eq!(title, "Untitled");
        assert_eq!(settings.cursor, WindowsCaptureCursor::Hide);
        assert_eq!(settings.border, WindowsCaptureBorder::Hide);
        assert_eq!(
            settings.secondary_windows,
            WindowsCaptureSecondaryWindows::Include
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        let SourceConfig::Unknown(v) = &parsed.config else {
            panic!("expected Unknown config")
        };
        assert_eq!(v.get("hwnd"), Some(&serde_json::json!(4242)));
        assert_eq!(v["settings"]["cursor"], serde_json::json!("hide"));
        assert_eq!(parsed.protocol, Protocol::Unknown("WindowsCapture".to_string()));
    }
    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["config"]["kind"], "window");
    assert_eq!(saved["config"]["hwnd"], 4242);
    assert_eq!(saved["config"]["title"], "Untitled");
    assert_eq!(saved["config"]["settings"]["cursor"], "hide");
    assert_eq!(saved["config"]["settings"]["border"], "hide");
    assert_eq!(saved["config"]["settings"]["secondary_windows"], "include");
    assert!(saved["config"].get("device_name").is_none());
}

/// A protocol this build has never heard of (the same code path Windows
/// takes for Syphon) must load, keep its config, and save back untouched.
#[test]
fn unknown_protocol_source_round_trips() {
    let json = r#"{
        "canvas": {
            "width":1920, "height":1080,
            "label":{"visibility":"Hide","position":"TopLeft","size":24.0,"text_color":[255,255,255,255],"background_color":[0,0,0,180]},
            "border":{"visibility":"Show","color":[180,180,180,255],"width":1.0},
            "sources": [{
                "uuid":"u1","name":"Spout In","protocol":"fake","source_ref":"Spout1",
                "x":0.0,"y":0.0,"width":640,"height":360,"z":0,"mode":"Fit",
                "flip_h":false,"flip_v":false,
                "label_visibility":"Inherit","border_visibility":"Inherit",
                "config":{"protocol":"fake","name":"Game"}
            }],
            "outputs": [{
                "uuid":"o1","name":"Spout Out","protocol":"fake","enabled":true,
                "config":{"protocol":"fake","name":"Program"}
            }]
        }
    }"#;

    let dir = std::env::temp_dir().join(format!("multiviewer-unknown-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("show.multiviewer");
    std::fs::write(&path, json).unwrap();

    let cfg = Config::load_from(&path).unwrap();
    let source = &cfg.canvas.sources[0];
    assert_eq!(source.protocol, Protocol::Unknown("fake".to_string()));
    assert_eq!(source.protocol.label(), "fake (Unavailable)");
    let SourceConfig::Unknown(v) = &source.config else {
        panic!("expected Unknown config")
    };
    assert_eq!(v.get("name"), Some(&serde_json::json!("Game")));
    let output = &cfg.canvas.outputs[0];
    assert_eq!(output.protocol, Protocol::Unknown("fake".to_string()));
    let OutputConfig::Unknown(v) = &output.config else {
        panic!("expected Unknown config")
    };
    assert_eq!(v.get("name"), Some(&serde_json::json!("Program")));

    // Save and load again: nothing may be lost.
    cfg.save_to(&path).unwrap();
    let reloaded = Config::load_from(&path).unwrap();
    assert_eq!(reloaded.canvas.sources[0].protocol, Protocol::Unknown("fake".to_string()));
    let SourceConfig::Unknown(v) = &reloaded.canvas.sources[0].config else {
        panic!("expected Unknown config")
    };
    assert_eq!(v.get("name"), Some(&serde_json::json!("Game")));
    let OutputConfig::Unknown(v) = &reloaded.canvas.outputs[0].config else {
        panic!("expected Unknown config")
    };
    assert_eq!(v.get("name"), Some(&serde_json::json!("Program")));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn decklink_output_config_round_trips() {
    let output = Output::new(
        "My DeckLink".into(),
        Protocol::Decklink,
        true,
        OutputConfig::Decklink(DecklinkOutputConfig {
            device_name: "DeckLink Mini Monitor".into(),
            display_mode: DecklinkMode::Hd1080p6000,
            width: 1920,
            height: 1080,
            fps: 60.0,
        }),
    );
    let json = serde_json::to_string(&output).unwrap();
    assert!(json.contains("\"display_mode\":1215313456"));
    let parsed: Output = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed.config, OutputConfig::Decklink(_)));
    match parsed.config {
        OutputConfig::Decklink(c) => {
            assert_eq!(c.device_name, "DeckLink Mini Monitor");
            assert_eq!(c.display_mode, DecklinkMode::Hd1080p6000);
        }
        _ => panic!("expected Decklink config"),
    }
}

#[test]
fn ndi_output_config_round_trips() {
    let output = Output::new(
        "My NDI".into(),
        Protocol::Ndi,
        true,
        OutputConfig::Ndi(NdiOutputConfig {
            sender_name: "Studio".into(),
        }),
    );
    let json = serde_json::to_string(&output).unwrap();
    let parsed: Output = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed.config, OutputConfig::Ndi(_)));
    match parsed.config {
        OutputConfig::Ndi(c) => assert_eq!(c.sender_name, "Studio"),
        _ => panic!("expected Ndi config"),
    }
}

#[test]
fn test_source_config_round_trips() {
    let mut source = Source::new(
        "Test".to_string(),
        Protocol::Test,
        Some("test-1".to_string()),
        0.0, 0.0, 1280, 720, 0,
        TextureMode::Fit,
        false, false,
    );
    source.config = SourceConfig::Test(TestSourceConfig { 
        width: 1920, 
        height: 1080,
        ..Default::default() 
    });
    let json = serde_json::to_string(&source).unwrap();
    let parsed: Source = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.protocol, Protocol::Test);
    match parsed.config {
        SourceConfig::Test(c) => {
            assert_eq!(c.width, 1920);
            assert_eq!(c.height, 1080);
        }
        _ => panic!("expected Test source config"),
    }
}

#[test]
fn ndi_source_config_round_trips() {
    let mut source = Source::new(
        "NDI".to_string(),
        Protocol::Ndi,
        Some("ndi-1".to_string()),
        0.0, 0.0, 1920, 1080, 0,
        TextureMode::Fit,
        false, false,
    );
    source.config = SourceConfig::Ndi(NdiSourceConfig {
        bandwidth: NdiReceiverBandwidth::Highest,
        color_format: NdiReceiverColorFormat::UYVY_RGBA,
    });
    let json = serde_json::to_string(&source).unwrap();
    let parsed: Source = serde_json::from_str(&json).unwrap();
    match parsed.config {
        SourceConfig::Ndi(c) => {
            assert_eq!(c.bandwidth, NdiReceiverBandwidth::Highest);
            assert_eq!(c.color_format, NdiReceiverColorFormat::UYVY_RGBA);
        }
        _ => panic!("expected Ndi source config"),
    }
}

#[test]
fn decklink_source_config_round_trips() {
    let mut source = Source::new(
        "DeckLink".to_string(),
        Protocol::Decklink,
        Some("decklink-1".to_string()),
        0.0, 0.0, 1920, 1080, 0,
        TextureMode::Fit,
        false, false,
    );
    source.config = SourceConfig::Decklink(DecklinkSourceConfig {
        connection: DecklinkVideoConnection::Hdmi,
    });
    let json = serde_json::to_string(&source).unwrap();
    let parsed: Source = serde_json::from_str(&json).unwrap();
    match parsed.config {
        SourceConfig::Decklink(c) => {
            assert_eq!(c.connection, DecklinkVideoConnection::Hdmi);
        }
        _ => panic!("expected Decklink source config"),
    }
}

#[test]
fn config_round_trips_through_path() {
    let dir = std::env::temp_dir().join(format!("multiviewer-config-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.multiviewer");

    let mut cfg = Config::default();
    cfg.canvas.width = 1234;
    cfg.canvas.sources.push(Source::new(
        "Test".to_string(),
        Protocol::Test,
        Some("test-1".to_string()),
        0.0, 0.0, 1280, 720, 0,
        TextureMode::Fit,
        false, false,
    ));
    cfg.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();
    assert_eq!(loaded.canvas.width, 1234);
    assert_eq!(loaded.canvas.sources.len(), 1);
    assert_eq!(loaded.canvas.sources[0].protocol, Protocol::Test);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn source_ref_round_trips() {
    let json = r#"{
        "uuid":"u1","name":"Cam","protocol":"Test","source_ref":"Test A",
        "x":0.0,"y":0.0,"width":640,"height":360,"z":0,"mode":"Fit",
        "flip_h":false,"flip_v":false,
        "label_visibility":"Inherit","border_visibility":"Inherit",
        "config":{"protocol":"Test","width":1280,"height":720,
                  "pattern":{"Smpte":"Smpte2022"},
                  "cursor":{"enabled":false,"speed_x":8.0,"speed_y":2.0,"width":1}}
    }"#;
    let parsed: Source = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.source_ref.as_deref(), Some("Test A"));

    let saved = serde_json::to_value(&parsed).unwrap();
    assert_eq!(saved["source_ref"], "Test A");
}

/// Quads on the same protocol + source_ref share one runtime source: the
/// shared reference and its config survive save → load on both quads.
#[test]
fn quads_sharing_source_ref_round_trips() {
    let json = r#"{
        "canvas": {
            "width":1920, "height":1080,
            "label":{"visibility":"Hide","position":"TopLeft","size":24.0,"text_color":[255,255,255,255],"background_color":[0,0,0,180]},
            "border":{"visibility":"Show","color":[180,180,180,255],"width":1.0},
            "sources": [
                {"uuid":"u1","name":"Quad A","protocol":"Ndi","source_ref":"Cam (1)",
                 "x":0.0,"y":0.0,"width":960,"height":540,"z":0,"mode":"Fit",
                 "flip_h":false,"flip_v":false,
                 "label_visibility":"Inherit","border_visibility":"Inherit",
                 "config":{"protocol":"Ndi","bandwidth":"Highest","color_format":"UYVY_RGBA"}},
                {"uuid":"u2","name":"Quad B","protocol":"Ndi","source_ref":"Cam (1)",
                 "x":960.0,"y":0.0,"width":960,"height":540,"z":1,"mode":"Fit",
                 "flip_h":false,"flip_v":false,
                 "label_visibility":"Inherit","border_visibility":"Inherit",
                 "config":{"protocol":"Ndi","bandwidth":"Highest","color_format":"UYVY_RGBA"}}
            ],
            "outputs": []
        }
    }"#;

    let cfg: Config = serde_json::from_str(json).unwrap();
    let [a, b] = &cfg.canvas.sources[..] else {
        panic!("expected two sources")
    };
    // One shared runtime reference, with its config on every bound placement.
    assert_eq!(a.source_ref.as_deref(), Some("Cam (1)"));
    assert_eq!(b.source_ref.as_deref(), Some("Cam (1)"));
    let (SourceConfig::Ndi(ca), SourceConfig::Ndi(cb)) = (&a.config, &b.config) else {
        panic!("expected Ndi configs")
    };
    assert_eq!(ca.bandwidth, NdiReceiverBandwidth::Highest);
    assert_eq!(cb.bandwidth, NdiReceiverBandwidth::Highest);

    // Round-trip keeps the shared source_ref and config on both quads.
    let saved = serde_json::to_string(&cfg).unwrap();
    let reloaded: Config = serde_json::from_str(&saved).unwrap();
    let [a, b] = &reloaded.canvas.sources[..] else {
        panic!("expected two sources")
    };
    assert_eq!(a.source_ref.as_deref(), Some("Cam (1)"));
    assert_eq!(b.source_ref.as_deref(), Some("Cam (1)"));
    let (SourceConfig::Ndi(ca), SourceConfig::Ndi(cb)) = (&a.config, &b.config) else {
        panic!("expected Ndi configs")
    };
    assert_eq!(ca.bandwidth, NdiReceiverBandwidth::Highest);
    assert_eq!(cb.bandwidth, NdiReceiverBandwidth::Highest);
}

#[test]
fn config_default_stamps_current_version() {
    let v = serde_json::to_value(Config::default()).unwrap();
    assert_eq!(v["version"], serde_json::json!(CONFIG_VERSION));
}

#[test]
fn config_version_defaults_to_legacy_when_absent() {
    let mut v = serde_json::to_value(Config::default()).unwrap();
    v.as_object_mut().unwrap().remove("version");

    let cfg: Config = serde_json::from_value(v).unwrap();

    assert_eq!(cfg.version, LEGACY_VERSION);
}

#[test]
fn config_version_round_trips() {
    let mut cfg = Config::default();
    cfg.canvas.width = 1234;

    let json = serde_json::to_string(&cfg).unwrap();
    let reloaded: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(reloaded.version, CONFIG_VERSION);
    assert_eq!(reloaded.canvas.width, 1234);
}
