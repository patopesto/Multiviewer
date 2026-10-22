use serde::{Deserialize, Serialize};

const INVALID_PREFIX_CHARS: [char; 8] = ['#', '*', ',', '?', '[', ']', '{', '}'];

/// Where OSC feedback is sent. The modes are mutually exclusive.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeedbackMode {
    /// No feedback is sent.
    #[default]
    Off,
    /// Reply to addresses that recently sent a command.
    Senders,
    /// Send to the configured feedback host and port.
    Configured,
    /// Reply to senders and send to the configured host and port.
    Both,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OscControlConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub feedback_mode: FeedbackMode,
    #[serde(default)]
    pub feedback_host: String,
    #[serde(default)]
    pub feedback_port: u16,
}

impl Default for OscControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1".to_string(),
            port: 9000,
            prefix: String::new(),
            feedback_mode: FeedbackMode::Off,
            feedback_host: String::new(),
            feedback_port: 0,
        }
    }
}

/// Canonical form of the OSC address prefix: empty for root, otherwise a
/// single leading slash and no trailing slash.
pub fn normalize_prefix(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(String::new());
    }
    if raw
        .chars()
        .any(|c| (c.is_whitespace() && c != ' ') || INVALID_PREFIX_CHARS.contains(&c))
    {
        return Err("prefix contains an unsupported character");
    }
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let mut prefix = String::with_capacity(trimmed.len() + 1);
    if !trimmed.starts_with('/') {
        prefix.push('/');
    }
    prefix.push_str(trimmed);
    if prefix.contains("//") {
        return Err("prefix contains an empty segment");
    }
    return Ok(prefix);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_prefixes() {
        assert_eq!(normalize_prefix(""), Ok(String::new()));
        assert_eq!(normalize_prefix("/"), Ok(String::new()));
        assert_eq!(
            normalize_prefix("multiviewer"),
            Ok("/multiviewer".to_string())
        );
        assert_eq!(normalize_prefix("/mv/"), Ok("/mv".to_string()));
        assert_eq!(normalize_prefix(" /show/ "), Ok("/show".to_string()));
        assert_eq!(
            normalize_prefix("/my control"),
            Ok("/my control".to_string())
        );
        assert_eq!(
            normalize_prefix(" /my control/ "),
            Ok("/my control".to_string())
        );
    }

    #[test]
    fn rejects_invalid_prefixes() {
        assert!(normalize_prefix("/a//b").is_err());
        assert!(normalize_prefix("/a?b").is_err());
        assert!(normalize_prefix("/a\tb").is_err());
    }
}
