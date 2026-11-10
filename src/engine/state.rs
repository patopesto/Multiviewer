use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{Config, SourceId};
use super::transform::ViewState;

/// The document state shared by the live `Engine` and its published snapshot.
#[derive(Clone, Serialize, Deserialize)]
pub struct EngineState {
    pub cfg: Config,
    pub selected_source_id: Option<SourceId>,
    pub expanded_source_id: Option<SourceId>,
    pub view: ViewState,
    pub dirty: bool,
    pub project_path: Option<PathBuf>,
    pub ui_visible: bool,
    pub revision: u64,
}
