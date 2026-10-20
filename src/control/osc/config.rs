use serde::{Deserialize, Serialize};

const INVALID_PREFIX_CHARS: [char; 8] = ['#', '*', ',', '?', '[', ']', '{', '}'];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OscControlConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    #[serde(default)]
    pub prefix: String,
}

impl Default for OscControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1".to_string(),
            port: 9000,
            prefix: String::new(),
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
        .any(|c| c.is_whitespace() || INVALID_PREFIX_CHARS.contains(&c))
    {
        return Err("prefix contains an OSC-special character");
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
    }

    #[test]
    fn rejects_invalid_prefixes() {
        assert!(normalize_prefix("/multi viewer").is_err());
        assert!(normalize_prefix("/a//b").is_err());
        assert!(normalize_prefix("/a?b").is_err());
    }
}
