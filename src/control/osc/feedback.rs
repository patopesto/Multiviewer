use rosc::OscType;

use crate::APP_NAME;
use crate::engine::Engine;

/// One OSC feedback value. Addresses always carry a single argument so a
/// controller can map a path directly to a button state or variable.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedbackValue {
    Int(i32),
    Float(f32),
    String(String),
}

impl FeedbackValue {
    /// Value sent when an address disappears, so stale feedback clears.
    pub fn reset(&self) -> Self {
        return match self {
            FeedbackValue::Int(_) => FeedbackValue::Int(0),
            FeedbackValue::Float(_) => FeedbackValue::Float(0.0),
            FeedbackValue::String(_) => FeedbackValue::String(String::new()),
        };
    }

    pub fn to_osc(&self) -> OscType {
        return match self {
            FeedbackValue::Int(value) => OscType::Int(*value),
            FeedbackValue::Float(value) => OscType::Float(*value),
            FeedbackValue::String(value) => OscType::String(value.clone()),
        };
    }
}

/// Current feedback values, sorted by address. Read straight from the engine on
/// the UI thread; nothing is stored outside the publishing adapter.
pub fn values(engine: &Engine, prefix: &str) -> Vec<(String, FeedbackValue)> {
    let canvas = &engine.cfg.canvas;
    let mut values = Vec::new();
    let mut push = |path: &str, value: FeedbackValue| {
        values.push((format!("{prefix}{path}"), value));
    };

    // @osc path="/app" kind="feedback" args="value:string" group="Global" desc="Application name."
    push(
        "/app",
        FeedbackValue::String(APP_NAME.to_string())
    );
    // @osc path="/version" kind="feedback" args="value:string" group="Global" desc="Application version."
    push(
        "/version",
        FeedbackValue::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    // @osc path="/canvas/width" kind="feedback" args="value:int" group="Global" desc="Canvas width in pixels."
    push(
        "/canvas/width",
        FeedbackValue::Int(canvas.width as i32),
    );
    // @osc path="/canvas/height" kind="feedback" args="value:int" group="Global" desc="Canvas height in pixels."
    push(
        "/canvas/height",
        FeedbackValue::Int(canvas.height as i32),
    );
    // @osc path="/ui/visible" kind="feedback" args="value:0|1" group="Global" desc="Whether the editor UI is visible."
    push(
        "/ui/visible",
        FeedbackValue::Int(engine.ui_visible as i32),
    );
    // @osc path="/view/zoom" kind="feedback" args="value:float" group="Global" desc="Current zoom level."
    push("/view/zoom", FeedbackValue::Float(engine.view.zoom));
    // @osc path="/view/pan_x" kind="feedback" args="value:float" group="Global" desc="Horizontal pan offset."
    push(
        "/view/pan_x",
        FeedbackValue::Float(engine.view.pan.x),
    );
    // @osc path="/view/pan_y" kind="feedback" args="value:float" group="Global" desc="Vertical pan offset."
    push(
        "/view/pan_y",
        FeedbackValue::Float(engine.view.pan.y),
    );
    // @osc path="/project/auto_save_interval" kind="feedback" args="value:int" group="Global" desc="Seconds between auto-saves; 0 when auto-save is off."
    push(
        "/project/auto_save_interval",
        FeedbackValue::Int(engine.cfg.auto_save_interval as i32),
    );

    let selected_index = engine
        .selected_source_id
        .as_ref()
        .and_then(|uuid| canvas.sources.iter().position(|source| &source.uuid == uuid))
        .map(|index| index as i32 + 1)
        .unwrap_or(0);
    // @osc path="/selected/index" kind="feedback" args="value:int" group="Global" desc="Selected source index, 0 when nothing is selected."
    push("/selected/index", FeedbackValue::Int(selected_index));
    // @osc path="/selected/uuid" kind="feedback" args="value:string" group="Global" desc="Selected source uuid, empty when nothing is selected."
    push(
        "/selected/uuid",
        FeedbackValue::String(engine.selected_source_id.clone().unwrap_or_default()),
    );
    // @osc path="/source/count" kind="feedback" args="value:int" group="Global" desc="Number of sources."
    push(
        "/source/count",
        FeedbackValue::Int(canvas.sources.len() as i32),
    );
    // @osc path="/output/count" kind="feedback" args="value:int" group="Global" desc="Number of outputs."
    push(
        "/output/count",
        FeedbackValue::Int(canvas.outputs.len() as i32),
    );

    for (index, source) in canvas.sources.iter().enumerate() {
        let i = index + 1;
        let selected = engine.selected_source_id.as_deref() == Some(source.uuid.as_str());
        let expanded = engine.expanded_source_id.as_deref() == Some(source.uuid.as_str());

        // @osc path="/source/{i}/exists" kind="feedback" args="value:0|1" group="Sources" desc="Whether a source exists at this index."
        push(&format!("/source/{i}/exists"), FeedbackValue::Int(1));
        // @osc path="/source/{i}/name" kind="feedback" args="value:string" group="Sources" desc="Source name."
        push(
            &format!("/source/{i}/name"),
            FeedbackValue::String(source.name.clone()),
        );
        // @osc path="/source/{i}/uuid" kind="feedback" args="value:string" group="Sources" desc="Source uuid."
        push(
            &format!("/source/{i}/uuid"),
            FeedbackValue::String(source.uuid.clone()),
        );
        // @osc path="/source/{i}/protocol" kind="feedback" args="value:string" group="Sources" desc="Source protocol."
        push(
            &format!("/source/{i}/protocol"),
            FeedbackValue::String(source.protocol.name().to_string()),
        );
        // @osc path="/source/{i}/connected" kind="feedback" args="value:0|1" group="Sources" desc="Whether the runtime source is live."
        push(
            &format!("/source/{i}/connected"),
            FeedbackValue::Int(engine.is_source_connected(source) as i32),
        );
        // @osc path="/source/{i}/selected" kind="feedback" args="value:0|1" group="Sources" desc="Whether this source is selected."
        push(
            &format!("/source/{i}/selected"),
            FeedbackValue::Int(selected as i32),
        );
        // @osc path="/source/{i}/expanded" kind="feedback" args="value:0|1" group="Sources" desc="Whether this source is shown full-canvas."
        push(
            &format!("/source/{i}/expanded"),
            FeedbackValue::Int(expanded as i32),
        );
        // @osc path="/source/{i}/x" kind="feedback" args="value:float" group="Sources" desc="Horizontal position in canvas pixels."
        push(
            &format!("/source/{i}/x"),
            FeedbackValue::Float(source.x),
        );
        // @osc path="/source/{i}/y" kind="feedback" args="value:float" group="Sources" desc="Vertical position in canvas pixels."
        push(
            &format!("/source/{i}/y"),
            FeedbackValue::Float(source.y),
        );
        // @osc path="/source/{i}/width" kind="feedback" args="value:int" group="Sources" desc="Placement width in pixels."
        push(
            &format!("/source/{i}/width"),
            FeedbackValue::Int(source.width as i32),
        );
        // @osc path="/source/{i}/height" kind="feedback" args="value:int" group="Sources" desc="Placement height in pixels."
        push(
            &format!("/source/{i}/height"),
            FeedbackValue::Int(source.height as i32),
        );
        // @osc path="/source/{i}/z" kind="feedback" args="value:int" group="Sources" desc="Stacking order."
        push(
            &format!("/source/{i}/z"),
            FeedbackValue::Int(source.z),
        );
        // @osc path="/source/{i}/mode" kind="feedback" args="value:string" group="Sources" desc="Placement mode."
        push(
            &format!("/source/{i}/mode"),
            FeedbackValue::String(source.mode.label().to_string()),
        );
        // @osc path="/source/{i}/flip_h" kind="feedback" args="value:0|1" group="Sources" desc="Whether the source is mirrored horizontally."
        push(
            &format!("/source/{i}/flip_h"),
            FeedbackValue::Int(source.flip_h as i32),
        );
        // @osc path="/source/{i}/flip_v" kind="feedback" args="value:0|1" group="Sources" desc="Whether the source is mirrored vertically."
        push(
            &format!("/source/{i}/flip_v"),
            FeedbackValue::Int(source.flip_v as i32),
        );
    }

    for (index, output) in canvas.outputs.iter().enumerate() {
        let i = index + 1;
        // @osc path="/output/{i}/exists" kind="feedback" args="value:0|1" group="Outputs" desc="Whether an output exists at this index."
        push(&format!("/output/{i}/exists"), FeedbackValue::Int(1));
        // @osc path="/output/{i}/name" kind="feedback" args="value:string" group="Outputs" desc="Output name."
        push(
            &format!("/output/{i}/name"),
            FeedbackValue::String(output.name.clone()),
        );
        // @osc path="/output/{i}/uuid" kind="feedback" args="value:string" group="Outputs" desc="Output uuid."
        push(
            &format!("/output/{i}/uuid"),
            FeedbackValue::String(output.uuid.clone()),
        );
        // @osc path="/output/{i}/protocol" kind="feedback" args="value:string" group="Outputs" desc="Output protocol."
        push(
            &format!("/output/{i}/protocol"),
            FeedbackValue::String(output.protocol.label()),
        );
        // @osc path="/output/{i}/enabled" kind="feedback" args="value:0|1" group="Outputs" desc="Whether the output is enabled."
        push(
            &format!("/output/{i}/enabled"),
            FeedbackValue::Int(output.enabled as i32),
        );
    }

    drop(push);
    values.sort_by(|a, b| a.0.cmp(&b.0));
    return values;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Canvas, Output};
    use crate::sources::{OutputConfig, Protocol};

    #[test]
    fn describes_sources_and_outputs() {
        let mut engine = Engine::test_instance(Canvas::default());
        let uuid = engine.add_source();
        engine.selected_source_id = Some(uuid.clone());
        engine.expanded_source_id = Some(uuid);
        engine.cfg.auto_save_interval = 300;
        engine.cfg.canvas.outputs.push(Output::new(
            "Program".to_string(),
            Protocol::Ndi,
            true,
            OutputConfig::default(),
        ));

        let values = values(&engine, "");
        let get = |path: &str| {
            values
                .iter()
                .find(|(address, _)| address == path)
                .map(|(_, value)| value.clone())
        };

        assert_eq!(get("/source/count"), Some(FeedbackValue::Int(1)));
        assert_eq!(
            get("/source/1/name"),
            Some(FeedbackValue::String("Source 1".to_string()))
        );
        assert_eq!(get("/source/1/selected"), Some(FeedbackValue::Int(1)));
        assert_eq!(get("/source/1/expanded"), Some(FeedbackValue::Int(1)));
        assert_eq!(get("/source/1/exists"), Some(FeedbackValue::Int(1)));
        assert_eq!(get("/output/count"), Some(FeedbackValue::Int(1)));
        assert_eq!(
            get("/output/1/name"),
            Some(FeedbackValue::String("Program".to_string()))
        );
        assert_eq!(get("/output/1/enabled"), Some(FeedbackValue::Int(1)));
        assert_eq!(get("/selected/index"), Some(FeedbackValue::Int(1)));
        assert_eq!(
            get("/project/auto_save_interval"),
            Some(FeedbackValue::Int(300))
        );
    }

    #[test]
    fn applies_the_configured_prefix() {
        let engine = Engine::test_instance(Canvas::default());
        let values = values(&engine, "/my control");

        assert!(
            values
                .iter()
                .any(|(address, _)| address == "/my control/app")
        );
    }
}
