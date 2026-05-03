//! Slack token storage on disk.
//!
//! Tokens live in `~/.config/slack-sessions/credentials.json` with mode
//! 0600 on Unix. We deliberately avoid the OS keyring: every `cargo install`
//! re-signs the binary, which invalidates keychain ACLs and forces a fresh
//! prompt on every secret read — fatal for a headless launchd daemon.
//!
//! Threat model: same as `~/.aws/credentials`, `~/.config/gh/hosts.yml`,
//! `~/.netrc` — file mode 0600, plaintext, anything running as the user
//! can read it. Slack tokens are workspace-scoped and revocable from the
//! Slack admin UI, so leaked-token blast radius is bounded.
//!
//! Token resolution order in the daemon:
//!   1. environment variable (SLACK_APP_TOKEN / SLACK_BOT_TOKEN)
//!   2. credentials.json
//!   3. error pointing at `slack-sessions setup`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const FILENAME: &str = "credentials.json";
const HEADER: &str = "// slack-sessions credentials — written by `slack-sessions setup`.\n\
// Tokens here are plaintext, file mode 0600. To rotate, re-run `slack-sessions setup`.\n\
// To wipe, delete this file (or run `slack-sessions service uninstall --purge`).";

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
}

impl Credentials {
    pub fn path() -> std::io::Result<PathBuf> {
        Ok(crate::config::config_dir()?.join(FILENAME))
    }

    pub fn load() -> std::io::Result<Self> {
        let p = Self::path()?;
        if !p.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&p)?;
        json5::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    #[allow(dead_code)]
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let combined = format!("{}\n{}\n", HEADER, json);
        write_secret(&path, combined.as_bytes())
    }
}

#[cfg(unix)]
#[allow(dead_code)]
fn write_secret(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
fn write_secret(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}
