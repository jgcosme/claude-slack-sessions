//! Shared helpers for slack-sessions config files.
//!
//! All slack-sessions state lives under `~/.config/slack-sessions/`
//! (matching the obsidian-memory plugin's convention) regardless of OS.
//! Each file is JSON5 — strict-JSON-superset that allows `//` comments —
//! with a constant doc-block header re-emitted on every save so the file
//! is self-documenting when opened directly.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Canonical config directory: `~/.config/slack-sessions/`.
pub fn config_dir() -> std::io::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no home directory")
    })?;
    Ok(home.join(".config").join("slack-sessions"))
}

/// Load a JSON5 config file. Returns `Self::default()` if the file doesn't
/// exist; surfaces parse errors otherwise.
#[allow(dead_code)]
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> std::io::Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = std::fs::read_to_string(path)?;
    json5::from_str(&raw).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Write a config file as `<header>\n<json>` where the header is a constant
/// JSON5 comment block. Round-trips cleanly because load() uses json5 (which
/// skips comments), and the header is a code constant re-emitted on every
/// save — so manual edits to the comments are NOT preserved across writes,
/// but manual edits to the data section round-trip fine.
#[allow(dead_code)]
pub fn save_with_header<T: Serialize>(path: &Path, header: &str, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let combined = format!("{}\n{}\n", header.trim_end(), json);
    std::fs::write(path, combined)
}
