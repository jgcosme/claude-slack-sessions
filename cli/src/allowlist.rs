use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Allowlist {
    #[serde(default)]
    pub slack_user_ids: BTreeSet<String>,
}

#[allow(dead_code)] // file is shared between cli and daemon; not all methods are used in both
impl Allowlist {
    pub fn config_path() -> std::io::Result<PathBuf> {
        let dir = dirs::config_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no config dir for this platform")
        })?;
        Ok(dir.join("slack-sessions").join("allowlist.json"))
    }

    pub fn load() -> std::io::Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, raw)
    }

    pub fn contains(&self, user_id: &str) -> bool {
        self.slack_user_ids.contains(user_id)
    }

    /// Slack user IDs start with `U` (regular) or `W` (Enterprise), followed by
    /// uppercase alphanumerics. Length is typically 9–11 chars but Slack does
    /// not document a strict cap, so we just require >= 9 total.
    pub fn validate_user_id(id: &str) -> Result<(), String> {
        if id.len() < 9 {
            return Err(format!("user id too short: {:?} (expected `U…` or `W…`)", id));
        }
        let mut chars = id.chars();
        match chars.next() {
            Some('U') | Some('W') => {}
            _ => return Err(format!("user id must start with `U` or `W`: {:?}", id)),
        }
        if !chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Err(format!(
                "user id must be uppercase alphanumeric after the leading letter: {:?}",
                id
            ));
        }
        Ok(())
    }
}
