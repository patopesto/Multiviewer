use std::sync::Arc;

use crate::sources::Protocol;

use super::state::EngineState;
use super::view::RuntimeView;
use super::Engine;

/// A point-in-time clone of the document and runtime data, published by the
/// runner for clients that must not touch `Engine`.
#[derive(Clone)]
pub struct EngineSnapshot {
    pub state: EngineState,
    pub runtime: Arc<RuntimeView>,
}

impl Engine {
    #[cfg(test)]
    pub fn snapshot(&self) -> EngineSnapshot {
        return self.snapshot_focused(None);
    }

    pub fn snapshot_focused(&self, focus: Option<&Protocol>) -> EngineSnapshot {
        return EngineSnapshot {
            state: self.state.clone(),
            runtime: Arc::new(self.runtime_view(focus)),
        };
    }
}
