use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Grid {
    pub rows: u32,
    pub cols: u32,
    /// row-major, len == rows*cols
    pub cells: Vec<Option<String>>,
}

impl Grid {
    pub fn new(rows: u32, cols: u32) -> Self {
        let n = (rows * cols) as usize;
        Self { rows, cols, cells: vec![None; n] }
    }

    /// Resize, keeping overlapping top-left region assignments.
    pub fn resize(&mut self, rows: u32, cols: u32) {
        let mut cells = vec![None; (rows * cols) as usize];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                cells[(r * cols + c) as usize] = self.cells[(r * self.cols + c) as usize].take();
            }
        }
        self.rows = rows;
        self.cols = cols;
        self.cells = cells;
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::new(4, 4)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub grid: Grid,
}

pub fn path() -> PathBuf {
    // ponytail: exe-dir beats CWD for packaged apps; dev builds get target/{profile}/multiviewer.json
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("multiviewer.json")
}

impl Config {
    pub fn load() -> Self {
        match std::fs::read_to_string(path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("bad config, using defaults: {e}");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(path(), s) {
                    tracing::error!("config save failed: {e}");
                }
            }
            Err(e) => tracing::error!("config serialize failed: {e}"),
        }
    }
}
