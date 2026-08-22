use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::APP_NAME;
const SESSION_FILENAME: &str = "session.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Session {
    pub last_project: Option<PathBuf>,
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
        Self::load_from(app_data_dir()).unwrap_or_default()
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

    pub fn set_last_project(&mut self, path: impl AsRef<Path>) -> Result<(), SessionError> {
        self.last_project = Some(path.as_ref().to_path_buf());
        self.save()
    }
}

pub fn app_data_dir() -> Result<PathBuf, SessionError> {
    dirs::data_local_dir()
        .map(|d| d.join(APP_NAME))
        .ok_or(SessionError::NoAppDataDir)
}

