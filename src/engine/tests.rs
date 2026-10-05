use super::*;
use crate::compositor::{self, Rect};
use crate::config::{Canvas, Output, Source, TextureMode, CONFIG_VERSION};
use crate::engine::project::AUTO_SAVE_INTERVAL;
use crate::sources::{Protocol, SourceConfig, SourceKey, OutputConfig, DecklinkSourceConfig, NdiOutputConfig, NdiSourceConfig, DecklinkVideoConnection, NdiReceiverBandwidth, NdiReceiverColorFormat};
use std::time::{Duration, Instant};

fn test_engine(canvas: Canvas) -> Engine {
    Engine {
        cfg: Config { canvas, ..Config::default() },
        registry: SourceRegistry::new(),
        output_registry: OutputRegistry::new(),
        ndi: None,
        decklink: None,
        #[cfg(target_os = "macos")]
        syphon: None,
        #[cfg(target_os = "macos")]
        avfoundation: None,
        #[cfg(target_os = "macos")]
        screencapturekit: None,
        #[cfg(target_os = "windows")]
        spout: None,
        #[cfg(target_os = "windows")]
        mediafoundation: None,
        #[cfg(target_os = "windows")]
        directshow: None,
        #[cfg(target_os = "windows")]
        windowscapture: None,
        comp: None,
        device: None,
        queue: None,
        project_path: None,
        dirty: false,
        last_saved_at: Instant::now(),
        selected_source_id: None,
        expanded_source_id: None,
        drag_state: DragState::None,
        snap_guides: SnapGuides::default(),
        view: ViewState::new(),
        load_warnings: Vec::new(),
    }
}

/// A project authored where unavailable protocols exist (Syphon on
/// Windows, an unknown protocol anywhere) must load, warn in the status
/// bar, and never spawn a runtime source for it.
#[test]
fn load_project_with_unavailable_protocol_warns_and_skips_runtime() {
    let json = r#"{
        "canvas": {
            "width":1920, "height":1080,
            "label":{"visibility":"Hide","position":"TopLeft","size":24.0,"text_color":[255,255,255,255],"background_color":[0,0,0,180]},
            "border":{"visibility":"Show","color":[180,180,180,255],"width":1.0},
            "sources": [
                {"uuid":"u1","name":"Spout In","protocol":"fake","source_ref":"Spout1",
                 "x":0.0,"y":0.0,"width":640,"height":360,"z":0,"mode":"Fit",
                 "flip_h":false,"flip_v":false,
                 "label_visibility":"Inherit","border_visibility":"Inherit",
                 "config":{"protocol":"fake","name":"Game"}},
                {"uuid":"u2","name":"Bars","protocol":"Test","source_ref":"Test A",
                 "x":0.0,"y":0.0,"width":640,"height":360,"z":1,"mode":"Fit",
                 "flip_h":false,"flip_v":false,
                 "label_visibility":"Inherit","border_visibility":"Inherit",
                 "config":{"protocol":"Test","width":1280,"height":720,
                           "pattern":{"Smpte":"Smpte2022"},
                           "cursor":{"enabled":false,"speed_x":8.0,"speed_y":2.0,"width":1}}}
            ],
            "outputs":[
                {"uuid":"o1","name":"Spout Out","protocol":"fake","enabled":true,
                 "config":{"protocol":"fake","name":"Program"}}
            ]
        }
    }"#;

    let dir = std::env::temp_dir().join(format!("multiviewer-unavailable-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("show.multiviewer");
    std::fs::write(&path, json).unwrap();

    let cfg = Config::load_from(&path).unwrap();
    let mut engine = test_engine(cfg.canvas);
    engine.rebuild_from_config();

    // Both entries survive, the unavailable config is kept verbatim.
    assert_eq!(engine.cfg.canvas.sources.len(), 2);
    assert_eq!(engine.cfg.canvas.outputs.len(), 1);
    let SourceConfig::Unknown(v) = &engine.cfg.canvas.sources[0].config else {
        panic!("expected Unknown config")
    };
    assert_eq!(v.get("name"), Some(&serde_json::json!("Game")));

    // Only the Test source gets a runtime instance. The unavailable placement's
    // key is distinct from any live source that happens to share its ref.
    assert_eq!(engine.registry.iter().count(), 1);
    let unavailable_key =
        SourceKey::new(Protocol::Unknown("fake".to_string()), "Spout1".to_string());
    assert!(engine.registry.get(&unavailable_key).is_none());
    let test_key = SourceKey::new(
        Protocol::Test,
        engine.cfg.canvas.sources[1].source_ref.clone().unwrap(),
    );
    assert!(engine.registry.get(&test_key).is_some());

    // The status bar picks this up from load_warnings.
    assert_eq!(engine.load_warnings.len(), 1);
    assert!(engine.load_warnings[0].contains("fake"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn expand_and_clear_source() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(
        "L1".into(),
        Protocol::Test,
        None,
        100.0,
        100.0,
        100,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();

    assert!(engine.expanded_source_id().is_none());
    engine.expand_source(uuid.clone());
    assert_eq!(engine.expanded_source_id(), Some(uuid.as_str()));
    engine.clear_expanded_source();
    assert!(engine.expanded_source_id().is_none());
}

#[test]
fn removing_expanded_source_clears_it() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(
        "L1".into(),
        Protocol::Test,
        None,
        100.0,
        100.0,
        100,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();
    engine.expand_source(uuid.clone());

    engine.remove_source(&uuid);
    assert!(engine.expanded_source_id().is_none());
}

#[test]
fn display_transform_is_base_at_default_zoom() {
    let engine = test_engine(Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    });
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    };
    let (scale, ox, oy) = engine.display_transform(&panel);
    let (base_scale, base_ox, base_oy) =
        compositor::canvas_transform(&engine.cfg.canvas, &panel);
    assert!((scale - base_scale).abs() < 1e-3);
    assert!((ox - base_ox).abs() < 1e-3);
    assert!((oy - base_oy).abs() < 1e-3);
}

#[test]
fn recenter_fits_canvas_with_margin() {
    let mut engine = test_engine(Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    });
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    };
    engine.recenter_view(&panel);
    let (scale, ox, oy) = engine.display_transform(&panel);
    let expected_scale = (panel.w / 1920.0).min(panel.h / 1080.0) * 0.9;
    assert!((scale - expected_scale).abs() < 1e-3);
    assert!((ox - (panel.w - 1920.0 * scale) / 2.0).abs() < 1e-3);
    assert!((oy - (panel.h - 1080.0 * scale) / 2.0).abs() < 1e-3);
}

#[test]
fn recenter_expands_to_include_sources() {
    let mut canvas = Canvas {
        width: 100,
        height: 100,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(

        "L1".into(),
        Protocol::Test,
        None,
        -50.0,
        -50.0,
        100,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    canvas.sources.push(Source::new(
        "L2".into(),
        Protocol::Test,
        None,
        200.0,
        200.0,
        100,
        100,
        1,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 400.0,
    };
    engine.recenter_view(&panel);
    let (scale, _ox, _oy) = engine.display_transform(&panel);
    let bbox_w = 350.0;
    let bbox_h = 350.0;
    let expected_scale = (panel.w / bbox_w).min(panel.h / bbox_h) * 0.9;
    assert!((scale - expected_scale).abs() < 1e-3);
}

#[test]
fn resize_source_handles_corners_and_edges() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(

        "L1".into(),
        Protocol::Test,
        None,
        100.0,
        100.0,
        200,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();
    let start = engine.source_rect_world(&uuid).unwrap();

    // Edge resize: drag right edge 30 px to the right.
    engine.resize_source(&uuid, ResizeHandle::Right, start, (30.0, 0.0), &panel);
    let source = &engine.cfg.canvas.sources[0];
    assert_eq!(source.x, 100.0);
    assert_eq!(source.y, 100.0);
    assert_eq!(source.width, 230);
    assert_eq!(source.height, 100);

    // Edge resize: drag top edge 20 px up.
    let start = engine.source_rect_world(&uuid).unwrap();
    engine.resize_source(&uuid, ResizeHandle::Top, start, (0.0, -20.0), &panel);
    let source = &engine.cfg.canvas.sources[0];
    assert_eq!(source.x, 100.0);
    assert_eq!(source.y, 80.0);
    assert_eq!(source.width, 230);
    assert_eq!(source.height, 120);

    // Corner resize: drag bottom-right along the diagonal.
    let start = engine.source_rect_world(&uuid).unwrap();
    engine.resize_source(
        &uuid,
        ResizeHandle::BottomRight,
        start,
        (50.0, 50.0 * 120.0 / 230.0),
        &panel,
    );
    let source = &engine.cfg.canvas.sources[0];
    let aspect = source.width as f32 / source.height as f32;
    // Integer dimensions can't match the exact float aspect; allow ~1% rounding error.
    assert!(
        (aspect - 230.0 / 120.0).abs() < 0.02,
        "aspect should be preserved, got {aspect}"
    );
    assert_eq!(source.x, 100.0);
    assert_eq!(source.y, 80.0);
}

#[test]
fn drag_source_snaps_to_canvas_edge() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(

        "L1".into(),
        Protocol::Test,
        None,
        15.0,
        100.0,
        100,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();
    // Drag left by 14 px: left edge moves from 15 to 1, within the 2 px snap threshold of 0.
    engine.drag_source(&uuid, (-14.0, 0.0), &panel);
    let source = &engine.cfg.canvas.sources[0];
    assert_eq!(source.x, 0.0);
    assert!(engine.snap_guides.x.is_some());
}

#[test]
fn resize_source_snaps_to_other_source_edge() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(

        "L1".into(),
        Protocol::Test,
        None,
        100.0,
        100.0,
        100,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    canvas.sources.push(Source::new(
        "L2".into(),
        Protocol::Test,
        None,
        300.0,
        100.0,
        100,
        100,
        1,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();
    let start = engine.source_rect_world(&uuid).unwrap();
    // Drag L1's right edge to 299: should snap to L2's left edge at 300.
    engine.resize_source(&uuid, ResizeHandle::Right, start, (99.0, 0.0), &panel);
    let source = &engine.cfg.canvas.sources[0];
    assert_eq!(source.width, 200);
    assert!(engine.snap_guides.x.is_some());
}

#[test]
fn drag_source_hysteresis_releases_after_break_threshold() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(

        "L1".into(),
        Protocol::Test,
        None,
        15.0,
        100.0,
        100,
        100,
        0,
        TextureMode::Fit,
        false,
        false,
    ));
    let mut engine = test_engine(canvas);
    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();

    // Snap left edge to the canvas edge at 0.
    engine.drag_source(&uuid, (-14.0, 0.0), &panel);
    assert_eq!(engine.cfg.canvas.sources[0].x, 0.0);
    assert!(engine.snap_guides.x.is_some());

    // Move 1 px back: stays snapped within the 20 px break threshold.
    engine.drag_source(&uuid, (1.0, 0.0), &panel);
    assert_eq!(engine.cfg.canvas.sources[0].x, 0.0);
    assert!(engine.snap_guides.x.is_some());

    // Move 25 px past the snap point: breaks free.
    engine.drag_source(&uuid, (25.0, 0.0), &panel);
    assert_eq!(engine.cfg.canvas.sources[0].x, 25.0);
    assert!(engine.snap_guides.x.is_none());
}

#[test]
fn select_source_cycles_forward_and_wraps() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(
        "L1".into(), Protocol::Test, None,
        0.0, 0.0, 100, 100, 0,
        TextureMode::Fit, false, false,
    ));
    canvas.sources.push(Source::new(
        "L2".into(), Protocol::Test, None,
        100.0, 0.0, 100, 100, 1,
        TextureMode::Fit, false, false,
    ));
    let mut engine = test_engine(canvas);
    let uuids: Vec<String> = engine.cfg.canvas.sources.iter().map(|s| s.uuid.clone()).collect();

    engine.select_source(1);
    assert_eq!(engine.selected_source_id.as_ref(), Some(&uuids[0]));

    engine.selected_source_id = Some(uuids[0].clone());
    engine.select_source(1);
    assert_eq!(engine.selected_source_id.as_ref(), Some(&uuids[1]));

    engine.select_source(1);
    assert_eq!(engine.selected_source_id.as_ref(), Some(&uuids[0]));
}

#[test]
fn select_source_cycles_backward_and_wraps() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(
        "L1".into(), Protocol::Test, None,
        0.0, 0.0, 100, 100, 0,
        TextureMode::Fit, false, false,
    ));
    canvas.sources.push(Source::new(
        "L2".into(), Protocol::Test, None,
        100.0, 0.0, 100, 100, 1,
        TextureMode::Fit, false, false,
    ));
    let mut engine = test_engine(canvas);
    let uuids: Vec<String> = engine.cfg.canvas.sources.iter().map(|s| s.uuid.clone()).collect();

    engine.selected_source_id = Some(uuids[0].clone());
    engine.select_source(-1);
    assert_eq!(engine.selected_source_id.as_ref(), Some(&uuids[1]));

    engine.select_source(-1);
    assert_eq!(engine.selected_source_id.as_ref(), Some(&uuids[0]));
}

#[test]
fn nudge_selected_source_moves_by_delta() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(
        "L1".into(), Protocol::Test, None,
        10.0, 20.0, 100, 100, 0,
        TextureMode::Fit, false, false,
    ));
    let mut engine = test_engine(canvas);
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();

    engine.selected_source_id = Some(uuid.clone());
    engine.nudge_selected_source(3.0, -5.0);
    let source = &engine.cfg.canvas.sources[0];
    assert_eq!(source.x, 13.0);
    assert_eq!(source.y, 15.0);
    assert!(engine.dirty);
}

#[test]
fn nudge_selected_source_ignores_when_expanded() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    canvas.sources.push(Source::new(
        "L1".into(), Protocol::Test, None,
        10.0, 20.0, 100, 100, 0,
        TextureMode::Fit, false, false,
    ));
    let mut engine = test_engine(canvas);
    let uuid = engine.cfg.canvas.sources[0].uuid.clone();

    engine.selected_source_id = Some(uuid.clone());
    engine.expand_source(uuid.clone());
    engine.nudge_selected_source(3.0, -5.0);
    let source = &engine.cfg.canvas.sources[0];
    assert_eq!(source.x, 10.0);
    assert_eq!(source.y, 20.0);
}

fn should_auto_save(engine: &Engine, now: Instant) -> bool {
    engine.dirty
        && engine.project_path.is_some()
        && now.duration_since(engine.last_saved_at) >= AUTO_SAVE_INTERVAL
}

#[test]
fn auto_save_triggers_after_interval_when_dirty() {
    let mut engine = test_engine(Canvas::default());
    engine.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
    engine.dirty = true;
    engine.last_saved_at = Instant::now() - AUTO_SAVE_INTERVAL - Duration::from_secs(1);
    assert!(should_auto_save(&engine, Instant::now()));
}

#[test]
fn auto_save_does_not_trigger_when_clean() {
    let mut engine = test_engine(Canvas::default());
    engine.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
    engine.dirty = false;
    engine.last_saved_at = Instant::now() - AUTO_SAVE_INTERVAL - Duration::from_secs(1);
    assert!(!should_auto_save(&engine, Instant::now()));
}

#[test]
fn auto_save_does_not_trigger_without_project_path() {
    let mut engine = test_engine(Canvas::default());
    engine.project_path = None;
    engine.dirty = true;
    engine.last_saved_at = Instant::now() - AUTO_SAVE_INTERVAL - Duration::from_secs(1);
    assert!(!should_auto_save(&engine, Instant::now()));
}

/// A cfg output entry without its runtime object (project saved before the
/// protocol was wired up) must be recreated on enable, or the side-panel
/// checkbox silently flips back off while cfg says enabled.
#[test]
fn enabling_output_with_missing_runtime_recreates_it() {
    let mut engine = test_engine(Canvas::default());
    let output = Output::new(
        "NDI".to_string(),
        Protocol::Ndi,
        false,
        OutputConfig::Ndi(NdiOutputConfig {
            sender_name: "Test".to_string(),
        }),
    );
    let id = output.uuid.clone();
    engine.cfg.canvas.outputs.push(output);

    engine.set_output_enabled(&id, true);

    assert!(engine.cfg.canvas.outputs.iter().any(|o| o.uuid == id && o.enabled));
    let registered = engine.output_registry.get(&id).expect("runtime object recreated");
    assert!(registered.enabled());
}

/// An edit on one placement persists to every placement bound to the same
/// (protocol, source_ref) — and to no placement bound to a different source or
/// protocol, even when the reference string is identical.
#[test]
fn sync_source_config_updates_quads_sharing_the_key() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    let ndi = |bw| SourceConfig::Ndi(NdiSourceConfig {
        bandwidth: bw,
        color_format: NdiReceiverColorFormat::UYVY_RGBA,
    });
    let highest = NdiReceiverBandwidth::Highest;
    let lowest = NdiReceiverBandwidth::Lowest;

    let mut quad_a = Source::new(
        "Quad A".into(), Protocol::Ndi, Some("Cam (1)".into()),
        0.0, 0.0, 960, 540, 0, TextureMode::Fit, false, false,
    );
    quad_a.config = ndi(highest);
    let mut quad_b = Source::new(
        "Quad B".into(), Protocol::Ndi, Some("Cam (1)".into()),
        960.0, 0.0, 960, 540, 1, TextureMode::Fit, false, false,
    );
    quad_b.config = ndi(highest);
    let mut quad_c = Source::new(
        "Quad C".into(), Protocol::Ndi, Some("Other Cam".into()),
        0.0, 540.0, 960, 540, 2, TextureMode::Fit, false, false,
    );
    quad_c.config = ndi(highest);
    let mut quad_d = Source::new(
        "Quad D".into(), Protocol::Decklink, Some("Cam (1)".into()),
        960.0, 540.0, 960, 540, 3, TextureMode::Fit, false, false,
    );
    quad_d.config = SourceConfig::Decklink(DecklinkSourceConfig {
        connection: DecklinkVideoConnection::Hdmi,
    });
    canvas.sources.extend([quad_a, quad_b, quad_c, quad_d]);
    let mut engine = test_engine(canvas);
    let uuid_a = engine.cfg.canvas.sources[0].uuid.clone();

    engine.sync_source_config(&uuid_a, ndi(lowest));

    let [a, b, c, d] = &engine.cfg.canvas.sources[..] else {
        panic!("expected four sources")
    };
    // Same (protocol, source_ref): both quads show the edit.
    let (SourceConfig::Ndi(ca), SourceConfig::Ndi(cb)) = (&a.config, &b.config) else {
        panic!("expected Ndi configs")
    };
    assert_eq!(ca.bandwidth, lowest);
    assert_eq!(cb.bandwidth, lowest);
    // Same protocol, different source_ref: untouched.
    let SourceConfig::Ndi(cc) = &c.config else {
        panic!("expected Ndi config")
    };
    assert_eq!(cc.bandwidth, highest);
    // Same reference string under another protocol is a different key.
    let SourceConfig::Decklink(cd) = &d.config else {
        panic!("expected Decklink config")
    };
    assert_eq!(cd.connection, DecklinkVideoConnection::Hdmi);
    assert!(engine.dirty);
}

#[test]
fn newer_project_version_warns_and_clamps() {
    let mut engine = test_engine(Canvas::default());
    engine.cfg.version = CONFIG_VERSION + 1;

    engine.warn_if_newer_version();

    assert_eq!(engine.cfg.version, CONFIG_VERSION);
    assert_eq!(engine.load_warnings.len(), 1);
    assert!(engine.load_warnings[0].contains("newer version"));
}

#[test]
fn current_project_version_does_not_warn() {
    let mut engine = test_engine(Canvas::default());
    engine.cfg.version = CONFIG_VERSION;

    engine.warn_if_newer_version();

    assert!(engine.load_warnings.is_empty());
    assert_eq!(engine.cfg.version, CONFIG_VERSION);
}
