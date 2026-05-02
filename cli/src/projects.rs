use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct ProjectsRegistry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_dir: Option<String>,
    #[serde(default)]
    pub projects: BTreeMap<String, String>,
}

#[allow(dead_code)] // file is shared between cli and daemon; not all methods are used in both
impl ProjectsRegistry {
    pub fn config_path() -> std::io::Result<PathBuf> {
        let dir = dirs::config_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no config dir for this platform")
        })?;
        Ok(dir.join("slack-sessions").join("projects.json"))
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

    pub fn resolved_default(&self) -> PathBuf {
        if let Some(d) = &self.default_dir {
            return PathBuf::from(d);
        }
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    }

    pub fn lookup(&self, name: &str) -> Option<PathBuf> {
        self.projects.get(name).map(PathBuf::from)
    }

    pub fn validate_name(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("name must not be empty".into());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("name must match [A-Za-z0-9_-]+: {:?}", name));
        }
        Ok(())
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if p == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(p)
}

/// Expand tilde, canonicalize, and verify it's an existing directory.
/// Returns canonical path on success or human-readable message on failure.
pub fn canonicalize_dir(path_str: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(path_str);
    let canonical = std::fs::canonicalize(&expanded)
        .map_err(|_| format!("path does not exist: {}", expanded.display()))?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", canonical.display()));
    }
    Ok(canonical)
}
