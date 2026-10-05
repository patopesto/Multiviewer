use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::APP_NAME;

const SESSION_FILENAME: &str = "session.json";

const RECENT_MAX: usize = 10;

/// Schema version of the session file. Bump on every breaking change.
pub const SESSION_VERSION: u32 = 1;

/// See `config::LEGACY_VERSION`.
const LEGACY_VERSION: u32 = 1;

fn session_version() -> u32 {
    return LEGACY_VERSION;
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Session {
    #[serde(default = "session_version")]
    pub version: u32,
    pub last_project: Option<PathBuf>,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            last_project: None,
            recent_projects: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NoAppDataDir,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "session IO error: {e}"),
            SessionError::Json(e) => write!(f, "session JSON error: {e}"),
            SessionError::NoAppDataDir => write!(f, "could not find app data directory"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        SessionError::Io(e)
    }
}

impl From<serde_json::Error> for SessionError {
    fn from(e: serde_json::Error) -> Self {
        SessionError::Json(e)
    }
}

impl Session {
    pub fn load() -> Self {
        let mut s = Self::load_from(app_data_dir()).unwrap_or_default();
        if s.version > SESSION_VERSION {
            tracing::warn!(
                "Session file was saved by a newer version ({} > {}); using it as-is",
                s.version,
                SESSION_VERSION
            );
            s.version = SESSION_VERSION;
        }
        s.prune_missing();
        return s;
    }

    fn load_from(dir: Result<PathBuf, SessionError>) -> Result<Self, SessionError> {
        let path = dir?.join(SESSION_FILENAME);
        let s = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn save(&self) -> Result<(), SessionError> {
        let dir = app_data_dir()?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(SESSION_FILENAME);
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(path, s)?;
        Ok(())
    }

    pub fn record_recent(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.recent_projects.retain(|p| *p != path);
        self.recent_projects.insert(0, path.clone());
        self.recent_projects.truncate(RECENT_MAX);
        self.last_project = Some(path);
    }

    fn prune_missing(&mut self) {
        self.recent_projects.retain(|p| p.exists());
    }
}

pub fn app_data_dir() -> Result<PathBuf, SessionError> {
    dirs::data_local_dir()
        .map(|d| d.join(APP_NAME))
        .ok_or(SessionError::NoAppDataDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_recent_moves_duplicate_to_front_and_caps() {
        let mut s = Session::default();
        s.record_recent("/tmp/a.multiviewer");
        s.record_recent("/tmp/b.multiviewer");
        s.record_recent("/tmp/a.multiviewer");
        assert_eq!(
            s.recent_projects,
            vec![
                PathBuf::from("/tmp/a.multiviewer"),
                PathBuf::from("/tmp/b.multiviewer")
            ]
        );
        assert_eq!(s.last_project, Some(PathBuf::from("/tmp/a.multiviewer")));

        for i in 0..20 {
            s.record_recent(format!("/tmp/p{i}.multiviewer"));
        }
        assert_eq!(s.recent_projects.len(), RECENT_MAX);
        assert_eq!(s.recent_projects[0], PathBuf::from("/tmp/p19.multiviewer"));
    }

    #[test]
    fn legacy_session_json_without_recent_loads() {
        let s: Session = serde_json::from_str(r#"{"last_project":"/tmp/a"}"#).unwrap();
        assert_eq!(s.last_project, Some(PathBuf::from("/tmp/a")));
        assert!(s.recent_projects.is_empty());
    }

    #[test]
    fn session_default_stamps_current_version() {
        let v = serde_json::to_value(Session::default()).unwrap();
        assert_eq!(v["version"], serde_json::json!(SESSION_VERSION));
    }

    #[test]
    fn session_version_defaults_to_legacy_when_absent() {
        let s: Session = serde_json::from_str(r#"{"last_project":"/tmp/a"}"#).unwrap();
        assert_eq!(s.version, LEGACY_VERSION);
    }

    #[test]
    fn prune_missing_drops_nonexistent_paths() {
        let mut s = Session::default();
        s.recent_projects = vec![std::env::temp_dir(), PathBuf::from("/nonexistent/x.multiviewer")];
        s.prune_missing();
        assert_eq!(s.recent_projects, vec![std::env::temp_dir()]);
    }
}

