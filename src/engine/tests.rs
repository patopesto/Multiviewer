use super::*;
use crate::compositor::{self, Rect};
use crate::config::{Canvas, Config, Output, Source, TextureMode, CONFIG_VERSION};
use crate::control::{Command, Selector};
use crate::sources::{Protocol, SourceConfig, SourceKey, OutputConfig, DecklinkSourceConfig, NdiOutputConfig, NdiSourceConfig, DecklinkVideoConnection, NdiReceiverBandwidth, NdiReceiverColorFormat};
use crate::sources::VncSourceConfig;
use std::time::{Duration, Instant};

fn test_engine(canvas: Canvas) -> Engine {
    return Engine::test_instance(canvas);
}

fn display_transform(engine: &Engine, panel: &Rect) -> (f32, f32, f32) {
    return super::transform::display_transform(
        &engine.state.view,
        &engine.state.cfg.canvas,
        panel,
        engine.state.ui_visible,
    );
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
    assert_eq!(engine.state.cfg.canvas.sources.len(), 2);
    assert_eq!(engine.state.cfg.canvas.outputs.len(), 1);
    let SourceConfig::Unknown(v) = &engine.state.cfg.canvas.sources[0].config else {
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
        engine.state.cfg.canvas.sources[1].source_ref.clone().unwrap(),
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
    let uuid = engine.state.cfg.canvas.sources[0].uuid.clone();

    assert!(engine.state.expanded_source_id.as_deref().is_none());
    engine.expand_source(uuid.clone());
    assert_eq!(engine.state.expanded_source_id.as_deref(), Some(uuid.as_str()));
    engine.clear_expanded_source();
    assert!(engine.state.expanded_source_id.as_deref().is_none());
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
    let uuid = engine.state.cfg.canvas.sources[0].uuid.clone();
    engine.expand_source(uuid.clone());

    engine.remove_source(&uuid);
    assert!(engine.state.expanded_source_id.as_deref().is_none());
    assert!(engine.active_source_keys().is_empty());
}

#[test]
fn active_source_keys_dedupes_shared_refs_and_skips_unbound() {
    let mut canvas = Canvas {
        width: 1920,
        height: 1080,
        sources: vec![],
        outputs: Vec::new(),
        ..Default::default()
    };
    for (name, source_ref) in [("A", Some("Test A")), ("B", Some("Test A")), ("C", None)] {
        canvas.sources.push(Source::new(
            name.into(),
            Protocol::Test,
            source_ref.map(str::to_string),
            100.0,
            100.0,
            100,
            100,
            0,
            TextureMode::Fit,
            false,
            false,
        ));
    }
    let engine = test_engine(canvas);
    assert_eq!(
        engine.active_source_keys(),
        vec![SourceKey::new(Protocol::Test, "Test A".into())]
    );
}

#[test]
fn rebuilding_the_project_bumps_the_document_revision() {
    let mut engine = test_engine(Canvas::default());
    let before = engine.state.revision;

    engine.rebuild_from_config();

    assert!(engine.state.revision > before);
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
    let (scale, ox, oy) = display_transform(&engine, &panel);
    let (base_scale, base_ox, base_oy) =
        compositor::canvas_transform(&engine.state.cfg.canvas, &panel);
    assert!((scale - base_scale).abs() < 1e-3);
    assert!((ox - base_ox).abs() < 1e-3);
    assert!((oy - base_oy).abs() < 1e-3);
}

#[test]
fn display_transform_applies_view_when_ui_is_visible() {
    let mut engine = test_engine(Canvas::default());
    engine.state.view.zoom = 2.0;
    engine.state.view.pan = [10.0, 20.0];

    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    };
    let (scale, ox, oy) = display_transform(&engine, &panel);
    let (base_scale, base_ox, base_oy) =
        compositor::canvas_transform(&engine.state.cfg.canvas, &panel);
    assert!((scale - base_scale * 2.0).abs() < 1e-3);
    assert!((ox - (base_ox + 10.0)).abs() < 1e-3);
    assert!((oy - (base_oy + 20.0)).abs() < 1e-3);
}

#[test]
fn display_transform_ignores_view_when_ui_is_hidden() {
    let mut engine = test_engine(Canvas::default());
    engine.state.view.zoom = 2.0;
    engine.state.view.pan = [10.0, 20.0];
    engine.state.ui_visible = false;

    let panel = Rect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    };
    let (scale, ox, oy) = display_transform(&engine, &panel);
    let (base_scale, base_ox, base_oy) =
        compositor::canvas_transform(&engine.state.cfg.canvas, &panel);
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
    engine.state.view.recenter(&engine.state.cfg.canvas, &panel);
    let (scale, ox, oy) = display_transform(&engine, &panel);
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
    engine.state.view.recenter(&engine.state.cfg.canvas, &panel);
    let (scale, _ox, _oy) = display_transform(&engine, &panel);
    let bbox_w = 350.0;
    let bbox_h = 350.0;
    let expected_scale = (panel.w / bbox_w).min(panel.h / bbox_h) * 0.9;
    assert!((scale - expected_scale).abs() < 1e-3);
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
    let uuids: Vec<String> = engine.state.cfg.canvas.sources.iter().map(|s| s.uuid.clone()).collect();

    engine.select_source(1);
    assert_eq!(engine.state.selected_source_id.as_ref(), Some(&uuids[0]));

    engine.state.selected_source_id = Some(uuids[0].clone());
    engine.select_source(1);
    assert_eq!(engine.state.selected_source_id.as_ref(), Some(&uuids[1]));

    engine.select_source(1);
    assert_eq!(engine.state.selected_source_id.as_ref(), Some(&uuids[0]));
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
    let uuids: Vec<String> = engine.state.cfg.canvas.sources.iter().map(|s| s.uuid.clone()).collect();

    engine.state.selected_source_id = Some(uuids[0].clone());
    engine.select_source(-1);
    assert_eq!(engine.state.selected_source_id.as_ref(), Some(&uuids[1]));

    engine.select_source(-1);
    assert_eq!(engine.state.selected_source_id.as_ref(), Some(&uuids[0]));
}

#[test]
fn nudge_command_moves_by_delta() {
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
    let uuid = engine.state.cfg.canvas.sources[0].uuid.clone();

    engine.state.selected_source_id = Some(uuid.clone());
    engine.apply_command(Command::Nudge { dx: 3.0, dy: -5.0 }).unwrap();
    let source = &engine.state.cfg.canvas.sources[0];
    assert_eq!(source.x, 13.0);
    assert_eq!(source.y, 15.0);
    assert!(engine.state.dirty);
}

#[test]
fn nudge_command_ignores_when_expanded() {
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
    let uuid = engine.state.cfg.canvas.sources[0].uuid.clone();

    engine.state.selected_source_id = Some(uuid.clone());
    engine.expand_source(uuid.clone());
    engine.apply_command(Command::Nudge { dx: 3.0, dy: -5.0 }).unwrap();
    let source = &engine.state.cfg.canvas.sources[0];
    assert_eq!(source.x, 10.0);
    assert_eq!(source.y, 20.0);
}

#[test]
fn auto_save_triggers_after_interval_when_dirty() {
    let mut engine = test_engine(Canvas::default());
    engine.state.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
    engine.state.dirty = true;
    let interval = Duration::from_secs(engine.state.cfg.auto_save_interval);
    engine.last_saved_at = Instant::now() - interval - Duration::from_secs(1);
    assert!(engine.should_auto_save(Instant::now()));
}

#[test]
fn auto_save_does_not_trigger_when_clean() {
    let mut engine = test_engine(Canvas::default());
    engine.state.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
    engine.state.dirty = false;
    let interval = Duration::from_secs(engine.state.cfg.auto_save_interval);
    engine.last_saved_at = Instant::now() - interval - Duration::from_secs(1);
    assert!(!engine.should_auto_save(Instant::now()));
}

#[test]
fn auto_save_does_not_trigger_without_project_path() {
    let mut engine = test_engine(Canvas::default());
    engine.state.project_path = None;
    engine.state.dirty = true;
    let interval = Duration::from_secs(engine.state.cfg.auto_save_interval);
    engine.last_saved_at = Instant::now() - interval - Duration::from_secs(1);
    assert!(!engine.should_auto_save(Instant::now()));
}

#[test]
fn auto_save_does_not_trigger_when_disabled() {
    let mut engine = test_engine(Canvas::default());
    engine.state.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
    engine.state.dirty = true;
    engine.state.cfg.auto_save_interval = 0;
    engine.last_saved_at = Instant::now() - Duration::from_secs(3600);
    assert!(!engine.should_auto_save(Instant::now()));
}

#[test]
fn auto_save_uses_the_configured_interval() {
    let mut engine = test_engine(Canvas::default());
    engine.state.project_path = Some(std::path::PathBuf::from("/tmp/test.multiviewer"));
    engine.state.dirty = true;
    engine.state.cfg.auto_save_interval = 300;
    engine.last_saved_at = Instant::now() - Duration::from_secs(60);
    assert!(!engine.should_auto_save(Instant::now()));
    engine.last_saved_at = Instant::now() - Duration::from_secs(301);
    assert!(engine.should_auto_save(Instant::now()));
}

#[test]
fn control_sets_auto_save_interval() {
    let mut engine = test_engine(Canvas::default());
    engine.apply_command(Command::SetAutoSaveInterval(300)).unwrap();
    assert_eq!(engine.state.cfg.auto_save_interval, 300);
    assert!(engine.state.dirty);
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
    engine.state.cfg.canvas.outputs.push(output);

    engine.set_output_enabled(&id, true);

    assert!(engine.state.cfg.canvas.outputs.iter().any(|o| o.uuid == id && o.enabled));
    let registered = engine.output_registry.get(&id).expect("runtime object recreated");
    assert!(registered.enabled());
}

/// A placement whose source has not been configured yet still gets a runtime, so
/// its Protocol Settings stay reachable and the host can be filled in. Without
/// one the side panel hides every field on that source, which is a dead end.
#[test]
fn an_unconfigured_manual_source_is_registered() {
    let mut placement = Source::new(
        "Remote".into(),
        Protocol::Vnc,
        Some("VNC 1".into()),
        0.0, 0.0, 640, 360, 0, TextureMode::Fit, false, false,
    );
    placement.config = SourceConfig::Vnc(VncSourceConfig::default());
    let mut engine = test_engine(Canvas {
        sources: vec![placement],
        ..Default::default()
    });

    // The discovery tick is what registers a placement's runtime.
    engine.last_discovery = Instant::now() - Duration::from_secs(1);
    engine.maintain();

    let key = SourceKey::new(Protocol::Vnc, "VNC 1".to_string());
    assert!(engine.registry.get(&key).is_some());
    // Auto-connect only builds a runtime source; the project itself is unchanged.
    assert!(!engine.state.dirty);
}

/// The heartbeat keeps the worst tick gap until it is reported, so a tick that
/// stalled (inline teardown, blocked present) is not averaged away by the
/// ticks that follow it.
#[test]
fn heartbeat_keeps_the_worst_tick_gap_until_it_reports() {
    let mut engine = test_engine(Canvas::default());

    engine.last_tick = Instant::now() - Duration::from_secs(3);
    engine.last_heartbeat = Instant::now();
    engine.maintain();
    assert!(engine.worst_tick_gap >= Duration::from_secs(3));

    engine.last_heartbeat = Instant::now() - Duration::from_secs(60);
    engine.maintain();
    assert_eq!(engine.worst_tick_gap, Duration::ZERO);
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
    let uuid_a = engine.state.cfg.canvas.sources[0].uuid.clone();

    engine.sync_source_config(&uuid_a, ndi(lowest));

    let [a, b, c, d] = &engine.state.cfg.canvas.sources[..] else {
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
    assert!(engine.state.dirty);
}

#[test]
fn newer_project_version_warns_and_clamps() {
    let mut engine = test_engine(Canvas::default());
    engine.state.cfg.version = CONFIG_VERSION + 1;

    engine.warn_if_newer_version();

    assert_eq!(engine.state.cfg.version, CONFIG_VERSION);
    assert_eq!(engine.load_warnings.len(), 1);
    assert!(engine.load_warnings[0].contains("newer version"));
}

#[test]
fn current_project_version_does_not_warn() {
    let mut engine = test_engine(Canvas::default());
    engine.state.cfg.version = CONFIG_VERSION;

    engine.warn_if_newer_version();

    assert!(engine.load_warnings.is_empty());
    assert_eq!(engine.state.cfg.version, CONFIG_VERSION);
}

#[test]
fn control_commands_mutate_the_canvas() {
    let mut engine = test_engine(Canvas::default());

    engine.apply_command(Command::AddSource).unwrap();
    let uuid = engine
        .state.selected_source_id
        .clone()
        .expect("added source is selected");

    engine
        .apply_command(Command::SetSourceX {
            source: Selector::Key(uuid),
            value: 123.0,
        })
        .unwrap();
    engine
        .apply_command(Command::SetSourceSize {
            source: Selector::Index(1),
            width: 321,
            height: 123,
        })
        .unwrap();

    let source = &engine.state.cfg.canvas.sources[0];
    assert_eq!(source.x, 123.0);
    assert_eq!(source.width, 321);
    assert_eq!(source.height, 123);
    assert!(engine.state.dirty);

    engine.apply_command(Command::ToggleUi).unwrap();
    assert!(!engine.state.ui_visible);
}

#[test]
fn control_rejects_unknown_targets_and_bad_values() {
    let mut engine = test_engine(Canvas::default());

    assert!(
        engine
            .apply_command(Command::RemoveSource(Selector::Index(99)))
            .is_err()
    );
    assert!(
        engine
            .apply_command(Command::SetSourceX {
                source: Selector::Index(1),
                value: f32::NAN,
            })
            .is_err()
    );
    assert!(
        engine
            .apply_command(Command::Zoom { factor: 0.0 })
            .is_err()
    );
}

#[test]
fn control_clear_selection_deselects() {
    let mut engine = test_engine(Canvas::default());
    engine.apply_command(Command::AddSource).unwrap();
    assert!(engine.state.selected_source_id.is_some());

    engine.apply_command(Command::ClearSelection).unwrap();

    assert!(engine.state.selected_source_id.is_none());
}

/// A view command that arrives before the first frame has laid out the canvas
/// must not divide by a zero-size viewport and poison the view with NaN.
#[test]
fn view_commands_ignore_an_uninitialised_viewport() {
    let mut engine = test_engine(Canvas::default());
    assert_eq!(engine.canvas_rect.width(), 0.0);

    engine.apply_command(Command::Zoom { factor: 1.1 }).unwrap();
    engine.apply_command(Command::RecenterView).unwrap();

    assert_eq!(engine.state.view.zoom, 1.0);
    assert!(engine.state.view.pan[0].is_finite());
    assert!(engine.state.view.pan[1].is_finite());
}
