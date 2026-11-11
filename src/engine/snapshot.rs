use std::sync::{Arc, Mutex};

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

/// Publish the current document state immediately, reusing the last runtime
/// view. The runner re-publishes on its own schedule; this covers writers that
/// do not go through its command channel (the HTTP API, host project actions).
pub(crate) fn publish_state(engine: &Engine, slot: &Mutex<Option<Arc<EngineSnapshot>>>) {
    let mut slot = slot.lock().unwrap();
    let runtime = slot
        .as_ref()
        .map(|snapshot| snapshot.runtime.clone())
        .unwrap_or_default();
    *slot = Some(Arc::new(EngineSnapshot {
        state: engine.state.clone(),
        runtime,
    }));
}

impl Engine {
    #[cfg(test)]
    pub fn snapshot(&self) -> EngineSnapshot {
        return self.snapshot_focused(&[None, None]);
    }

    pub fn snapshot_focused(&self, focuses: &[Option<Protocol>]) -> EngineSnapshot {
        return EngineSnapshot {
            state: self.state.clone(),
            runtime: Arc::new(self.runtime_view(focuses)),
        };
    }
}
